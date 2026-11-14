//! Mouse handling: capture toggle, focus/tab clicks, and wheel scrolling

use anyhow::Result;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::app::state::{App, NotifLevel, WindowFocus};

/// Lines scrolled in the chat log per wheel step.
const WHEEL_SCROLL_LINES: usize = 3;
/// Rows taken by the suggestion panel and by the approval panel above the input line.
const ACTION_PANEL_HEIGHT: u16 = 5;
/// Tab bar geometry: the workspace tab label, then fixed-width shell tab labels.
const TAB_BAR_ROW: u16 = 1;
const WORKSPACE_TAB_WIDTH: u16 = 24;
const SHELL_TAB_WIDTH: u16 = 16;

impl App {
    pub fn toggle_mouse_capture(&mut self) -> Result<()> {
        self.mouse_capture_enabled = !self.mouse_capture_enabled;
        if self.mouse_capture_enabled {
            crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
            self.add_notification(
                "Mouse Mode ON: Wheel scrolling & tab clicks active (Hold Shift to select text)".to_string(),
                NotifLevel::Info,
            );
        } else {
            crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)?;
            self.add_notification(
                "Mouse Mode OFF: Native terminal text selection & copy enabled".to_string(),
                NotifLevel::Info,
            );
        }
        Ok(())
    }

    pub async fn handle_mouse(&mut self, mouse: MouseEvent) -> Result<()> {
        if !self.mouse_capture_enabled {
            return Ok(());
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left)
            | MouseEventKind::Up(MouseButton::Left)
            | MouseEventKind::Drag(MouseButton::Left) => self.handle_left_button(mouse),
            MouseEventKind::ScrollUp => self.handle_wheel(true),
            MouseEventKind::ScrollDown => self.handle_wheel(false),
            _ => {}
        }
        Ok(())
    }

    fn handle_left_button(&mut self, mouse: MouseEvent) {
        // If any modal is showing, do not change focus
        if self.ui_state.show_model_picker
            || self.ui_state.show_session_picker
            || self.ui_state.show_api_key_prompt
            || self.ui_state.show_help
        {
            return;
        }

        let sidebar_width = self.sidebar_width();

        if mouse.row == TAB_BAR_ROW && mouse.column >= sidebar_width {
            let col_in_tabs = mouse.column - sidebar_width;
            if col_in_tabs < WORKSPACE_TAB_WIDTH {
                self.active_tab = 0;
            } else {
                let shell_tab_idx = ((col_in_tabs - WORKSPACE_TAB_WIDTH) / SHELL_TAB_WIDTH) as usize;
                if shell_tab_idx < self.tabs.len() {
                    self.active_tab = shell_tab_idx + 1;
                }
            }
            return;
        }

        if self.active_tab != 0 {
            return;
        }

        if !self.ui_state.sidebar_collapsed && mouse.column < sidebar_width {
            self.ui_state.focus = WindowFocus::Sidebar;
            let target_idx = (mouse.row.saturating_sub(2) as usize) / 2;
            if let Some(node) = self.sidebar_nodes.get(target_idx) {
                self.active_target_id = Some(node.target.id);
            }
            return;
        }

        // Input line is at row term_rows - 2 (footer is term_rows - 1); the
        // suggestion and approval panels sit directly above it.
        let input_row = self.terminal_size.1.saturating_sub(2);
        let mut panels_height = 0;
        if self.llm.suggestion_visible {
            panels_height += ACTION_PANEL_HEIGHT;
        }
        if self.pending_exploit_approval.is_some() {
            panels_height += ACTION_PANEL_HEIGHT;
        }
        let input_area_start = input_row.saturating_sub(panels_height);

        if mouse.row >= input_area_start {
            // Clicked on input line or action panels -> focus input
            self.ui_state.focus = WindowFocus::Input;
            let prompt_len = 2; // "> " or "? "
            let click_col = mouse.column.saturating_sub(sidebar_width + prompt_len);
            self.set_input_cursor_from_column(click_col as usize);
        } else if mouse.row >= 2 {
            // Clicked inside the chat / log window -> focus chat window
            self.ui_state.focus = WindowFocus::ChatLog;
        }
    }

    /// Wheel scrolling moves the selection in an open picker, otherwise the chat log.
    fn handle_wheel(&mut self, up: bool) {
        let step = |value: usize, amount: usize| {
            if up {
                value.saturating_sub(amount)
            } else {
                value.saturating_add(amount)
            }
        };

        if self.ui_state.show_model_picker {
            self.ui_state.model_picker_selected = step(self.ui_state.model_picker_selected, 1);
        } else if self.ui_state.show_session_picker {
            self.ui_state.session_picker_selected = step(self.ui_state.session_picker_selected, 1);
        } else if self.active_tab == 0 {
            self.ui_state.focus = WindowFocus::ChatLog;
            if up {
                self.ui_state.log_at_bottom = false;
            }
            self.ui_state.log_scroll = step(self.ui_state.log_scroll, WHEEL_SCROLL_LINES);
        }
    }
}
