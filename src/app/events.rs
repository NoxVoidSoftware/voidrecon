//! Application events for voidrecon

use crossterm::event::KeyEvent;
use crate::llm::types::ToolCall;

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    PtyData(usize, Vec<u8>),
    PtyExited(usize, i32),
    LlmChunk(u64, String),
    LlmToolCall(u64, ToolCall),
    LlmRequestStarted(u64),
    LlmRequestFailed(u64, String),
    LlmDone(u64),
    Tick,
    Resize(u16, u16),
    SidebarReload,
    ModelsLoadedForProvider(crate::config::ProviderKind, Vec<crate::config::ModelInfo>),
}