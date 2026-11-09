//! Shell input handling
use crate::app::state::{App, InputMode, NotifLevel};
use crate::llm::ChatMessage;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use anyhow::Result;

impl App {
    pub fn insert_input_char(&mut self, c: char) {
        if self.ui_state.input_cursor >= self.input_buffer.len() {
            self.input_buffer.push(c);
            self.ui_state.input_cursor = self.input_buffer.len();
        } else {
            self.input_buffer.insert(self.ui_state.input_cursor, c);
            self.ui_state.input_cursor += 1;
        }
    }

    pub fn delete_char_before_cursor(&mut self) {
        if self.ui_state.input_cursor > 0 && !self.input_buffer.is_empty() {
            self.ui_state.input_cursor -= 1;
            if self.ui_state.input_cursor < self.input_buffer.len() {
                self.input_buffer.remove(self.ui_state.input_cursor);
            }
        }
    }

    pub fn delete_char_at_cursor(&mut self) {
        if self.ui_state.input_cursor < self.input_buffer.len() {
            self.input_buffer.remove(self.ui_state.input_cursor);
        }
    }

    pub fn cursor_left(&mut self) {
        if self.ui_state.input_cursor > 0 {
            self.ui_state.input_cursor -= 1;
        }
    }

    pub fn cursor_right(&mut self) {
        if self.ui_state.input_cursor < self.input_buffer.len() {
            self.ui_state.input_cursor += 1;
        }
    }

    pub fn cursor_home(&mut self) {
        self.ui_state.input_cursor = 0;
    }

    pub fn cursor_end(&mut self) {
        self.ui_state.input_cursor = self.input_buffer.len();
    }

    pub async fn handle_shell_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Enter => {
                let cmd = self.input_buffer.clone();
                self.input_buffer.clear();
                self.ui_state.input_cursor = 0;

                // If a command is running in workspace tab, forward input to its stdin
                if self.command_run.running.is_some() && self.active_tab == 0 {
                    if let Some(tx) = &self.command_run.stdin_tx {
                        let _ = tx.send(format!("{}\n", cmd));
                        self.append_chat_message(ChatMessage {
                            role: "user".to_string(),
                            content: if cmd.trim().is_empty() {
                                "[stdin] > <Enter>".to_string()
                            } else {
                                format!("[stdin] > {}", cmd.trim())
                            },
                            tool_calls: None,
                            tool_call_id: None,
                        });
                        self.add_notification("Sent input to running process".to_string(), NotifLevel::Info);
                    } else {
                        self.add_notification("Command is running (stdin closed). Press Ctrl+C to cancel.".to_string(), NotifLevel::Warning);
                    }
                    return Ok(());
                }

                let trimmed = cmd.trim().to_string();
                if !trimmed.is_empty() {
                    self.command_history.push_back(trimmed.clone());
                    self.history_index = None;
                    
                    // Handle special commands
                    if trimmed.starts_with(':') {
                        self.handle_special_command(&trimmed).await?;
                    } else if trimmed.starts_with('?') {
                        let query = trimmed.trim_start_matches('?').trim().to_string();
                        if !query.is_empty() {
                            self.send_llm_query(&query).await?;
                        }
                    } else if self.active_tab == 0 {
                        // Tab 0: execute as shell command in workspace
                        self.execute_workspace_command(&trimmed).await?;
                    } else {
                        // Other tabs: send to PTY
                        self.send_to_pty(&trimmed).await?;
                    }
                }
            }
            KeyCode::Char(c) => {
                self.ui_state.focus = crate::app::state::WindowFocus::Input;
                if self.llm.suggestion_visible && self.input_buffer.is_empty() {
                    match c {
                        'r' => {
                            if let Some(sug) = self.llm.suggestion.take() {
                                self.llm.suggestion_visible = false;
                                if let Some(cmd) = sug.command {
                                    if self.active_tab == 0 {
                                        self.execute_workspace_command(&cmd).await?;
                                    } else {
                                        self.send_to_pty(&cmd).await?;
                                    }
                                }
                            }
                            return Ok(());
                        }
                        'e' => {
                            if let Some(sug) = &self.llm.suggestion {
                                if let Some(cmd) = &sug.command {
                                    self.input_buffer = cmd.clone();
                                    self.ui_state.input_cursor = self.input_buffer.len();
                                    self.input_mode = InputMode::EditCommand;
                                }
                            }
                            self.llm.suggestion_visible = false;
                            return Ok(());
                        }
                        's' => {
                            self.llm.suggestion_visible = false;
                            return Ok(());
                        }
                        _ => {}
                    }
                }
                if self.input_buffer.is_empty() && c == '?' {
                    self.input_mode = InputMode::LlmQuery;
                    self.ui_state.input_cursor = 0;
                    return Ok(());
                }
                self.insert_input_char(c);
            }
            KeyCode::Backspace => {
                self.ui_state.focus = crate::app::state::WindowFocus::Input;
                self.delete_char_before_cursor();
            }
            KeyCode::Delete => {
                self.ui_state.focus = crate::app::state::WindowFocus::Input;
                self.delete_char_at_cursor();
            }
            KeyCode::Left => {
                self.ui_state.focus = crate::app::state::WindowFocus::Input;
                self.cursor_left();
            }
            KeyCode::Right => {
                self.ui_state.focus = crate::app::state::WindowFocus::Input;
                self.cursor_right();
            }
            KeyCode::Home => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.ui_state.log_at_bottom = false;
                    self.ui_state.log_scroll = 0;
                } else {
                    self.cursor_home();
                }
            }
            KeyCode::End => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.ui_state.log_at_bottom = true;
                } else {
                    self.cursor_end();
                }
            }
            KeyCode::Up => {
                if self.ui_state.focus == crate::app::state::WindowFocus::ChatLog {
                    self.ui_state.log_at_bottom = false;
                    self.ui_state.log_scroll = self.ui_state.log_scroll.saturating_sub(1);
                    return Ok(());
                }
                if let Some(idx) = self.history_index {
                    if idx > 0 {
                        self.history_index = Some(idx - 1);
                        self.input_buffer = self.command_history[idx - 1].clone();
                        self.ui_state.input_cursor = self.input_buffer.len();
                    }
                } else if !self.command_history.is_empty() {
                    self.history_index = Some(self.command_history.len() - 1);
                    if let Some(last) = self.command_history.back() {
                        self.input_buffer = last.clone();
                    }
                    self.ui_state.input_cursor = self.input_buffer.len();
                } else if self.input_buffer.is_empty() {
                    // When no command history and empty input, Up scrolls chat log up by 1 line
                    self.ui_state.log_at_bottom = false;
                    self.ui_state.log_scroll = self.ui_state.log_scroll.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if self.ui_state.focus == crate::app::state::WindowFocus::ChatLog {
                    self.ui_state.log_scroll = self.ui_state.log_scroll.saturating_add(1);
                    return Ok(());
                }
                if let Some(idx) = self.history_index {
                    if idx + 1 < self.command_history.len() {
                        self.history_index = Some(idx + 1);
                        self.input_buffer = self.command_history[idx + 1].clone();
                        self.ui_state.input_cursor = self.input_buffer.len();
                    } else {
                        self.history_index = None;
                        self.input_buffer.clear();
                        self.ui_state.input_cursor = 0;
                    }
                } else if self.input_buffer.is_empty() {
                    // When no command history and empty input, Down scrolls chat log down by 1 line
                    self.ui_state.log_scroll = self.ui_state.log_scroll.saturating_add(1);
                }
            }
            KeyCode::PageUp => {
                self.ui_state.log_at_bottom = false;
                self.ui_state.log_scroll = self.ui_state.log_scroll.saturating_sub(10);
            }
            KeyCode::PageDown => {
                self.ui_state.log_scroll = self.ui_state.log_scroll.saturating_add(10);
            }
            _ => {}
        }
        Ok(())
    }
    
    pub async fn handle_llm_query_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Enter => {
                let query = self.input_buffer.trim().to_string();
                self.input_buffer.clear();
                self.ui_state.input_cursor = 0;
                self.input_mode = InputMode::Shell;
                if !query.is_empty() {
                    self.send_llm_query(&query).await?;
                }
            }
            KeyCode::Char(c) => {
                self.insert_input_char(c);
            }
            KeyCode::Backspace => {
                self.ui_state.focus = crate::app::state::WindowFocus::Input;
                self.delete_char_before_cursor();
            }
            KeyCode::Delete => {
                self.ui_state.focus = crate::app::state::WindowFocus::Input;
                self.delete_char_at_cursor();
            }
            KeyCode::Left => {
                self.ui_state.focus = crate::app::state::WindowFocus::Input;
                self.cursor_left();
            }
            KeyCode::Right => {
                self.ui_state.focus = crate::app::state::WindowFocus::Input;
                self.cursor_right();
            }
            KeyCode::Home => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.ui_state.log_at_bottom = false;
                    self.ui_state.log_scroll = 0;
                } else {
                    self.cursor_home();
                }
            }
            KeyCode::End => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.ui_state.log_at_bottom = true;
                } else {
                    self.cursor_end();
                }
            }
            KeyCode::PageUp => {
                self.ui_state.log_at_bottom = false;
                self.ui_state.log_scroll = self.ui_state.log_scroll.saturating_sub(10);
            }
            KeyCode::PageDown => {
                self.ui_state.log_scroll = self.ui_state.log_scroll.saturating_add(10);
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Shell;
                self.input_buffer.clear();
                self.ui_state.input_cursor = 0;
            }
            _ => {}
        }
        Ok(())
    }
    
    pub async fn handle_edit_command_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Enter => {
                let cmd = self.input_buffer.trim().to_string();
                self.input_buffer.clear();
                self.ui_state.input_cursor = 0;
                self.input_mode = InputMode::Shell;
                self.llm.suggestion_visible = false;
                if !cmd.is_empty() {
                    self.command_history.push_back(cmd.clone());
                    if self.active_tab == 0 {
                        self.execute_workspace_command(&cmd).await?;
                    } else {
                        self.send_to_pty(&cmd).await?;
                    }
                }
            }
            KeyCode::Char(c) => {
                self.insert_input_char(c);
            }
            KeyCode::Backspace => {
                self.ui_state.focus = crate::app::state::WindowFocus::Input;
                self.delete_char_before_cursor();
            }
            KeyCode::Delete => {
                self.ui_state.focus = crate::app::state::WindowFocus::Input;
                self.delete_char_at_cursor();
            }
            KeyCode::Left => {
                self.ui_state.focus = crate::app::state::WindowFocus::Input;
                self.cursor_left();
            }
            KeyCode::Right => {
                self.ui_state.focus = crate::app::state::WindowFocus::Input;
                self.cursor_right();
            }
            KeyCode::Home => {
                self.cursor_home();
            }
            KeyCode::End => {
                self.cursor_end();
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Shell;
                self.input_buffer.clear();
                self.ui_state.input_cursor = 0;
                self.llm.suggestion_visible = false;
            }
            _ => {}
        }
        Ok(())
    }
    

}
