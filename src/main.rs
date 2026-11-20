//! Main entry point for voidrecon - Production-grade interactive TUI terminal application
//! for security auditing, CLI workflows, and LLM-assisted terminal operations.

mod app;
mod config;
mod db;
mod llm;
mod pty;
mod tools;
mod ui;
mod web;

use anyhow::Result;
use app::App;
use crossterm::{
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use std::io::stdout;

/// RAII guard to ensure terminal is restored on panic or normal exit
struct TermGuard;

impl Drop for TermGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install panic hook for clean shutdown
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen);
        original_hook(panic_info);
    }));

    let _guard = TermGuard;

    // Setup terminal
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    // Bracketed paste delivers multi-line pastes as one event instead of a
    // burst of key presses (where each newline would submit the input).
    let _ = execute!(stdout(), EnableBracketedPaste);
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;

    // Create and run app
    let mut app = App::new().await?;
    app.run(&mut terminal).await?;

    Ok(())
}