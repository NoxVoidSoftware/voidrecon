//! Keeps what the user told the LLM in view: their messages are repeated in
//! the system prompt and survive history compaction verbatim, so a fact like
//! "Tab 3 is my SSH session, not the listener" is not lost after a few tool
//! calls.

use crate::app::state::App;
use crate::llm::ChatMessage;

/// User messages repeated in the system prompt, newest kept.
const PINNED_MESSAGES: usize = 12;
/// Bytes of user messages repeated in the system prompt.
const PINNED_BYTES: usize = 6_000;
/// Bytes kept of a single long user message.
const MESSAGE_BYTES: usize = 1_500;
/// User messages kept verbatim when older history is compacted.
pub(super) const KEPT_ON_COMPACTION: usize = 30;

/// Whether a chat-log entry is something the user typed to the LLM.
pub(super) fn is_user_query(message: &ChatMessage) -> bool {
    message.role == "user" && message.content.starts_with("? ")
}

impl App {
    /// System-prompt section repeating the user's recent messages, oldest
    /// first. Empty when the user has not written anything yet.
    pub(super) fn user_messages_prompt(&self) -> String {
        let mut selected = Vec::new();
        let mut bytes = 0;
        for message in self.llm.chat_log.iter().rev().filter(|m| is_user_query(m)) {
            if selected.len() >= PINNED_MESSAGES {
                break;
            }
            let text = Self::truncate_bytes(message.content.trim_start_matches("? ").trim(), MESSAGE_BYTES);
            if !selected.is_empty() && bytes + text.len() > PINNED_BYTES {
                break;
            }
            bytes += text.len();
            selected.push(text);
        }
        if selected.is_empty() {
            return String::new();
        }
        selected.reverse();

        let mut section = String::from(
            "\n\n## What The User Told You (most recent last)\nThese are the user's own messages, repeated here so you never lose them. Facts the user states (what runs in a tab, what a port is used for, what already worked or failed, what to avoid) are true until the user says otherwise and override your own assumptions. Instructions stay in force until the user changes them; the newest message wins when two conflict. Do not ask again for something the user already told you.\n",
        );
        let count = selected.len();
        for (index, text) in selected.iter().enumerate() {
            let marker = if index + 1 == count { " (latest)" } else { "" };
            section.push_str(&format!("{}.{} {}\n", index + 1, marker, text));
        }
        section
    }
}

#[cfg(test)]
mod tests {
    use crate::app::state::App;
    use crate::llm::types::{LlmRequestState, ToolCall};
    use crate::llm::ChatMessage;
    use serde_json::json;

    #[tokio::test]
    async fn user_messages_are_pinned_in_the_prompt() {
        let mut app = App::new().await.expect("App::new failed");
        app.llm.chat_log.clear();
        assert_eq!(app.user_messages_prompt(), "");

        app.llm.chat_log.push(ChatMessage::user("? Tab 2 is my SSH session, not a listener"));
        app.llm.chat_log.push(ChatMessage::user("$ id"));
        app.llm.chat_log.push(ChatMessage::assistant("ok"));
        app.llm.chat_log.push(ChatMessage::user("? port 8000 is already serving the workspace"));

        let prompt = app.user_messages_prompt();
        assert!(prompt.contains("## What The User Told You"));
        assert!(prompt.contains("1. Tab 2 is my SSH session, not a listener"));
        assert!(prompt.contains("2. (latest) port 8000 is already serving the workspace"));
        assert!(!prompt.contains("$ id"), "only messages to the LLM are pinned");
    }

    #[tokio::test]
    async fn new_message_interrupts_running_work() {
        let mut app = App::new().await.expect("App::new failed");
        app.new_shell_tab().await.expect("shell tab");
        let tab_id = app.tabs[0].number;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        app.read_terminal_tab(&json!({"tab_id": tab_id})).expect("initial read");

        let call: ToolCall = serde_json::from_value(json!({
            "id": "call_wait",
            "type": "function",
            "function": {
                "name": "read_terminal_tab",
                "arguments": json!({"tab_id": tab_id, "wait_seconds": 30}).to_string()
            }
        }))
        .expect("tool call");
        app.llm.chat_log.push(ChatMessage {
            tool_calls: Some(vec![call.clone()]),
            ..ChatMessage::assistant("")
        });
        let (task, label) = app.background_tool_task(&call).expect("waits when nothing is new");
        app.start_background_tool(&call, task, label, Vec::new());
        app.llm.request_state = LlmRequestState::ProcessingToolCalls;
        let request_id = app.llm.request_id;

        app.interrupt_for_new_query().await;

        assert!(app.llm.background_tool.is_none());
        assert!(app.llm.abort_handle.is_none());
        assert_eq!(app.llm.request_state, LlmRequestState::Idle);
        assert!(app.llm.request_id > request_id, "late events of the old request are ignored");
        let answer = app.llm.chat_log.last().expect("tool answer");
        assert_eq!(answer.tool_call_id.as_deref(), Some("call_wait"));
        assert!(answer.content.contains("Interrupted"));
        // The interrupted turn stays complete, so it remains in the history.
        assert!(app
            .build_chat_messages()
            .iter()
            .any(|message| message.tool_call_id.as_deref() == Some("call_wait")));

        // Nothing running: a new message changes nothing.
        let count = app.llm.chat_log.len();
        app.interrupt_for_new_query().await;
        assert_eq!(app.llm.chat_log.len(), count);
    }
}
