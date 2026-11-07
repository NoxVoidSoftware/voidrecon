//! PTY module for voidrecon
//! Handles PTY spawn, resize, and vt100 screen widget for ratatui rendering.

pub mod tab;
pub mod widget;

pub use tab::*;
pub use widget::*;