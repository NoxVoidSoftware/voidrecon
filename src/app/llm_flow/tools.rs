//! Dispatch of LLM tool calls: terminal-tab tools, approval requests, and
//! post-processing of tool results.

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::app::state::{App, ApprovalKind, ExploitApproval, NotifLevel};
use crate::db::DatabaseQueries;
use crate::llm::types::{LlmSuggestion, ToolCall};
use crate::llm::ChatMessage;
use crate::pty::tab::PtyTab;

/// Whether the tool loop may move on to the next call or must wait for the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolFlow {
    Continue,
    AwaitUser,
}

const DEFAULT_LISTENER_PORT: u16 = 4444;

const NO_NEW_OUTPUT_MESSAGE: &str = "Nothing new since your last read of this tab. Do not keep re-reading it: pass wait_seconds to wait for output (e.g. an incoming callback), or end your turn and tell the user what you are waiting for.";

/// The last `max_chars` characters of `text`.
fn tail_chars(text: &str, max_chars: usize) -> &str {
    text.char_indices()
        .rev()
        .nth(max_chars.saturating_sub(1))
        .map_or(text, |(index, _)| &text[index..])
}

fn str_field(value: &Value, key: &str, default: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn tab_id_arg(args: &Value) -> Result<usize> {
    args.get("tab_id")
        .and_then(Value::as_u64)
        .and_then(|id| usize::try_from(id).ok())
        .context("Invalid tab_id")
}

/// Reverse-shell listener commands (nc/ncat/socat) would block the workspace
/// terminal, so they are run in a dedicated listener tab instead.
fn is_listener_command(cmd: &str) -> bool {
    let lower = cmd.to_lowercase();
    ["nc -lv", "nc -l ", "ncat -l", "socat tcp-listen"]
        .iter()
        .any(|pattern| lower.contains(pattern))
}

/// First non-privileged port number mentioned in the command.
fn listener_port(cmd: &str) -> u16 {
    cmd.split_whitespace()
        .filter_map(|token| token.parse::<u16>().ok())
        .find(|port| *port > 1024)
        .unwrap_or(DEFAULT_LISTENER_PORT)
}

impl App {
    /// Runs every tool call of the finished assistant turn, appending each
    /// result to the conversation.
    ///
    /// Returns `false` when the loop stopped early because an action is
    /// waiting for user approval.
    pub(super) async fn process_tool_calls(&mut self, tool_calls: &[ToolCall]) -> Result<bool> {
        for (index, tool_call) in tool_calls.iter().enumerate() {
            if let Some((task, label)) = self.background_tool_task(tool_call) {
                self.start_background_tool(tool_call, task, label, tool_calls[index + 1..].to_vec());
                return Ok(false);
            }
            let managed_result = match self.handle_terminal_management_tool(tool_call) {
                Ok(result) => result,
                Err(error) => Some(self.tool_error(tool_call, &error)),
            };
            if self.pending_exploit_approval.is_some() {
                self.defer_tool_calls(&tool_calls[index + 1..]).await;
                return Ok(false);
            }

            let result = match managed_result {
                Some(result) => result,
                None => {
                    let executed = crate::tools::execute_tool_call(
                        tool_call,
                        &self.db,
                        self.active_target_id,
                        &self.session_meta.workspace_path(),
                        &self.event_tx,
                    )
                    .await;
                    match executed {
                        Ok(result) => result,
                        Err(error) => self.tool_error(tool_call, &error),
                    }
                }
            };

            if self.apply_tool_result(tool_call, result).await? == ToolFlow::AwaitUser {
                self.defer_tool_calls(&tool_calls[index + 1..]).await;
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Answers the tool calls that are skipped because an earlier one is
    /// waiting for approval. Every call of an assistant turn needs a result,
    /// otherwise the whole turn is dropped from the next request.
    async fn defer_tool_calls(&mut self, deferred: &[ToolCall]) {
        for tool_call in deferred {
            self.append_chat_message(ChatMessage::tool_result(
                tool_call.id.clone(),
                json!({
                    "status": "not_executed",
                    "message": "A prior action is waiting for user approval."
                })
                .to_string(),
            ))
            .await;
        }
    }

    pub(super) fn tool_error(&mut self, tool_call: &ToolCall, error: &anyhow::Error) -> Value {
        self.add_notification(
            format!("Tool {} error: {}", tool_call.function.name, error),
            NotifLevel::Error,
        );
        json!({
            "status": "error",
            "error": error.to_string()
        })
    }

    /// Applies tool-specific side effects (notifications, suggestions, approval
    /// requests) and records the tool result in the conversation.
    async fn apply_tool_result(&mut self, tool_call: &ToolCall, result: Value) -> Result<ToolFlow> {
        match tool_call.function.name.as_str() {
            "save_exploit_script" => {
                if let Some(path) = result.get("path").and_then(Value::as_str) {
                    self.add_notification(format!("Saved exploit to {}", path), NotifLevel::Info);
                }
            }
            "run_command" => {
                // A listener's tool result is written once the user approves or rejects it.
                if self.apply_run_command(tool_call, &result).await == ToolFlow::AwaitUser {
                    return Ok(ToolFlow::AwaitUser);
                }
            }
            "save_workspace_file" => {
                self.request_workspace_file_approval(tool_call, &result).await;
                return Ok(ToolFlow::AwaitUser);
            }
            "set_target" | "add_target" => self.activate_target_from_result(&result),
            "execute_exploit_script" => {
                return self.request_exploit_approval(tool_call, &result).await;
            }
            _ => {}
        }

        // Add tool result to conversation for next LLM call
        self.append_chat_message(ChatMessage::tool_result(
            tool_call.id.clone(),
            result.to_string(),
        ))
        .await;
        Ok(ToolFlow::Continue)
    }

    fn request_approval(&mut self, approval: ExploitApproval, notification: String) {
        self.pending_exploit_approval = Some(approval);
        self.add_notification(notification, NotifLevel::Warning);
    }

    /// Display name of a target for approval prompts.
    async fn target_ip_label(&self, target_id: i64) -> String {
        if target_id <= 0 {
            return "active_target".to_string();
        }
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            DatabaseQueries::get_target(&db.conn(), target_id)
                .ok()
                .flatten()
                .map(|target| target.ip_host)
        })
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "target".to_string())
    }

    /// A suggested command is either offered to the user for execution or, for
    /// reverse-shell listeners, turned into a listener-tab approval request.
    async fn apply_run_command(&mut self, tool_call: &ToolCall, result: &Value) -> ToolFlow {
        let Ok(args) = serde_json::from_str::<Value>(&tool_call.function.arguments) else {
            return ToolFlow::Continue;
        };
        let cmd = args.get("command").and_then(Value::as_str).unwrap_or("");
        if cmd.is_empty() {
            return ToolFlow::Continue;
        }

        if is_listener_command(cmd) {
            let port = listener_port(cmd);
            let target_id = self.active_target_id.unwrap_or(0);
            let target_ip = self.target_ip_label(target_id).await;
            self.request_approval(
                ExploitApproval {
                    target_id,
                    target_ip,
                    kind: ApprovalKind::ReverseShellListener {
                        lhost: "0.0.0.0".to_string(),
                        lport: port,
                        shell_type: "bash".to_string(),
                    },
                    tool_call: tool_call.clone(),
                },
                format!("Approval required for reverse shell on port {}", port),
            );
            ToolFlow::AwaitUser
        } else {
            let interactive = result
                .get("interactive")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || Self::is_interactive_command(cmd);
            self.llm.suggestion = Some(LlmSuggestion {
                command: Some(cmd.to_string()),
                explanation: str_field(&args, "explanation", ""),
                interactive,
            });
            self.llm.suggestion_visible = true;
            ToolFlow::Continue
        }
    }

    async fn request_workspace_file_approval(&mut self, tool_call: &ToolCall, result: &Value) {
        let target_id = result
            .get("target_id")
            .and_then(Value::as_i64)
            .or(self.active_target_id)
            .unwrap_or(0);
        let target_ip = self.target_ip_label(target_id).await;
        let filename = str_field(result, "filename", "file.txt");
        let notification = format!("Approval required to save file to {}", filename);
        self.request_approval(
            ExploitApproval {
                target_id,
                target_ip,
                kind: ApprovalKind::SaveWorkspaceFile {
                    folder: str_field(result, "folder", "evidence"),
                    filename,
                    description: str_field(result, "description", ""),
                    content: str_field(result, "content", ""),
                },
                tool_call: tool_call.clone(),
            },
            notification,
        );
    }

    fn activate_target_from_result(&mut self, result: &Value) {
        let target_id = result
            .get("id")
            .and_then(Value::as_i64)
            .or_else(|| result.get("target_id").and_then(Value::as_i64));
        if let Some(target_id) = target_id {
            self.active_target_id = Some(target_id);
            let ip = result
                .get("ip_host")
                .and_then(Value::as_str)
                .unwrap_or("target");
            self.add_notification(
                format!("Active target: {} (ID: {})", ip, target_id),
                NotifLevel::Info,
            );
        }
    }

    /// Exploit scripts only run against targets in scope, and only after approval.
    async fn request_exploit_approval(
        &mut self,
        tool_call: &ToolCall,
        result: &Value,
    ) -> Result<ToolFlow> {
        let target_id = result.get("target_id").and_then(Value::as_i64).unwrap_or(0);
        let db = self.db.clone();
        let target = tokio::task::spawn_blocking(move || {
            DatabaseQueries::get_target(&db.conn(), target_id)
        })
        .await
        .context("Target lookup task panicked")??;

        let Some(target) = target else {
            self.append_chat_message(ChatMessage::tool_result(
                tool_call.id.clone(),
                json!({
                    "error": format!("Target ID {} is not within current assessment scope", target_id)
                })
                .to_string(),
            ))
            .await;
            return Ok(ToolFlow::Continue);
        };

        let script_name = str_field(result, "script_name", "exploit.py");
        let notification = format!("Approval required to execute {}", script_name);
        self.request_approval(
            ExploitApproval {
                target_id,
                target_ip: target.ip_host,
                kind: ApprovalKind::ExecuteExploit {
                    script_name,
                    command_line: str_field(result, "command_line", ""),
                },
                tool_call: tool_call.clone(),
            },
            notification,
        );
        Ok(ToolFlow::AwaitUser)
    }

    fn open_tab(&self, tab_id: usize) -> Result<&PtyTab> {
        self.tabs
            .iter()
            .find(|tab| tab.number == tab_id)
            .with_context(|| format!("Terminal tab {} is not open", tab_id))
    }

    fn request_terminal_approval(
        &mut self,
        tool_call: &ToolCall,
        kind: ApprovalKind,
        notification: String,
    ) -> Value {
        self.request_approval(
            ExploitApproval {
                target_id: self.active_target_id.unwrap_or_default(),
                target_ip: "terminal session".to_string(),
                kind,
                tool_call: tool_call.clone(),
            },
            notification,
        );
        json!({"status": "requires_user_approval"})
    }

    /// Handles the tools that operate on terminal tabs, which live in `App`
    /// rather than the database. Returns `None` for any other tool.
    fn handle_terminal_management_tool(&mut self, tool_call: &ToolCall) -> Result<Option<Value>> {
        let name = tool_call.function.name.as_str();
        if !matches!(
            name,
            "list_terminal_tabs" | "read_terminal_tab" | "send_terminal_input" | "close_terminal_tab"
        ) {
            return Ok(None);
        }
        crate::tools::validate_tool_call(tool_call)?;
        let args: Value = serde_json::from_str(&tool_call.function.arguments)
            .context("Failed to parse terminal tool arguments")?;

        let result = match name {
            "list_terminal_tabs" => self.list_terminal_tabs(),
            "read_terminal_tab" => self.read_terminal_tab(&args)?,
            "send_terminal_input" => self.request_terminal_input(tool_call, &args)?,
            "close_terminal_tab" => {
                let tab_id = tab_id_arg(&args)?;
                let label = self.open_tab(tab_id)?.label.clone();
                self.request_terminal_approval(
                    tool_call,
                    ApprovalKind::CloseTerminalTab { tab_id, label },
                    format!("Approval required to close terminal tab {}", tab_id),
                )
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    fn list_terminal_tabs(&self) -> Value {
        let active_tab_id = self.active_terminal_tab_id();
        let mut tabs = vec![json!({
            "tab_id": 0,
            "label": "Workspace & LLM",
            "kind": "workspace",
            "state": "open",
            "active": self.active_tab == 0,
            "manageable": false
        })];
        tabs.extend(self.tabs.iter().map(|tab| {
            json!({
                "tab_id": tab.number,
                "label": tab.label,
                "kind": tab.kind.as_str(),
                "state": tab.state_label(),
                "active": active_tab_id == Some(tab.number),
                "unread_bytes": tab.unread_bytes()
            })
        }));
        json!({"tabs": tabs})
    }

    /// Reads a tab for the LLM. The default `new` mode returns only output
    /// printed since the LLM's previous read of that tab, so it can tell what
    /// it has already seen from what just arrived.
    pub(super) fn read_terminal_tab(&mut self, args: &Value) -> Result<Value> {
        let tab_id = tab_id_arg(args)?;
        let max_chars = args
            .get("max_chars")
            .and_then(Value::as_u64)
            .unwrap_or(8_000)
            .clamp(200, 12_000) as usize;
        let legacy_scrollback = args.get("scrollback").and_then(Value::as_bool) == Some(true);
        let mode = args
            .get("mode")
            .and_then(Value::as_str)
            .unwrap_or(if legacy_scrollback { "all" } else { "new" });
        let tab = self
            .tabs
            .iter_mut()
            .find(|tab| tab.number == tab_id)
            .with_context(|| {
                format!(
                    "Terminal tab {} is not open. If it exited, its output was saved and posted to the chat.",
                    tab_id
                )
            })?;

        let (unread, last_output) = tab.unread_output();
        let mut result = json!({
            "tab_id": tab_id,
            "label": tab.label,
            "state": tab.state_label(),
            "mode": mode,
            "seconds_since_last_output": last_output.map(|time| time.elapsed().as_secs()),
        });
        match mode {
            "screen" => {
                // A look at the screen does not mark the output as read.
                result["screen"] = json!(tail_chars(&tab.screen_text(), max_chars));
                result["unread_chars"] = json!(unread.text.chars().count());
            }
            "all" => {
                let output = PtyTab::transcript_text(&tab.transcript);
                result["total_chars"] = json!(output.chars().count());
                result["output"] = json!(tail_chars(&output, max_chars));
                tab.llm_read_offset = unread.end;
            }
            _ => {
                result["mode"] = json!("new");
                let new_chars = unread.text.chars().count();
                result["has_new_output"] = json!(new_chars > 0);
                if new_chars == 0 {
                    result["new_output"] = json!("");
                    result["message"] = json!(NO_NEW_OUTPUT_MESSAGE);
                    result["screen_tail"] = json!(tail_chars(&tab.screen_text(), 600));
                } else {
                    result["new_output"] = json!(tail_chars(&unread.text, max_chars));
                    if new_chars > max_chars {
                        result["omitted_chars"] = json!(new_chars - max_chars);
                        result["note"] = json!("Only the end of the new output is shown. Use mode=all with a larger max_chars, or read_workspace_file once the tab exits.");
                    }
                    if unread.lost_bytes > 0 {
                        result["lost_bytes"] = json!(unread.lost_bytes);
                    }
                }
                tab.llm_read_offset = unread.end;
            }
        }
        Ok(result)
    }

    fn request_terminal_input(&mut self, tool_call: &ToolCall, args: &Value) -> Result<Value> {
        let tab_id = tab_id_arg(args)?;
        if self.open_tab(tab_id)?.exit_code.is_some() {
            anyhow::bail!("Terminal tab {} has already exited", tab_id);
        }
        let input = args
            .get("input")
            .and_then(Value::as_str)
            .context("Missing terminal input")?;
        if input.len() > 100 || input.chars().any(|character| character.is_control()) {
            anyhow::bail!(
                "Terminal input must be at most 100 UTF-8 bytes and contain no control characters"
            );
        }
        let press_enter = args
            .get("press_enter")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(self.request_terminal_approval(
            tool_call,
            ApprovalKind::TerminalInput {
                tab_id,
                input: input.to_string(),
                press_enter,
            },
            format!("Approval required to send input to terminal tab {}", tab_id),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_terminal_tab_returns_only_new_output() {
        let mut app = App::new().await.expect("App::new failed");
        app.new_shell_tab().await.expect("shell tab");
        let tab_id = app.tabs[0].number;
        let args = json!({"tab_id": tab_id});
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        app.read_terminal_tab(&args).expect("initial read");

        if let Ok(mut transcript) = app.tabs[0].transcript.lock() {
            transcript.append(b"callback from 10.10.14.2\n");
        }
        let first = app.read_terminal_tab(&args).expect("read new output");
        assert_eq!(first["has_new_output"], json!(true));
        assert!(first["new_output"].as_str().unwrap_or("").contains("callback from 10.10.14.2"));

        let again = app.read_terminal_tab(&args).expect("read again");
        assert!(!again["new_output"].as_str().unwrap_or("").contains("callback"));
        assert_eq!(app.tabs[0].unread_bytes(), 0);
    }

    #[tokio::test]
    async fn waiting_read_returns_when_output_arrives() {
        let mut app = App::new().await.expect("App::new failed");
        app.new_shell_tab().await.expect("shell tab");
        let tab_id = app.tabs[0].number;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        app.read_terminal_tab(&json!({"tab_id": tab_id})).expect("initial read");

        let call: ToolCall = serde_json::from_value(json!({
            "id": "call_1",
            "type": "function",
            "function": {
                "name": "read_terminal_tab",
                "arguments": json!({"tab_id": tab_id, "wait_seconds": 20}).to_string()
            }
        }))
        .expect("tool call");
        let (task, _) = app.background_tool_task(&call).expect("waits when nothing is new");

        let transcript = app.tabs[0].transcript.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(600)).await;
            if let Ok(mut transcript) = transcript.lock() {
                transcript.append(b"GET /shell.sh HTTP/1.1\n");
            }
        });
        let started = std::time::Instant::now();
        let outcome = task.await;
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "returned early");
        assert!(outcome["waited_seconds"].is_u64());
        // The shell may print first and end the wait sooner; let the append land.
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;

        let read = app.read_terminal_tab(&json!({"tab_id": tab_id})).expect("read");
        assert!(read["new_output"].as_str().unwrap_or("").contains("GET /shell.sh"));
    }
}
