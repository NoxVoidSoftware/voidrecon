//! Application events for voidrecon

use crossterm::event::{KeyEvent, MouseEvent};
use crate::llm::types::ToolCall;

#[derive(Debug, Clone)]
pub struct CommandResult {
    pub cmd: String,
    pub output: String,
    pub exit_code: i32,
    pub duration_ms: i64,
}

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    PtyData(usize, Vec<u8>),
    PtyExited(usize, i32),
    LlmChunk(u64, String),
    LlmToolCall(u64, ToolCall),
    LlmRequestStarted(u64),
    LlmRequestFailed(u64, String),
    LlmDone(u64),
    CommandStarted(u64, String),
    CommandCompleted(u64, CommandResult),
    Tick,
    Resize(u16, u16),
    SidebarReload,
    SetActiveTarget(i64),
    ModelsLoadedForProvider(crate::config::ProviderKind, Vec<crate::config::ModelInfo>),
}
