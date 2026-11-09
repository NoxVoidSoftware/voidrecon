//! PTY control
use crate::app::state::{App, NotifLevel};
use crate::llm::LlmRequestState;
use anyhow::Result;
use std::time::Instant;

impl App {
    pub async fn handle_pty_data(&mut self, _tab_id: usize, _data: Vec<u8>) -> Result<()> {
        // Data already processed by reader thread
        // Just ensure UI updates - we could force a redraw here if needed
        Ok(())
    }
    
    pub async fn handle_pty_exited(&mut self, tab_id: usize, exit_code: i32) -> Result<()> {
        if let Some(pos) = self.tabs.iter().position(|t| t.id == tab_id) {
            let label = self.tabs[pos].label.clone();
            self.tabs.remove(pos);
            let tab_index = pos + 1;
            if self.active_tab == tab_index {
                // If on the closed tab, switch to previous tab or Tab 0 (Workspace & LLM)
                self.active_tab = pos;
            } else if self.active_tab > tab_index {
                self.active_tab -= 1;
            }
            if self.active_tab > self.tabs.len() {
                self.active_tab = self.tabs.len();
            }
            self.add_notification(format!("{} closed (exit code {})", label, exit_code), NotifLevel::Info);
        }
        Ok(())
    }
    
    pub fn handle_tick(&mut self) -> bool {
        let now = Instant::now();
        let had_notifications = !self.notifications.is_empty();
        self.notifications.retain(|n| now.duration_since(n.created_at).as_secs() < 5);
        if self.notifications.len() > 3 {
            self.notifications.drain(0..self.notifications.len() - 3);
        }
        let has_notifications = !self.notifications.is_empty();
        self.last_tick = now;

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
        let (term_cols, term_rows) = self.terminal_size;
        let sidebar_width = if self.ui_state.sidebar_collapsed { 0 } else { 40 };
        let pty_cols = term_cols.saturating_sub(sidebar_width).max(20);
        let pty_rows = term_rows.saturating_sub(3).max(5);

        for tab in &mut self.tabs {
            let _ = tab.resize(pty_cols, pty_rows);
        }
        Ok(())
    }


}
