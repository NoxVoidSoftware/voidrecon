//! Session picker key handling
use crate::app::state::{App, NotifLevel};
use crossterm::event::{KeyCode, KeyEvent};
use anyhow::Result;

impl App {
    pub async fn handle_session_picker_key(&mut self, key: KeyEvent) -> Result<()> {
        if let Some(to_delete) = self.ui_state.session_delete_confirm.clone() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.delete_session(&to_delete).await?;
                    self.ui_state.session_delete_confirm = None;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.ui_state.session_delete_confirm = None;
                }
                _ => {}
            }
            return Ok(());
        }

        if self.ui_state.session_naming_mode {
            match key.code {
                KeyCode::Esc => {
                    self.ui_state.session_naming_mode = false;
                    self.ui_state.session_name_input.clear();
                }
                KeyCode::Enter => {
                    let raw = self.ui_state.session_name_input.trim();
                    let session_name = if raw.is_empty() {
                        format!("{}_scan", chrono::Local::now().format("%Y-%m-%d_%H%M%S"))
                    } else {
                        // Sanitize filename: replace invalid chars with '_'
                        raw.chars().map(|c| {
                            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                                c
                            } else {
                                '_'
                            }
                        }).collect::<String>()
                    };

                    self.switch_session(session_name).await?;
                    self.ui_state.session_naming_mode = false;
                    self.ui_state.session_name_input.clear();
                    self.ui_state.show_session_picker = false;
                    self.ui_state.is_initial_session_select = false;
                }
                KeyCode::Backspace => {
                    self.ui_state.session_name_input.pop();
                }
                KeyCode::Char(c) => {
                    if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' || c == ' ' {
                        let c = if c == ' ' { '-' } else { c };
                        if self.ui_state.session_name_input.len() < 50 {
                            self.ui_state.session_name_input.push(c);
                        }
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        let total_rows = 1 + self.ui_state.session_list.len();
        match key.code {
            KeyCode::Esc => {
                if self.ui_state.is_initial_session_select {
                    self.ui_state.should_quit = true;
                } else {
                    self.ui_state.show_session_picker = false;
                }
            }
            KeyCode::Up => {
                if self.ui_state.session_picker_selected > 0 {
                    self.ui_state.session_picker_selected -= 1;
                }
            }
            KeyCode::Down => {
                if self.ui_state.session_picker_selected + 1 < total_rows {
                    self.ui_state.session_picker_selected += 1;
                }
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                let selected = self.ui_state.session_picker_selected;
                if selected == 0 {
                    self.add_notification("Cannot delete '+ Create New Session' option".to_string(), NotifLevel::Warning);
                } else if let Some(s) = self.ui_state.session_list.get(selected - 1) {
                    self.ui_state.session_delete_confirm = Some(s.name.clone());
                }
            }
            KeyCode::Enter => {
                let selected = self.ui_state.session_picker_selected;
                if selected == 0 {
                    self.ui_state.session_naming_mode = true;
                    self.ui_state.session_name_input.clear();
                } else if let Some(s) = self.ui_state.session_list.get(selected - 1) {
                    let name = s.name.clone();
                    self.switch_session(name).await?;
                    self.ui_state.show_session_picker = false;
                    self.ui_state.is_initial_session_select = false;
                }
            }
            _ => {}
        }
        Ok(())
    }


}
