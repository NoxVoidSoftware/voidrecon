//! YOLO mode: the LLM's suggested commands run and its approval requests
//! (exploits, listeners, terminal input, closing tabs, saving files) are
//! accepted without asking. Toggled with `:yolo`; off on every start.

use anyhow::Result;

use crate::app::state::{ApprovalKind, App, NotifLevel};
use crate::llm::LlmRequestState;

/// Automatic actions allowed per user message before YOLO pauses for the
/// user, so a model stuck in a loop cannot run forever.
pub const MAX_AUTO_ACTIONS: u32 = 200;

#[derive(Debug, Default)]
pub struct YoloState {
    pub enabled: bool,
    /// Actions taken automatically since the user's last message.
    pub auto_actions: u32,
}

fn approval_label(kind: &ApprovalKind) -> String {
    match kind {
        ApprovalKind::ReverseShellListener { lport, .. } => format!("listener on port {}", lport),
        ApprovalKind::ExecuteExploit { command_line, .. } => format!("exploit: {}", command_line),
        ApprovalKind::SaveWorkspaceFile { filename, .. } => format!("save file {}", filename),
        ApprovalKind::TerminalInput { tab_id, .. } => format!("input to tab {}", tab_id),
        ApprovalKind::CloseTerminalTab { tab_id, .. } => format!("close tab {}", tab_id),
    }
}

impl App {
    /// `:yolo [on|off]`; without an argument it toggles.
    pub async fn set_yolo(&mut self, arg: &str) -> Result<()> {
        let enabled = match arg.trim() {
            "on" | "1" | "true" => true,
            "off" | "0" | "false" => false,
            "" => !self.yolo.enabled,
            other => {
                self.add_notification(
                    format!("Unknown :yolo argument '{}'. Use :yolo on|off", other),
                    NotifLevel::Warning,
                );
                return Ok(());
            }
        };
        self.yolo = YoloState { enabled, auto_actions: 0 };
        if enabled {
            self.add_notification(
                "YOLO mode ON: AI commands and approvals run without asking (Ctrl+C cancels, :yolo off stops)".to_string(),
                NotifLevel::Warning,
            );
            // Anything already waiting for the user goes ahead now.
            self.run_yolo_step().await
        } else {
            self.add_notification("YOLO mode OFF".to_string(), NotifLevel::Info);
            Ok(())
        }
    }

    /// In YOLO mode, accepts whatever the LLM is waiting on: a pending
    /// approval or a suggested command. Called after each LLM turn ends.
    pub async fn run_yolo_step(&mut self) -> Result<()> {
        if !self.yolo.enabled
            || matches!(
                self.llm.request_state,
                LlmRequestState::Streaming | LlmRequestState::ProcessingToolCalls
            )
        {
            return Ok(());
        }
        let waiting = self.pending_exploit_approval.is_some()
            || (self.llm.suggestion_visible && self.llm.suggestion.is_some());
        if !waiting {
            return Ok(());
        }
        if self.yolo.auto_actions >= MAX_AUTO_ACTIONS {
            self.add_notification(
                format!(
                    "YOLO paused after {} automatic actions: approve this one yourself or send a new message",
                    MAX_AUTO_ACTIONS
                ),
                NotifLevel::Warning,
            );
            return Ok(());
        }
        self.yolo.auto_actions += 1;

        if let Some(approval) = self.pending_exploit_approval.take() {
            self.add_notification(
                format!("YOLO: approved {}", approval_label(&approval.kind)),
                NotifLevel::Warning,
            );
            if self.run_approved_action(approval).await {
                self.continue_llm_conversation().await?;
            }
            return Ok(());
        }

        self.llm.suggestion_visible = false;
        let Some(suggestion) = self.llm.suggestion.take() else {
            return Ok(());
        };
        if let Some(command) = suggestion.command {
            self.add_notification(format!("YOLO: running {}", command), NotifLevel::Warning);
            self.run_llm_command(&command, suggestion.interactive).await?;
        }
        Ok(())
    }
}
