//! User approval of LLM-requested actions (listeners, exploits, file saves, terminal input).

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use serde_json::{json, Value};
use std::time::Instant;

use crate::app::state::{App, ApprovalKind, ExploitApproval, InputMode, NotifLevel};
use crate::db::{DatabaseQueries, NewFinding};
use crate::llm::ChatMessage;

/// Tool result sent back to the LLM, plus the banner shown to the user.
type ActionOutcome = (Value, String, NotifLevel);

/// Lines of exploit output forwarded to the LLM before truncation kicks in.
const EXPLOIT_OUTPUT_MAX_LINES: usize = 30;
const EXPLOIT_OUTPUT_KEPT_LINES: usize = 25;

fn error_outcome(message: String, notification: String) -> ActionOutcome {
    (
        json!({ "status": "error", "message": message }),
        notification,
        NotifLevel::Error,
    )
}

fn truncate_exploit_output(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() > EXPLOIT_OUTPUT_MAX_LINES {
        format!(
            "{}\n... [truncated {} lines, full output displayed below] ...",
            lines[..EXPLOIT_OUTPUT_KEPT_LINES].join("\n"),
            lines.len() - EXPLOIT_OUTPUT_KEPT_LINES
        )
    } else {
        output.to_string()
    }
}

fn rejection_message(kind: &ApprovalKind) -> &'static str {
    match kind {
        ApprovalKind::ReverseShellListener { .. } => "User rejected spawning reverse shell listener",
        ApprovalKind::ExecuteExploit { .. } => "User rejected exploit script execution",
        ApprovalKind::SaveWorkspaceFile { .. } => "User rejected saving workspace file",
        ApprovalKind::TerminalInput { .. } => "User rejected sending terminal input",
        ApprovalKind::CloseTerminalTab { .. } => "User rejected closing terminal tab",
    }
}

impl App {
    /// Handles a key press while an approval prompt is pending: y/Enter approves,
    /// n/Esc rejects, `?` drops the action and switches to LLM query mode.
    pub(super) async fn handle_approval_key(
        &mut self,
        key: KeyEvent,
        approval: ExploitApproval,
    ) -> Result<()> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                self.pending_exploit_approval = None;
                self.run_approved_action(approval).await;
                self.continue_llm_conversation().await
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.pending_exploit_approval = None;
                let result = json!({
                    "status": "rejected",
                    "message": rejection_message(&approval.kind)
                });
                self.append_chat_message(ChatMessage::tool_result(
                    approval.tool_call.id,
                    result.to_string(),
                ))
                .await;
                self.add_notification("Action rejected".to_string(), NotifLevel::Info);
                self.continue_llm_conversation().await
            }
            KeyCode::Char('?') => {
                self.pending_exploit_approval = None;
                self.input_mode = InputMode::LlmQuery;
                self.input_buffer.clear();
                self.ui_state.input_cursor = 0;
                self.add_notification(
                    "Action cancelled - entering LLM query mode".to_string(),
                    NotifLevel::Info,
                );
                Ok(())
            }
            _ => {
                self.add_notification(
                    "Action pending: Press [Y/Enter] to approve, [N/Esc] to reject, or [?] to ask query".to_string(),
                    NotifLevel::Warning,
                );
                Ok(())
            }
        }
    }

    /// Performs the approved action and reports its outcome as the tool result.
    async fn run_approved_action(&mut self, approval: ExploitApproval) {
        let ExploitApproval {
            target_id,
            kind,
            tool_call,
            ..
        } = approval;

        // Chat-log entries shown after the tool result. The result must directly
        // follow the assistant's tool call, or the LLM request drops the turn.
        let mut transcript = Vec::new();
        let (result, notification, level) = match kind {
            ApprovalKind::ReverseShellListener {
                lhost,
                lport,
                shell_type,
            } => {
                self.approve_listener(target_id, &lhost, lport, &shell_type)
                    .await
            }
            ApprovalKind::ExecuteExploit {
                script_name,
                command_line,
            } => {
                self.approve_exploit(target_id, &script_name, &command_line, &mut transcript)
                    .await
            }
            ApprovalKind::SaveWorkspaceFile {
                folder,
                filename,
                description,
                content,
            } => {
                self.approve_workspace_file(target_id, folder, filename, description, &content)
                    .await
            }
            ApprovalKind::TerminalInput {
                tab_id,
                input,
                press_enter,
            } => self.approve_terminal_input(tab_id, &input, press_enter),
            ApprovalKind::CloseTerminalTab { tab_id, .. } => self.approve_close_tab(tab_id),
        };

        self.append_chat_message(ChatMessage::tool_result(tool_call.id, result.to_string()))
            .await;
        for message in transcript {
            self.append_chat_message(message).await;
        }
        self.add_notification(notification, level);
    }

    async fn approve_listener(
        &mut self,
        target_id: i64,
        lhost: &str,
        lport: u16,
        shell_type: &str,
    ) -> ActionOutcome {
        match self.spawn_listener_tab(lport).await {
            Ok(()) => (
                json!({
                    "status": "approved",
                    "message": format!("Listener spawned on port {}", lport),
                    "target_id": target_id,
                    "lhost": lhost,
                    "lport": lport,
                    "shell_type": shell_type,
                }),
                format!("Spawned listener on port {}", lport),
                NotifLevel::Info,
            ),
            Err(e) => error_outcome(
                format!("Failed to spawn listener on port {}: {}", lport, e),
                format!("Failed to spawn listener: {}", e),
            ),
        }
    }

    async fn approve_exploit(
        &mut self,
        target_id: i64,
        script_name: &str,
        command_line: &str,
        transcript: &mut Vec<ChatMessage>,
    ) -> ActionOutcome {
        let start = Instant::now();
        transcript.push(ChatMessage::user(format!("$ {}", command_line)));

        let output_result = self.run_command_capture(command_line).await;
        let duration_ms = start.elapsed().as_millis() as i64;

        match output_result {
            Ok(out) => {
                transcript.push(ChatMessage::output(out.clone()));
                self.record_exploit_run(target_id, command_line, &out, duration_ms)
                    .await;
                (
                    json!({
                        "status": "executed",
                        "script": script_name,
                        "exit_code": 0,
                        "duration_ms": duration_ms,
                        "output": truncate_exploit_output(&out)
                    }),
                    format!("Exploit executed ({})", script_name),
                    NotifLevel::Info,
                )
            }
            Err(e) => (
                json!({
                    "status": "execution_failed",
                    "script": script_name,
                    "error": e.to_string()
                }),
                format!("Exploit failed: {}", e),
                NotifLevel::Error,
            ),
        }
    }

    /// Records the exploit command and its output artifact in the database.
    /// Blocking SQLite + artifact writes run off the event loop.
    async fn record_exploit_run(
        &self,
        target_id: i64,
        command_line: &str,
        output: &str,
        duration_ms: i64,
    ) {
        let db = self.db.clone();
        let session_path = self.session_meta.path();
        let command_line = command_line.to_string();
        let output = output.to_string();
        let _ = tokio::task::spawn_blocking(move || {
            let _ = DatabaseQueries::add_command(
                &db.conn(),
                &session_path,
                Some(target_id),
                &command_line,
                Some(&output),
                Some(0),
                Some(duration_ms),
            );
            let _ = DatabaseQueries::add_chat_message(
                &db.conn(),
                Some(target_id),
                "user",
                &format!("$ {}", command_line),
            );
            let _ = DatabaseQueries::add_chat_message(
                &db.conn(),
                Some(target_id),
                "assistant",
                &output,
            );
        })
        .await;
    }

    async fn approve_workspace_file(
        &mut self,
        target_id: i64,
        folder: String,
        filename: String,
        description: String,
        content: &str,
    ) -> ActionOutcome {
        let target_dir = self.session_meta.workspace_path().join(&folder);
        let _ = tokio::fs::create_dir_all(&target_dir).await;
        if let Err(e) = tokio::fs::write(target_dir.join(&filename), content).await {
            return (
                json!({ "status": "save_failed", "error": e.to_string() }),
                format!("Failed to save file: {}", e),
                NotifLevel::Error,
            );
        }

        let rel_path = format!("workspace/{}/{}", folder, filename);
        let result = json!({
            "status": "approved_and_saved",
            "folder": folder,
            "file": filename,
            "path": rel_path,
            "bytes": content.len()
        });
        let notification = format!("Approved & saved: {}", rel_path);

        self.record_workspace_file(target_id, folder, filename, description, rel_path)
            .await;
        let _ = self.reload_sidebar().await;
        self.refresh_exploit_cache().await;

        (result, notification, NotifLevel::Info)
    }

    /// Evidence files become findings and artifacts become notes on the target.
    async fn record_workspace_file(
        &self,
        target_id: i64,
        folder: String,
        filename: String,
        description: String,
        rel_path: String,
    ) {
        let db = self.db.clone();
        let _ = tokio::task::spawn_blocking(move || match folder.as_str() {
            "evidence" => {
                let title = if description.is_empty() {
                    format!("Evidence: {}", filename)
                } else {
                    format!("Evidence: {} ({})", filename, description)
                };
                let _ = DatabaseQueries::add_finding(
                    &db.conn(),
                    &NewFinding {
                        evidence_path: Some(&rel_path),
                        ..NewFinding::new(target_id, &title, &description, "info")
                    },
                );
            }
            "artifacts" => {
                let note_text = if description.is_empty() {
                    format!("Saved artifact: {}", rel_path)
                } else {
                    format!("Saved artifact {}: {}", rel_path, description)
                };
                let _ = DatabaseQueries::add_note(&db.conn(), target_id, &note_text);
            }
            _ => {}
        })
        .await;
    }

    fn approve_terminal_input(
        &mut self,
        tab_id: usize,
        input: &str,
        press_enter: bool,
    ) -> ActionOutcome {
        let sent = match self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
            None => Err(anyhow::anyhow!("Terminal tab {} is no longer open", tab_id)),
            Some(tab) if tab.exit_code.is_some() => Err(anyhow::anyhow!(
                "Terminal tab {} has already exited",
                tab_id
            )),
            Some(tab) => {
                let mut bytes = input.as_bytes().to_vec();
                if press_enter {
                    bytes.push(b'\n');
                }
                tab.write(&bytes)
            }
        };
        match sent {
            Ok(()) => (
                json!({
                    "status": "input_sent",
                    "tab_id": tab_id,
                    "press_enter": press_enter
                }),
                format!("Sent approved input to terminal tab {}", tab_id),
                NotifLevel::Info,
            ),
            Err(error) => error_outcome(
                error.to_string(),
                format!("Failed to send terminal input: {}", error),
            ),
        }
    }

    fn approve_close_tab(&mut self, tab_id: usize) -> ActionOutcome {
        match self.close_terminal_tab(tab_id) {
            Ok(label) => (
                json!({
                    "status": "closed",
                    "tab_id": tab_id,
                    "label": label
                }),
                format!("Closed terminal tab {}", tab_id),
                NotifLevel::Info,
            ),
            Err(error) => error_outcome(
                error.to_string(),
                format!("Failed to close terminal tab: {}", error),
            ),
        }
    }
}
