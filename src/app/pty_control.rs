//! PTY tab lifecycle, sizing, and key translation
use crate::app::state::{App, NotifLevel};
use crate::llm::LlmRequestState;
use crate::pty::tab::PtyTab;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;

/// Width of the expanded sidebar in terminal columns.
const SIDEBAR_WIDTH: u16 = 40;

impl App {
    pub async fn handle_pty_data(&mut self, _tab_id: usize, _data: Vec<u8>) -> Result<()> {
        // Data already processed by reader thread
        // Just ensure UI updates - we could force a redraw here if needed
        Ok(())
    }
    
    /// A PTY process exited: its tab is closed. Interactive command tabs (nc,
    /// ssh, ...) keep their last screen in the chat log so the output survives.
    pub async fn handle_pty_exited(&mut self, tab_id: usize, exit_code: i32) -> Result<()> {
        let Some(tab) = self.tabs.iter().find(|t| t.id == tab_id) else {
            return Ok(());
        };
        let final_screen = (tab.kind == crate::pty::tab::TerminalKind::Command)
            .then(|| tab.screen_text())
            .filter(|text| !text.is_empty());
        let label = self.close_terminal_tab(tab_id)?;
        if let Some(text) = final_screen {
            self.append_chat_message(crate::llm::ChatMessage::output(format!(
                "[{} exited with code {}]\n{}",
                label, exit_code, text
            )))
            .await;
        }
        self.add_notification(format!("{} closed (exit code {})", label, exit_code), NotifLevel::Info);
        Ok(())
    }

    pub fn close_terminal_tab(&mut self, tab_id: usize) -> Result<String> {
        let pos = self
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .ok_or_else(|| anyhow::anyhow!("Terminal tab {} is not open", tab_id))?;
        let label = self.tabs[pos].label.clone();
        self.tabs.remove(pos);
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
        Ok(label)
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

    /// Id of the terminal tab that currently has focus, if any.
    pub fn active_terminal_tab_id(&self) -> Option<usize> {
        self.active_tab
            .checked_sub(1)
            .and_then(|index| self.tabs.get(index))
            .map(|tab| tab.id)
    }

    pub async fn new_shell_tab(&mut self) -> Result<()> {
        let tab = PtyTab::new_shell(
            self.next_tab_id,
            self.event_tx.clone(),
            self.pty_dimensions(),
        )?;
        self.next_tab_id += 1;
        self.tabs.push(tab);
        self.active_tab = self.tabs.len(); // switch focus to the new shell tab (Tab 1..N)
        Ok(())
    }
    
    pub async fn spawn_listener_tab(&mut self, lport: u16) -> Result<()> {
        let tab = PtyTab::new_listener(
            self.next_tab_id,
            lport,
            self.event_tx.clone(),
            self.pty_dimensions(),
        )?;
        self.next_tab_id += 1;
        self.tabs.push(tab);
        self.active_tab = self.tabs.len(); // switch focus to the listener tab
        Ok(())
    }
    
    pub async fn close_current_shell(&mut self) -> Result<()> {
        if self.active_tab > 0 && self.active_tab - 1 < self.tabs.len() {
            self.tabs.remove(self.active_tab - 1);
            if self.active_tab > self.tabs.len() {
                self.active_tab = self.tabs.len();
            }
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

