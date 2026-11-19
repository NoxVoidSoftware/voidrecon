//! Main event loop and keyboard dispatch for voidrecon

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::Backend;
use std::time::Instant;

use crate::app::events::AppEvent;
use crate::app::pty_control::key_to_pty_bytes;
use crate::app::state::{App, InputMode, NotifLevel, WindowFocus};
use crate::llm::{ChatMessage, LlmRequestState};
use crate::ui::render_ui;

impl App {
    pub fn spawn_event_generators(&self) {
        // Spawn tick event generator
        let tx_tick = self.event_tx.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(50));
            loop {
                interval.tick().await;
                if tx_tick.send(AppEvent::Tick).is_err() {
                    break;
                }
            }
        });

        // Spawn crossterm event reader
        let tx_keys = self.event_tx.clone();
        tokio::spawn(async move {
            let mut reader = crossterm::event::EventStream::new();
            use futures_util::StreamExt;
            while let Some(event) = reader.next().await {
                if let Ok(crossterm::event::Event::Key(key)) = event {
                    // Discard Release events to prevent duplicate inputs and flickering on Windows
                    if key.kind != crossterm::event::KeyEventKind::Release
                        && tx_keys.send(AppEvent::Key(key)).is_err()
                    {
                        break;
                    }
                } else if let Ok(crossterm::event::Event::Mouse(mouse)) = event {
                    if tx_keys.send(AppEvent::Mouse(mouse)).is_err() {
                        break;
                    }
                } else if let Ok(crossterm::event::Event::Paste(text)) = event {
                    if tx_keys.send(AppEvent::Paste(text)).is_err() {
                        break;
                    }
                } else if let Ok(crossterm::event::Event::Resize(cols, rows)) = event {
                    if tx_keys.send(AppEvent::Resize(cols, rows)).is_err() {
                        break;
                    }
                }
            }
        });
    }

    pub async fn run<B: Backend>(&mut self, terminal: &mut ratatui::Terminal<B>) -> Result<()> {
        self.spawn_event_generators();

        // Initial render
        terminal.draw(|f| render_ui(f, self))?;
        
        while !self.ui_state.should_quit {
            let Some(first_event) = self.event_rx.recv().await else {
                break;
            };
            
            let mut needs_render = self.handle_event(first_event).await?;
            
            // Drain and batch any already-queued events before drawing
            while let Ok(event) = self.event_rx.try_recv() {
                if self.handle_event(event).await? {
                    needs_render = true;
                }
            }
            
            if needs_render && !self.ui_state.should_quit {
                terminal.draw(|f| render_ui(f, self))?;
            }
        }
        Ok(())
    }
    
    async fn handle_event(&mut self, event: AppEvent) -> Result<bool> {
        match event {
            AppEvent::Key(key) => {
                self.handle_key(key).await?;
                Ok(true)
            }
            AppEvent::Mouse(mouse) => {
                self.handle_mouse(mouse).await?;
                Ok(true)
            }
            AppEvent::Paste(text) => {
                self.handle_paste(text);
                Ok(true)
            }
            AppEvent::PtyData(tab_id, data) => {
                self.handle_pty_data(tab_id, data).await?;
                Ok(true)
            }
            AppEvent::PtyExited(tab_id, exit_code) => {
                self.handle_pty_exited(tab_id, exit_code).await?;
                Ok(true)
            }
            AppEvent::LlmChunk(request_id, chunk) => {
                self.handle_llm_chunk(request_id, chunk);
                Ok(true)
            }
            AppEvent::LlmToolCall(request_id, tool_call) => {
                self.handle_llm_tool_call(request_id, tool_call).await?;
                Ok(true)
            }
            AppEvent::LlmRequestStarted(request_id) => {
                self.handle_llm_request_started(request_id);
                Ok(true)
            }
            AppEvent::LlmRequestFailed(request_id, error) => {
                self.handle_llm_request_failed(request_id, error).await;
                Ok(true)
            }
            AppEvent::LlmDone(request_id, truncated) => {
                self.handle_llm_done(request_id, truncated).await?;
                Ok(true)
            }
            AppEvent::CommandStarted(request_id, cmd) => {
                self.handle_command_started(request_id, cmd);
                Ok(true)
            }
            AppEvent::CommandChunk(request_id, chunk) => {
                self.handle_command_chunk(request_id, chunk);
                Ok(true)
            }
            AppEvent::CommandCompleted(request_id, result) => {
                self.handle_command_completed(request_id, result).await?;
                Ok(true)
            }
            AppEvent::Tick => Ok(self.handle_tick().await),
            AppEvent::Resize(cols, rows) => {
                self.handle_resize(cols, rows).await?;
                Ok(true)
            }
            AppEvent::SidebarReload => {
                self.reload_sidebar().await?;
                self.refresh_exploit_cache().await;
                Ok(true)
            }
            AppEvent::SetActiveTarget(target_id) => {
                self.active_target_id = Some(target_id);
                self.reload_sidebar().await?;
                Ok(true)
            }
            AppEvent::ModelsLoadedForProvider(provider, models) => {
                let count = models.len();
                self.config.update_models_cache_for_provider(provider, models);
                let _ = self.config.save().await;
                self.add_notification(
                    format!("Fetched {} models from {}", count, provider.as_str()),
                    NotifLevel::Info,
                );
                Ok(true)
            }
        }
    }

    pub fn trigger_fetch_models(&self) {
        self.trigger_fetch_models_for_provider(self.config.provider);
    }

    pub fn trigger_fetch_models_for_provider(&self, provider: crate::config::ProviderKind) {
        let tx = self.event_tx.clone();
        let key = self.config.get_api_key(provider);
        let base_url = self.config.base_url.clone();
        let p = crate::llm::providers::Provider::from_config(provider, key, base_url);
        let Ok(client) = crate::llm::LlmClient::new(p) else { return; };
        tokio::spawn(async move {
            if let Ok(models) = client.fetch_models().await {
                if !models.is_empty() {
                    let _ = tx.send(AppEvent::ModelsLoadedForProvider(provider, models));
                }
            }
        });
    }

    /// Routes a key press: open modals and pending approvals take priority,
    /// then global shortcuts, then the active tab.
    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Defensive check: ignore key release events to prevent double inputs and flickering
        if key.kind == crossterm::event::KeyEventKind::Release {
            return Ok(());
        }

        if self.ui_state.show_session_picker {
            return self.handle_session_picker_key(key).await;
        }
        if self.ui_state.show_api_key_prompt {
            return self.handle_api_key_prompt_key(key).await;
        }
        if let Some(approval) = self.pending_exploit_approval.clone() {
            return self.handle_approval_key(key, approval).await;
        }
        if self.ui_state.show_help {
            if self.is_help_key(key) || matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                self.ui_state.show_help = false;
            }
            return Ok(());
        }
        if self.ui_state.show_model_picker {
            return self.handle_model_picker_key(key).await;
        }
        if self.is_help_key(key) {
            self.ui_state.show_help = true;
            return Ok(());
        }

        // While the LLM is generating, Ctrl+C or Esc on the workspace tab cancels
        // the request. In shell tabs these keys belong to the running process.
        let llm_busy = matches!(
            self.llm.request_state,
            LlmRequestState::Streaming | LlmRequestState::ProcessingToolCalls
        );
        let is_cancel_key = key.code == KeyCode::Esc
            || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL));
        if self.active_tab == 0 && llm_busy && is_cancel_key {
            self.cancel_llm_request().await;
            return Ok(());
        }

        if self.handle_global_shortcut(key).await? {
            return Ok(());
        }
        if self.active_tab > 0 {
            self.handle_shell_tab_key(key);
            return Ok(());
        }
        self.handle_workspace_key(key).await
    }

    /// Pasted text goes to whatever currently receives typed input. Multi-line
    /// text is kept intact in the workspace input instead of being submitted
    /// line by line.
    fn handle_paste(&mut self, text: String) {
        if text.is_empty() || self.ui_state.show_help || self.pending_exploit_approval.is_some() {
            return;
        }
        let single_line = || text.lines().map(str::trim).collect::<Vec<_>>().join(" ");
        if self.ui_state.show_session_picker {
            // Session names are capped at 50 bytes, same as typed input.
            let input = &mut self.ui_state.session_name_input;
            for c in single_line().chars() {
                if input.len() + c.len_utf8() > 50 {
                    break;
                }
                input.push(c);
            }
            return;
        }
        if self.ui_state.show_api_key_prompt {
            self.ui_state.api_key_input.push_str(text.trim());
            return;
        }
        if self.ui_state.show_model_picker {
            self.ui_state.model_picker_search.push_str(&single_line());
            self.ui_state.model_picker_selected = 0;
            return;
        }
        if self.active_tab > 0 {
            self.paste_into_shell_tab(&text);
            return;
        }

        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        for c in normalized.chars() {
            if c == '\n' || c == '\t' || !c.is_control() {
                self.insert_input_char(c);
            }
        }
        self.ui_state.focus = WindowFocus::Input;
    }

    /// Writes pasted text to the active PTY, wrapped in bracketed-paste markers
    /// when the program asked for them (so shells don't run each line).
    fn paste_into_shell_tab(&mut self, text: &str) {
        let Some(tab) = self.tabs.get_mut(self.active_tab - 1) else {
            return;
        };
        let bracketed = tab
            .parser
            .lock()
            .is_ok_and(|parser| parser.screen().bracketed_paste());
        let body = text.replace("\r\n", "\r").replace('\n', "\r");
        let mut bytes = Vec::with_capacity(body.len() + 12);
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(body.as_bytes());
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        let _ = tab.write(&bytes);
    }

    /// Shortcuts that work on every tab. Returns `true` when the key was consumed.
    async fn handle_global_shortcut(&mut self, key: KeyEvent) -> Result<bool> {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::CONTROL) => self.ui_state.should_quit = true,
            (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                self.input_mode = InputMode::LlmQuery;
                self.input_buffer.clear();
                self.ui_state.input_cursor = 0;
                self.active_tab = 0; // LLM queries are typed on the Workspace & LLM tab
            }
            (KeyCode::F(2), _) => self.open_model_picker(),
            (KeyCode::F(3), _) => self.new_shell_tab().await?,
            (KeyCode::F(4), _) | (KeyCode::Char('m'), KeyModifiers::ALT) => {
                self.toggle_mouse_capture()?
            }
            (KeyCode::BackTab, _) => self.cycle_tabs(),
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Interactive shell tabs (Tab 1..N): Esc returns to the workspace, every
    /// other key goes to the PTY as raw input.
    fn handle_shell_tab_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            self.active_tab = 0;
            self.input_mode = InputMode::Shell;
            return;
        }
        if let Some(tab) = self.tabs.get_mut(self.active_tab - 1) {
            if let Some(bytes) = key_to_pty_bytes(key) {
                let _ = tab.write(&bytes);
            }
        }
    }

    /// Tab 0 (Workspace & LLM): local shortcuts, then the current input mode.
    async fn handle_workspace_key(&mut self, key: KeyEvent) -> Result<()> {
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                if self.cancel_running_work().await {
                    return Ok(());
                }
            }
            (KeyCode::Esc, _) => {
                self.input_mode = InputMode::Shell;
                self.input_buffer.clear();
                self.ui_state.input_cursor = 0;
                self.llm.suggestion_visible = false;
                self.history_index = None;
                self.ui_state.show_help = false;
                self.ui_state.show_model_picker = false;
                self.ui_state.show_session_picker = false;
                self.ui_state.show_api_key_prompt = false;
                return Ok(());
            }
            (KeyCode::Char('['), _) if self.input_buffer.is_empty() => {
                self.ui_state.sidebar_collapsed = !self.ui_state.sidebar_collapsed;
                return self.update_pty_dimensions().await;
            }
            _ => {}
        }

        match self.input_mode {
            InputMode::Shell => self.handle_shell_key(key).await,
            InputMode::LlmQuery => self.handle_llm_query_key(key).await,
            InputMode::EditCommand => self.handle_edit_command_key(key).await,
        }
    }

    /// Ctrl+C: aborts the running workspace command, or else the pending LLM
    /// request. Returns `false` when there was nothing to cancel.
    async fn cancel_running_work(&mut self) -> bool {
        if let Some(handle) = self.command_run.abort_handle.take() {
            handle.abort();
            self.command_run.running = None;
            self.command_run.stdin_tx = None;
            self.command_run.output.clear();
            self.command_run.raw.clear();
            self.append_chat_message(ChatMessage::system(
                "⚠️ Command cancelled by user (Ctrl+C)",
            ))
            .await;
            self.add_notification("Command cancelled".to_string(), NotifLevel::Warning);
            return true;
        }
        if let Some(handle) = self.llm.abort_handle.take() {
            handle.abort();
            self.llm.request_state = LlmRequestState::Idle;
            self.append_chat_message(ChatMessage::system(
                "⚠️ LLM generation cancelled by user (Ctrl+C)",
            ))
            .await;
            self.add_notification("LLM cancelled".to_string(), NotifLevel::Warning);
            return true;
        }
        false
    }

    fn is_help_key(&self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::F(1) | KeyCode::F(12) => true,
            KeyCode::Char('h') | KeyCode::Char('H') if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => true,
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::CONTROL) && self.input_buffer.is_empty() => true,
            _ => false,
        }
    }

    pub async fn handle_resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.terminal_size = (cols, rows);
        self.update_pty_dimensions().await?;
        Ok(())
    }

    /// Shift+Tab: switches to the next tab, wrapping from the last shell tab
    /// back to the workspace (Tab 0).
    pub fn cycle_tabs(&mut self) {
        let tab_count = self.tabs.len() + 1;
        self.active_tab = (self.active_tab + 1) % tab_count;
        if self.active_tab == 0 {
            self.ui_state.focus = WindowFocus::Input;
        }
    }

    pub fn add_notification(&mut self, message: String, level: NotifLevel) {
        self.notifications.push(crate::app::state::Notification {
            message,
            level,
            created_at: Instant::now(),
        });
        if self.notifications.len() > 3 {
            self.notifications.remove(0);
        }
    }
}
