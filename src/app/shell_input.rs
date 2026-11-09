//! Shell input handling
use crate::app::state::{App, InputMode, NotifLevel};
use crate::llm::ChatMessage;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use anyhow::Result;
use std::path::Path;
use unicode_width::UnicodeWidthChar;

fn input_token_bounds(input: &str, cursor: usize) -> (usize, usize) {
    let cursor = cursor_boundary(input, cursor);
    let start = input[..cursor]
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map_or(0, |(index, character)| index + character.len_utf8());
    let end = input[cursor..]
        .char_indices()
        .find(|(_, character)| character.is_whitespace())
        .map_or(input.len(), |(index, _)| cursor + index);
    (start, end)
}

fn common_prefix(candidates: &[String]) -> &str {
    let Some(first) = candidates.first() else {
        return "";
    };
    let mut prefix_len = first.len();
    for candidate in &candidates[1..] {
        prefix_len = first
            .char_indices()
            .map(|(index, character)| (index, character.len_utf8()))
            .take_while(|(index, length)| {
                *index + *length <= prefix_len
                    && candidate.get(*index..*index + *length)
                        == first.get(*index..*index + *length)
            })
            .map(|(index, length)| index + length)
            .last()
            .unwrap_or(0);
    }
    &first[..prefix_len]
}

fn choose_completion(partial: &str, completions: &[String]) -> Option<String> {
    let first = completions.first()?;
    if completions.len() == 1 {
        return Some(first.clone());
    }

    let prefix = common_prefix(completions);
    if prefix.len() > partial.len() {
        Some(prefix.to_string())
    } else {
        Some(first.clone())
    }
}

fn is_executable(path: &Path) -> Result<bool> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        Ok(true)
    }
}

fn command_completions(prefix: &str) -> Result<Vec<String>> {
    let mut completions = [
        "cd", "echo", "exit", "export", "history", "jobs", "printf", "pwd", "read",
        "source", "test", "type", "umask", "unalias", "unset",
    ]
    .into_iter()
    .filter(|command| command.starts_with(prefix))
    .map(str::to_string)
    .collect::<Vec<_>>();
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let entries = match std::fs::read_dir(directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(prefix) && is_executable(&entry.path())? {
                    completions.push(name);
                }
            }
        }
    }

    completions.sort();
    completions.dedup();
    Ok(completions)
}

fn path_completions(prefix: &str, workspace: &Path) -> Result<Vec<String>> {
    let (directory_prefix, name_prefix) = prefix
        .rsplit_once('/')
        .map_or(("", prefix), |(directory, name)| {
            (if directory.is_empty() { "/" } else { directory }, name)
        });
    let directory = if directory_prefix.is_empty() {
        workspace.to_path_buf()
    } else {
        workspace.join(directory_prefix)
    };

    let mut completions = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(name_prefix) {
            continue;
        }

        let mut completion = format!("{directory_prefix}{name}");
        if entry.path().is_dir() {
            completion.push('/');
        }
        completions.push(completion);
    }

    completions.sort();
    Ok(completions)
}

fn cursor_boundary(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    if cursor == text.len() {
        return cursor;
    }

    text.char_indices()
        .take_while(|(index, _)| *index <= cursor)
        .last()
        .map_or(0, |(index, _)| index)
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    let cursor = cursor_boundary(text, cursor);
    text[..cursor]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    let cursor = cursor_boundary(text, cursor);
    cursor
        + text[cursor..]
            .chars()
            .next()
            .map_or(0, char::len_utf8)
}

impl App {
    pub fn insert_input_char(&mut self, c: char) {
        let cursor = cursor_boundary(&self.input_buffer, self.ui_state.input_cursor);
        self.input_buffer.insert(cursor, c);
        self.ui_state.input_cursor = cursor + c.len_utf8();
    }

    pub fn delete_char_before_cursor(&mut self) {
        let cursor = cursor_boundary(&self.input_buffer, self.ui_state.input_cursor);
        if cursor > 0 {
            let previous = previous_char_boundary(&self.input_buffer, cursor);
            self.input_buffer.remove(previous);
            self.ui_state.input_cursor = previous;
        }
    }

    pub fn delete_char_at_cursor(&mut self) {
        let cursor = cursor_boundary(&self.input_buffer, self.ui_state.input_cursor);
        if cursor < self.input_buffer.len() {
            self.input_buffer.remove(cursor);
            self.ui_state.input_cursor = cursor;
        }
    }

    pub fn cursor_left(&mut self) {
        self.ui_state.input_cursor =
            previous_char_boundary(&self.input_buffer, self.ui_state.input_cursor);
    }

    pub fn cursor_right(&mut self) {
        self.ui_state.input_cursor =
            next_char_boundary(&self.input_buffer, self.ui_state.input_cursor);
    }

    pub fn cursor_home(&mut self) {
        self.ui_state.input_cursor = 0;
    }

    pub fn cursor_end(&mut self) {
        self.ui_state.input_cursor = self.input_buffer.len();
    }

    pub fn set_input_cursor_from_column(&mut self, column: usize) {
        let mut display_column = 0;
        for (byte_index, character) in self.input_buffer.char_indices() {
            let width = UnicodeWidthChar::width(character).unwrap_or(0);
            if column < display_column + width {
                self.ui_state.input_cursor = byte_index;
                return;
            }

            display_column += width;
            if column == display_column {
                self.ui_state.input_cursor = byte_index + character.len_utf8();
                return;
            }
        }

        self.ui_state.input_cursor = self.input_buffer.len();
    }

    pub fn complete_input(&mut self) -> Result<()> {
        let cursor = cursor_boundary(&self.input_buffer, self.ui_state.input_cursor);
        let (start, end) = input_token_bounds(&self.input_buffer, cursor);
        let token = &self.input_buffer[start..cursor];
        if token.is_empty() {
            return Ok(());
        }
        let is_command = self.input_buffer[..start].trim().is_empty()
            && !token.contains('/')
            && !token.starts_with('.')
            && !token.starts_with('~');
        let completions = if is_command {
            if token.starts_with(':') {
                Ok([
                    ":target",
                    ":targets",
                    ":findings",
                    ":note",
                    ":models",
                    ":provider",
                    ":sessions",
                    ":mouse",
                    ":newtab",
                    ":closeshell",
                    ":exploit",
                    ":help",
                    ":quit",
                ]
                .into_iter()
                .filter(|command| command.starts_with(token))
                .map(str::to_string)
                .collect())
            } else {
                command_completions(token)
            }
        } else {
            path_completions(token, &self.session_meta.workspace_path())
        };

        let completions = match completions {
            Ok(completions) => completions,
            Err(error) => {
                self.add_notification(
                    format!("Autocomplete failed: {error}"),
                    NotifLevel::Warning,
                );
                return Ok(());
            }
        };
        if completions.is_empty() {
            return Ok(());
        }

        let partial = &self.input_buffer[start..cursor];
        let Some(completion) = choose_completion(partial, &completions) else {
            return Ok(());
        };

        let append_space = completions.len() == 1
            && !completion.ends_with('/')
            && end == self.input_buffer.len();
        self.input_buffer.replace_range(start..end, &completion);
        let mut cursor = start + completion.len();
        if append_space {
            self.input_buffer.insert(cursor, ' ');
            cursor += 1;
        }
        self.ui_state.input_cursor = cursor;
        self.ui_state.focus = crate::app::state::WindowFocus::Input;
        Ok(())
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
            KeyCode::Tab => {
                self.complete_input()?;
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
            KeyCode::Tab => {
                self.complete_input()?;
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

#[cfg(test)]
mod tests {
    use super::{
        choose_completion, common_prefix, cursor_boundary, input_token_bounds, next_char_boundary,
        previous_char_boundary,
    };

    #[test]
    fn cursor_movements_stay_on_utf8_boundaries() {
        let text = "aЖ界";
        assert_eq!(next_char_boundary(text, 1), 3);
        assert_eq!(next_char_boundary(text, 3), 6);
        assert_eq!(previous_char_boundary(text, 6), 3);
        assert_eq!(previous_char_boundary(text, 3), 1);
        assert_eq!(cursor_boundary(text, 2), 1);
    }

    #[test]
    fn deleting_non_ascii_characters_uses_character_boundaries() {
        let mut text = String::from("aЖ界");
        let mut cursor = text.len();

        let previous = previous_char_boundary(&text, cursor);
        text.remove(previous);
        cursor = previous;
        assert_eq!(text, "aЖ");
        assert_eq!(cursor, 3);

        let previous = previous_char_boundary(&text, cursor);
        text.remove(previous);
        assert_eq!(text, "a");
    }

    #[test]
    fn completion_prefix_and_token_bounds_support_unicode() {
        let candidates = vec!["café-au-lait".to_string(), "café-noir".to_string()];
        assert_eq!(common_prefix(&candidates), "café-");

        let input = "echo café au";
        let cursor = "echo café".len();
        assert_eq!(input_token_bounds(input, cursor), (5, 10));
    }

    #[test]
    fn ambiguous_completion_is_inserted_instead_of_only_notified() {
        let candidates = vec!["pwd".to_string(), "python".to_string()];
        assert_eq!(choose_completion("p", &candidates).as_deref(), Some("pwd"));

        let candidates = vec!["cargo".to_string(), "cat".to_string()];
        assert_eq!(choose_completion("c", &candidates).as_deref(), Some("ca"));
    }
}
