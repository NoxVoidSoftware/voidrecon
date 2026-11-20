//! The "Open Terminal Tabs" and "Listening Ports" sections of the system
//! prompt: what runs where, for how long, and which ports it holds, so the
//! LLM does not forget its own servers or start them twice.

use std::collections::HashMap;
use std::time::Duration;

use crate::app::state::App;
use crate::pty::ports::{self, ListeningPort};
use crate::pty::tab::PtyTab;

const PORT_SCAN_TIMEOUT: Duration = Duration::from_secs(3);
/// Recent screen lines shown per running tab.
const TAIL_LINES: usize = 3;
const TAIL_LINE_CHARS: usize = 160;
const MAX_PORTS_LISTED: usize = 40;

/// Listening ports and the process tree at one moment.
#[derive(Debug, Default)]
pub struct PortSnapshot {
    pub ports: Vec<ListeningPort>,
    pub parents: HashMap<u32, u32>,
}

impl PortSnapshot {
    /// Reads ports off the event loop; `None` if the tools are missing or slow.
    pub async fn take() -> Option<Self> {
        let scan = tokio::task::spawn_blocking(|| PortSnapshot {
            ports: ports::listening_ports(),
            parents: ports::process_parents(),
        });
        tokio::time::timeout(PORT_SCAN_TIMEOUT, scan).await.ok()?.ok()
    }

    /// The open tab whose process tree owns `pid`.
    pub fn owning_tab<'a>(&self, pid: u32, tabs: &'a [PtyTab]) -> Option<&'a PtyTab> {
        tabs.iter().find(|tab| {
            tab.exit_code.is_none()
                && tab.pid.is_some_and(|root| ports::descends_from(pid, root, &self.parents))
        })
    }

    /// Ports held by a tab's processes.
    fn tab_ports(&self, tab: &PtyTab, tabs: &[PtyTab]) -> Vec<u16> {
        let mut held: Vec<u16> = self
            .ports
            .iter()
            .filter(|port| self.owning_tab(port.pid, tabs).is_some_and(|owner| owner.id == tab.id))
            .map(|port| port.port)
            .collect();
        held.dedup();
        held
    }
}

pub fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match seconds {
        0..=59 => format!("{}s", seconds),
        60..=3599 => format!("{}m{:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

fn screen_tail(tab: &PtyTab) -> Vec<String> {
    let screen = tab.screen_text();
    let mut lines: Vec<String> = screen
        .lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(TAIL_LINES)
        .map(|line| line.trim_end().chars().take(TAIL_LINE_CHARS).collect())
        .collect();
    lines.reverse();
    lines
}

impl App {
    pub(super) fn terminal_tabs_prompt(&self, snapshot: Option<&PortSnapshot>) -> String {
        let active_tab_id = self.active_terminal_tab_id();
        let mut prompt = format!(
            "\n\n## Open Terminal Tabs (live state, refreshed for every request)\nTab 0: Workspace & LLM [{}].",
            if self.active_tab == 0 { "active" } else { "inactive" }
        );
        for tab in &self.tabs {
            let started_by = if self.tab_command_runs.contains_key(&tab.id) || tab.command.is_some() {
                "started by you"
            } else {
                "user's shell"
            };
            let running = match tab.exit_code {
                None => format!("RUNNING for {}", format_duration(tab.started.elapsed())),
                Some(code) => format!("exited ({})", code),
            };
            prompt.push_str(&format!(
                "\n- Tab {} [{}, {}, {}, {}]",
                tab.number,
                tab.kind.as_str(),
                running,
                started_by,
                if active_tab_id == Some(tab.number) { "active" } else { "inactive" }
            ));
            match &tab.command {
                Some(command) => prompt.push_str(&format!("\n  command: {}", Self::quote_untrusted(command))),
                None => prompt.push_str(&format!("\n  label: {}", Self::quote_untrusted(&tab.label))),
            }
            if let Some(snapshot) = snapshot {
                let held = snapshot.tab_ports(tab, &self.tabs);
                if !held.is_empty() {
                    let list: Vec<String> = held.iter().map(u16::to_string).collect();
                    prompt.push_str(&format!("\n  listening on TCP port(s): {}", list.join(", ")));
                }
            }
            let activity = match (tab.unread_bytes(), tab.last_output()) {
                (0, _) => "no new output since your last read".to_string(),
                (bytes, Some(time)) => format!(
                    "{} bytes of NEW output you have not read yet (last output {} ago)",
                    bytes,
                    format_duration(time.elapsed())
                ),
                (bytes, None) => format!("{} bytes of NEW output you have not read yet", bytes),
            };
            prompt.push_str(&format!("\n  output: {}", activity));
            let tail = screen_tail(tab);
            if !tail.is_empty() {
                prompt.push_str("\n  screen ends with (untrusted):");
                for line in tail {
                    prompt.push_str(&format!("\n    {}", Self::quote_untrusted(&line)));
                }
            }
        }
        if self.tabs.is_empty() {
            prompt.push_str("\nNo terminal tabs are currently open.");
        }

        match snapshot {
            Some(snapshot) if !snapshot.ports.is_empty() => {
                prompt.push_str("\n\n## Listening TCP Ports On This Machine\nPorts already in use. Do not start another server or listener on one of them: reuse the tab that holds it, or pick a free port.");
                for port in snapshot.ports.iter().take(MAX_PORTS_LISTED) {
                    let owner = match snapshot.owning_tab(port.pid, &self.tabs) {
                        Some(tab) => format!("Tab {}", tab.number),
                        None => "not from a voidrecon tab".to_string(),
                    };
                    prompt.push_str(&format!(
                        "\n- {} on {} ({} pid {}, {})",
                        port.port,
                        port.address,
                        Self::quote_untrusted(&port.process),
                        port.pid,
                        owner
                    ));
                }
                if snapshot.ports.len() > MAX_PORTS_LISTED {
                    prompt.push_str(&format!("\n- ... {} more", snapshot.ports.len() - MAX_PORTS_LISTED));
                }
            }
            Some(_) => prompt.push_str("\n\n## Listening TCP Ports On This Machine\nNone."),
            None => {}
        }
        prompt
    }

    /// A suggested command that starts a server or listener on a port that is
    /// already listening on this machine. Returns why it should not run.
    pub(super) async fn port_conflict(&self, tool_call: &crate::llm::types::ToolCall) -> Option<String> {
        let args: serde_json::Value = serde_json::from_str(&tool_call.function.arguments).ok()?;
        let command = args.get("command")?.as_str()?;
        if !ports::starts_listener(command) {
            return None;
        }
        let wanted = ports::ports_in_command(command);
        if wanted.is_empty() {
            return None;
        }
        let snapshot = PortSnapshot::take().await?;
        let busy = snapshot.ports.iter().find(|port| wanted.contains(&port.port))?;
        let owner = match snapshot.owning_tab(busy.pid, &self.tabs) {
            Some(tab) => format!(
                "Tab {} ({}), which you can reuse; read it with read_terminal_tab",
                tab.number,
                tab.command.as_deref().unwrap_or(&tab.label)
            ),
            None => format!("{} (pid {}), not a voidrecon tab", busy.process, busy.pid),
        };
        Some(format!(
            "Not run: port {} is already in use by {}. Use that process or choose a free port.",
            busy.port, owner
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn running_server_shows_in_prompt_and_blocks_a_second_start() {
        let mut app = App::new().await.expect("App::new failed");
        app.config.api_key.clear();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("free port")
            .port();
        let command = format!("python3 -m http.server {} --bind 127.0.0.1", port);
        app.run_llm_command(&command, false).await.expect("start server");

        let mut snapshot = None;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            snapshot = PortSnapshot::take().await;
            if snapshot.as_ref().is_some_and(|s| s.ports.iter().any(|p| p.port == port)) {
                break;
            }
        }
        let snapshot = snapshot.expect("port snapshot");
        assert!(snapshot.ports.iter().any(|p| p.port == port), "server is listening");

        let prompt = app.terminal_tabs_prompt(Some(&snapshot));
        let tab_number = app.tabs[0].number;
        assert!(prompt.contains(&command), "full command is in the prompt:\n{}", prompt);
        assert!(prompt.contains("RUNNING for"));
        assert!(prompt.contains(&format!("listening on TCP port(s): {}", port)), "{}", prompt);
        assert!(prompt.contains(&format!("{} on 127.0.0.1", port)));
        assert!(prompt.contains(&format!("Tab {})", tab_number)), "{}", prompt);

        let call: crate::llm::types::ToolCall = serde_json::from_value(json!({
            "id": "call_1",
            "type": "function",
            "function": {
                "name": "run_command",
                "arguments": json!({"command": format!("python3 -m http.server {}", port), "explanation": "x"}).to_string()
            }
        }))
        .expect("tool call");
        let conflict = app.port_conflict(&call).await.expect("busy port is detected");
        assert!(conflict.contains(&format!("Tab {}", tab_number)), "{}", conflict);
    }
}
