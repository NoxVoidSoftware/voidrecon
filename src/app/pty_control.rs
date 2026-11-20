//! PTY tab lifecycle, sizing, and key translation
use crate::app::state::{App, NotifLevel};
use crate::llm::LlmRequestState;
use crate::pty::tab::PtyTab;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::{Duration, Instant};

/// A command the LLM ran that is still going after this long (a server, a
/// listener, a long scan) is reported so the LLM can carry on meanwhile.
const RUNNING_REPORT_AFTER: Duration = Duration::from_secs(15);
/// ...once its output has been quiet for this long.
const RUNNING_QUIET_FOR: Duration = Duration::from_secs(3);

/// Width of the expanded sidebar in terminal columns.
const SIDEBAR_WIDTH: u16 = 40;

impl App {
    pub async fn handle_pty_data(&mut self, _tab_id: usize, _data: Vec<u8>) -> Result<()> {
        // Data already processed by reader thread
        // Just ensure UI updates - we could force a redraw here if needed
        Ok(())
    }
    
    /// A PTY process exited: its tab is closed. Commands the LLM ran are
    /// recorded and analyzed; interactive command and listener tabs (ssh, nc,
    /// ...) save their output to workspace/outputs/ and the chat log.
    pub async fn handle_pty_exited(&mut self, tab_id: usize, exit_code: i32) -> Result<()> {
        let Some(pos) = self.tabs.iter().position(|t| t.id == tab_id) else {
            return Ok(());
        };
        // Command and listener sessions keep their output; plain shells are the user's own.
        let keep_output = matches!(
            self.tabs[pos].kind,
            crate::pty::tab::TerminalKind::Command | crate::pty::tab::TerminalKind::Listener
        );
        let transcript = self.tabs[pos].transcript.clone();
        let was_viewing = self.active_tab == pos + 1;
        // Dropping the tab joins its reader thread, so the transcript is complete afterwards.
        let label = self.remove_tab_at(pos);
        self.add_notification(format!("{} closed (exit code {})", label, exit_code), NotifLevel::Info);

        if let Some(run) = self.tab_command_runs.remove(&tab_id) {
            if was_viewing {
                self.active_tab = 0;
                self.ui_state.focus = crate::app::state::WindowFocus::Input;
            }
            let output = PtyTab::transcript_text(&transcript);
            let result = crate::app::events::CommandResult {
                cmd: run.command,
                output,
                exit_code,
                duration_ms: run.started.elapsed().as_millis() as i64,
            };
            return self.finish_command_result(result).await;
        }

        if keep_output {
            let output = PtyTab::transcript_text(&transcript);
            if !output.is_empty() {
                let output = format!("[{} exited with code {}]\n{}", label, exit_code, output);
                // Save the full session output so the LLM can read the parts an
                // excerpt leaves out (file writes run off the event loop).
                let session_path = self.session_meta.path();
                let to_save = output.clone();
                let saved_path = tokio::task::spawn_blocking(move || {
                    crate::db::DatabaseQueries::save_output_file(&session_path, &to_save)
                })
                .await
                .ok()
                .and_then(Result::ok);
                let chat_output = match &saved_path {
                    Some(path) if output.len() > Self::CHAT_OUTPUT_BYTES => {
                        Self::output_with_saved_path(&output, path)
                    }
                    _ => output,
                };
                self.append_chat_message(crate::llm::ChatMessage::output(chat_output))
                    .await;
            }
        }
        Ok(())
    }

    /// Closes the tab the user or LLM knows as "Tab `number`".
    pub fn close_terminal_tab(&mut self, number: usize) -> Result<String> {
        let pos = self
            .tabs
            .iter()
            .position(|tab| tab.number == number)
            .ok_or_else(|| anyhow::anyhow!("Terminal tab {} is not open", number))?;
        self.tab_command_runs.remove(&self.tabs[pos].id);
        Ok(self.remove_tab_at(pos))
    }

    /// Removes the tab at `pos` (index into `tabs`) and keeps the active tab valid.
    fn remove_tab_at(&mut self, pos: usize) -> String {
        let tab = self.tabs.remove(pos);
        let label = tab.label.clone();
        drop(tab);
        let tab_index = pos + 1;
        if self.active_tab == tab_index {
            self.active_tab = pos;
        } else if self.active_tab > tab_index {
            self.active_tab -= 1;
        }
        self.active_tab = self.active_tab.min(self.tabs.len());
        if self.active_tab == 0 {
            self.ui_state.focus = crate::app::state::WindowFocus::Input;
        }
        label
    }

    /// Adds a new tab with the lowest free number and focuses it. Returns its number.
    pub(super) fn add_tab(&mut self, mut tab: PtyTab) -> usize {
        let number = (1..)
            .find(|n| !self.tabs.iter().any(|t| t.number == *n))
            .unwrap_or(self.tabs.len() + 1);
        tab.number = number;
        self.tabs.push(tab);
        self.active_tab = self.tabs.len();
        number
    }

    /// Next internal tab id; ids are never reused so stale PTY events are ignored.
    pub(super) fn allocate_tab_id(&mut self) -> usize {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        id
    }
    
    pub async fn handle_tick(&mut self) -> bool {
        let now = Instant::now();
        let had_notifications = !self.notifications.is_empty();
        self.notifications.retain(|n| now.duration_since(n.created_at).as_secs() < 5);
        if self.notifications.len() > 3 {
            self.notifications.drain(0..self.notifications.len() - 3);
        }
        let has_notifications = !self.notifications.is_empty();
        self.last_tick = now;

        // Periodically refresh the exploit file cache so newly saved exploits
        // appear in the sidebar without blocking the render path.
        if now.duration_since(self.exploit_cache_time).as_secs() >= 30 {
            self.exploit_cache_time = now;
            self.refresh_exploit_cache().await;
        }

        self.report_long_running_command().await;

        let is_animating = matches!(
            self.llm.request_state,
            LlmRequestState::Streaming | LlmRequestState::ProcessingToolCalls
        );

        if is_animating {
            self.ui_state.tick_count = self.ui_state.tick_count.wrapping_add(1);
            true
        } else {
            had_notifications || has_notifications
        }
    }
    
    /// The LLM only continues when a command it ran exits. A server never
    /// exits, so once such a command has run for a while it gets the output
    /// so far, learns that the command keeps running, and continues.
    async fn report_long_running_command(&mut self) {
        let llm_idle = !matches!(
            self.llm.request_state,
            LlmRequestState::Streaming | LlmRequestState::ProcessingToolCalls
        ) && self.pending_exploit_approval.is_none()
            && !self.llm.suggestion_visible
            && self.llm.background_tool.is_none();
        if !llm_idle {
            return;
        }
        let due = self.tab_command_runs.iter().find_map(|(id, run)| {
            if run.reported_running || run.started.elapsed() < RUNNING_REPORT_AFTER {
                return None;
            }
            let tab = self.tabs.iter().find(|tab| tab.id == *id && tab.exit_code.is_none())?;
            let quiet = tab
                .last_output()
                .is_none_or(|time| time.elapsed() >= RUNNING_QUIET_FOR);
            quiet.then_some(*id)
        });
        let Some(id) = due else {
            return;
        };
        let Some(run) = self.tab_command_runs.get_mut(&id) else {
            return;
        };
        run.reported_running = true;
        let command = run.command.clone();
        let seconds = run.started.elapsed().as_secs();
        let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        let number = tab.number;
        let output = PtyTab::transcript_text(&tab.transcript);
        tab.mark_read();

        let shown = if output.is_empty() { "(no output yet)" } else { output.as_str() };
        self.append_chat_message(crate::llm::ChatMessage::output(format!(
            "[Tab {} still running after {}s: {}]\n{}",
            number, seconds, command, shown
        )))
        .await;
        self.ui_state.log_at_bottom = true;
        let note = format!(
            "NOTE: this command is STILL RUNNING in Tab {} after {}s (for example a server, listener or long scan); the output below is only what it printed so far. Do not wait for it to exit: carry on with the next step. Check it later with read_terminal_tab (wait_seconds waits for new output). Its full output is sent when it exits.",
            number, seconds
        );
        self.llm.post_exec_mode = true;
        if let Err(error) = self
            .send_post_exec_analysis(&command, shown, None, Some(&note))
            .await
        {
            self.add_notification(
                format!("Failed to update the LLM about Tab {}: {}", number, error),
                NotifLevel::Error,
            );
        }
    }

    pub async fn update_pty_dimensions(&mut self) -> Result<()> {
        let (pty_cols, pty_rows) = self.pty_dimensions();

        for tab in &mut self.tabs {
            let _ = tab.resize(pty_cols, pty_rows);
        }
        Ok(())
    }

    /// Columns currently taken by the sidebar (0 when collapsed).
    pub fn sidebar_width(&self) -> u16 {
        if self.ui_state.sidebar_collapsed {
            0
        } else {
            SIDEBAR_WIDTH
        }
    }

    pub fn pty_dimensions(&self) -> (u16, u16) {
        let (term_cols, term_rows) = self.terminal_size;
        (
            term_cols.saturating_sub(self.sidebar_width()).max(20),
            term_rows.saturating_sub(3).max(5),
        )
    }

    /// Number of the terminal tab that currently has focus, if any.
    pub fn active_terminal_tab_id(&self) -> Option<usize> {
        self.active_tab
            .checked_sub(1)
            .and_then(|index| self.tabs.get(index))
            .map(|tab| tab.number)
    }

    pub async fn new_shell_tab(&mut self) -> Result<()> {
        let id = self.allocate_tab_id();
        let tab = PtyTab::new_shell(id, self.event_tx.clone(), self.pty_dimensions())?;
        self.add_tab(tab);
        Ok(())
    }
    
    pub async fn spawn_listener_tab(&mut self, lport: u16) -> Result<()> {
        let id = self.allocate_tab_id();
        let tab = PtyTab::new_listener(id, lport, self.event_tx.clone(), self.pty_dimensions())?;
        self.add_tab(tab);
        Ok(())
    }
    
    pub async fn close_current_shell(&mut self) -> Result<()> {
        if let Some(number) = self.active_terminal_tab_id() {
            self.close_terminal_tab(number)?;
        }
        Ok(())
    }
}

/// Convert crossterm KeyEvent to raw terminal bytes for PTY master stream
pub(super) fn key_to_pty_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let ascii = (c as u8).to_ascii_uppercase();
                if (b'@'..=b'_').contains(&ascii) {
                    Some(vec![ascii - b'@'])
                } else {
                    None
                }
            } else {
                let mut b = [0u8; 4];
                let s = c.encode_utf8(&mut b);
                Some(s.as_bytes().to_vec())
            }
        }
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Backspace => Some(vec![8]),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::Esc => Some(vec![27]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        _ => None,
    }
}

