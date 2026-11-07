//! Event handlers for voidrecon

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::Backend;
use std::time::Instant;

use crate::app::events::AppEvent;
use crate::app::state::{App, InputMode, NotifLevel, SidebarNode};
use crate::db::DatabaseQueries;
use crate::db::models::Finding;
use crate::llm::{
    ChatMessage, LlmProvider, LlmRequestState, build_system_prompt, get_tool_definitions,
    parse_llm_response, POST_EXEC_PROMPT,
};
use crate::llm::types::ToolCall;
use crate::pty::tab::PtyTab;
use crate::ui::render_ui;

impl App {
    pub async fn run<B: Backend>(&mut self, terminal: &mut ratatui::Terminal<B>) -> Result<()> {
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
                self.handle_llm_request_failed(request_id, error);
                Ok(true)
            }
            AppEvent::LlmDone(request_id) => {
                self.handle_llm_done(request_id).await?;
                Ok(true)
            }
            AppEvent::Tick => Ok(self.handle_tick()),
            AppEvent::Resize(cols, rows) => {
                self.handle_resize(cols, rows).await?;
                Ok(true)
            }
            AppEvent::SidebarReload => {
                self.reload_sidebar().await?;
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
                let _ = self.config.save();
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
        let p = crate::llm::provider::Provider::from_config(provider, key, base_url);
        let client = crate::llm::LlmClient::new(p);
        tokio::spawn(async move {
            if let Ok(models) = client.fetch_models().await {
                if !models.is_empty() {
                    let _ = tx.send(AppEvent::ModelsLoadedForProvider(provider, models));
                }
            }
        });
    }
    
    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Defensive check: ignore key release events to prevent double inputs and flickering
        if key.kind == crossterm::event::KeyEventKind::Release {
            return Ok(());
        }

        // If Session Picker modal is open, handle session picker keys first
        if self.ui_state.show_session_picker {
            self.handle_session_picker_key(key).await?;
            return Ok(());
        }

        // If Provider & API key setup modal is open, intercept keys
        if self.ui_state.show_api_key_prompt {
            let providers = crate::config::ALL_PROVIDERS;
            let total_providers = providers.len();
            match key.code {
                KeyCode::Esc => {
                    self.ui_state.show_api_key_prompt = false;
                    self.ui_state.api_key_input.clear();
                    self.add_notification(
                        "Provider configuration closed".to_string(),
                        NotifLevel::Info,
                    );
                }
                KeyCode::Tab | KeyCode::Right => {
                    self.ui_state.provider_picker_selected = (self.ui_state.provider_picker_selected + 1) % total_providers;
                    self.ui_state.api_key_input.clear();
                }
                KeyCode::BackTab | KeyCode::Left => {
                    if self.ui_state.provider_picker_selected == 0 {
                        self.ui_state.provider_picker_selected = total_providers - 1;
                    } else {
                        self.ui_state.provider_picker_selected -= 1;
                    }
                    self.ui_state.api_key_input.clear();
                }
                KeyCode::Enter => {
                    let selected_provider = providers[self.ui_state.provider_picker_selected.min(total_providers - 1)];
                    let key_val = self.ui_state.api_key_input.trim().to_string();

                    if !key_val.is_empty() {
                        self.config.set_api_key(selected_provider, key_val);
                        self.add_notification(
                            format!("Saved key for {} and activated", selected_provider.as_str()),
                            NotifLevel::Info,
                        );
                    } else if self.config.has_api_key(selected_provider) {
                        self.add_notification(
                            format!("Activated {} (using saved key)", selected_provider.as_str()),
                            NotifLevel::Info,
                        );
                    } else if !selected_provider.requires_api_key() {
                        self.add_notification(
                            format!("Activated {}", selected_provider.as_str()),
                            NotifLevel::Info,
                        );
                    } else {
                        self.add_notification(
                            format!("No API key entered for {}", selected_provider.as_str()),
                            NotifLevel::Warning,
                        );
                        return Ok(());
                    }

                    self.config.set_provider(selected_provider, None);
                    let _ = self.config.save();
                    self.llm_client = crate::llm::LlmClient::from_config(&self.config);
                    self.ui_state.show_api_key_prompt = false;
                    self.ui_state.api_key_input.clear();
                    if self.config.models_cache.is_empty() {
                        self.trigger_fetch_models();
                    }
                }
                KeyCode::Char(c) => {
                    self.ui_state.api_key_input.push(c);
                }
                KeyCode::Backspace => {
                    self.ui_state.api_key_input.pop();
                }
                _ => {}
            }
            return Ok(());
        }

        // If Exploit Approval is pending, intercept y/n/Enter/Esc
        if let Some(approval) = self.pending_exploit_approval.clone() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.pending_exploit_approval = None;
                    match &approval.kind {
                        crate::app::state::ApprovalKind::ReverseShellListener { lhost, lport, shell_type } => {
                            match self.spawn_listener_tab(*lport).await {
                                Ok(()) => {
                                    self.append_chat_message(ChatMessage {
                                        role: "tool".to_string(),
                                        content: serde_json::json!({
                                            "status": "approved",
                                            "message": format!("Listener spawned on port {}", lport),
                                            "target_id": approval.target_id,
                                            "lhost": lhost,
                                            "lport": lport,
                                            "shell_type": shell_type,
                                        }).to_string(),
                                        tool_calls: None,
                                        tool_call_id: Some(approval.tool_call.id),
                                    });
                                    self.add_notification(
                                        format!("Spawned listener on port {}", lport),
                                        NotifLevel::Info,
                                    );
                                }
                                Err(e) => {
                                    self.append_chat_message(ChatMessage {
                                        role: "tool".to_string(),
                                        content: serde_json::json!({
                                            "status": "error",
                                            "message": format!("Failed to spawn listener on port {}: {}", lport, e),
                                        }).to_string(),
                                        tool_calls: None,
                                        tool_call_id: Some(approval.tool_call.id),
                                    });
                                    self.add_notification(
                                        format!("Failed to spawn listener: {}", e),
                                        NotifLevel::Error,
                                    );
                                }
                            }
                        }
                        crate::app::state::ApprovalKind::ExecuteExploit { script_name, command_line } => {
                            let start = Instant::now();
                            self.append_chat_message(ChatMessage {
                                role: "user".to_string(),
                                content: format!("$ {}", command_line),
                                tool_calls: None,
                                tool_call_id: None,
                            });

                            let output_result = self.run_command_capture(command_line).await;
                            let duration_ms = start.elapsed().as_millis() as i64;

                            match output_result {
                                Ok(out) => {
                                    let lines: Vec<&str> = out.lines().collect();
                                    self.append_chat_message(ChatMessage {
                                        role: "output".to_string(),
                                        content: out.clone(),
                                        tool_calls: None,
                                        tool_call_id: None,
                                    });

                                    // Record command and output artifact in database
                                    let _ = DatabaseQueries::add_command(
                                        self.db.conn(),
                                        &self.session_meta.path(),
                                        Some(approval.target_id),
                                        command_line,
                                        Some(&out),
                                        Some(0),
                                        Some(duration_ms),
                                    );
                                    let _ = DatabaseQueries::add_chat_message(self.db.conn(), Some(approval.target_id), "user", &format!("$ {}", command_line));
                                    let _ = DatabaseQueries::add_chat_message(self.db.conn(), Some(approval.target_id), "assistant", &out);

                                    let tool_out = if lines.len() > 30 {
                                        let top = lines[..25].join("\n");
                                        format!("{}\n... [truncated {} lines, full output displayed above] ...", top, lines.len() - 25)
                                    } else {
                                        out.clone()
                                    };

                                    self.append_chat_message(ChatMessage {
                                        role: "tool".to_string(),
                                        content: serde_json::json!({
                                            "status": "executed",
                                            "script": script_name,
                                            "exit_code": 0,
                                            "duration_ms": duration_ms,
                                            "output": tool_out
                                        }).to_string(),
                                        tool_calls: None,
                                        tool_call_id: Some(approval.tool_call.id),
                                    });
                                    self.add_notification(format!("Exploit executed ({})", script_name), NotifLevel::Info);
                                }
                                Err(e) => {
                                    self.append_chat_message(ChatMessage {
                                        role: "tool".to_string(),
                                        content: serde_json::json!({
                                            "status": "execution_failed",
                                            "script": script_name,
                                            "error": e.to_string()
                                        }).to_string(),
                                        tool_calls: None,
                                        tool_call_id: Some(approval.tool_call.id),
                                    });
                                    self.add_notification(format!("Exploit failed: {}", e), NotifLevel::Error);
                                }
                            }
                            self.ui_state.log_at_bottom = true;
                        }
                    }
                    self.continue_llm_conversation().await?;
                    return Ok(());
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.pending_exploit_approval = None;
                    let reject_msg = match &approval.kind {
                        crate::app::state::ApprovalKind::ReverseShellListener { .. } => "User rejected spawning reverse shell listener",
                        crate::app::state::ApprovalKind::ExecuteExploit { .. } => "User rejected exploit script execution",
                    };
                    self.append_chat_message(ChatMessage {
                        role: "tool".to_string(),
                        content: serde_json::json!({
                            "status": "rejected",
                            "message": reject_msg
                        }).to_string(),
                        tool_calls: None,
                        tool_call_id: Some(approval.tool_call.id),
                    });
                    self.add_notification("Action rejected".to_string(), NotifLevel::Info);
                    self.continue_llm_conversation().await?;
                    return Ok(());
                }
                KeyCode::Char('?') => {
                    self.pending_exploit_approval = None;
                    self.input_mode = InputMode::LlmQuery;
                    self.input_buffer.clear();
                    self.ui_state.input_cursor = 0;
                    self.add_notification("Action cancelled - entering LLM query mode".to_string(), NotifLevel::Info);
                    return Ok(());
                }
                _ => {
                    self.add_notification("Action pending: Press [Y/Enter] to approve, [N/Esc] to reject, or [?] to ask query".to_string(), NotifLevel::Warning);
                    return Ok(());
                }
            }
        }

        // If Help modal is open, intercept keys
        if self.ui_state.show_help {
            if self.is_help_key(key) || matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                self.ui_state.show_help = false;
            }
            return Ok(());
        }

        // If Model Picker modal is open, handle picker keys
        if self.ui_state.show_model_picker {
            self.handle_model_picker_key(key).await?;
            return Ok(());
        }

        // Help hotkey (global check)
        if self.is_help_key(key) {
            self.ui_state.show_help = true;
            return Ok(());
        }

        // If LLM is generating, Ctrl+C or Esc cancels the request
        if matches!(self.llm_request_state, LlmRequestState::Streaming | LlmRequestState::ProcessingToolCalls) {
            if (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
                || key.code == KeyCode::Esc {
                self.cancel_llm_request();
                return Ok(());
            }
        }

        // If on interactive Shell tab (Tab 1..N), pass raw input to PTY
        if self.active_tab > 0 {
            match (key.code, key.modifiers) {
                (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
                    self.ui_state.should_quit = true;
                    return Ok(());
                }
                (KeyCode::F(2), _) => {
                    self.ui_state.model_picker_provider = self.config.provider;
                    self.ui_state.model_picker_selected = 0;
                    self.ui_state.model_picker_search.clear();
                    if self.config.get_models_for_provider(self.config.provider).is_empty() {
                        self.trigger_fetch_models();
                    }
                    self.ui_state.show_model_picker = true;
                    return Ok(());
                }
                (KeyCode::F(3), _) => {
                    self.new_shell_tab().await?;
                    return Ok(());
                }
                (KeyCode::Tab, _) | (KeyCode::BackTab, _) => {
                    self.cycle_tab();
                    return Ok(());
                }
                (KeyCode::Esc, _) => {
                    // Esc switches back from Shell tab to Workspace & LLM (Tab 0)
                    self.active_tab = 0;
                    self.input_mode = InputMode::Shell;
                    return Ok(());
                }
                (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                    self.input_mode = InputMode::LlmQuery;
                    self.input_buffer.clear();
                    self.ui_state.input_cursor = 0;
                    self.active_tab = 0; // return to Workspace & LLM tab
                    return Ok(());
                }
                _ => {
                    if let Some(tab) = self.tabs.get_mut(self.active_tab - 1) {
                        if let Some(bytes) = key_to_pty_bytes(key) {
                            let _ = tab.write(&bytes);
                        }
                    }
                    return Ok(());
                }
            }
        }

        // Tab 0 (Workspace & LLM) shortcuts
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::CONTROL) => {
                self.ui_state.should_quit = true;
                return Ok(());
            }
            (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                self.input_mode = InputMode::LlmQuery;
                self.input_buffer.clear();
                self.ui_state.input_cursor = 0;
                return Ok(());
            }
            (KeyCode::F(2), _) => {
                self.ui_state.model_picker_provider = self.config.provider;
                self.ui_state.model_picker_selected = 0;
                self.ui_state.model_picker_search.clear();
                if self.config.get_models_for_provider(self.config.provider).is_empty() {
                    self.trigger_fetch_models();
                }
                self.ui_state.show_model_picker = true;
                return Ok(());
            }
            (KeyCode::F(3), _) => {
                self.new_shell_tab().await?;
                return Ok(());
            }
            (KeyCode::Tab, _) | (KeyCode::BackTab, _) => {
                self.cycle_tab();
                return Ok(());
            }
            (KeyCode::Esc, _) => {
                self.input_mode = InputMode::Shell;
                self.input_buffer.clear();
                self.ui_state.input_cursor = 0;
                self.suggestion_visible = false;
                self.history_index = None;
                self.ui_state.show_help = false;
                self.ui_state.show_model_picker = false;
                self.ui_state.show_session_picker = false;
                self.ui_state.show_api_key_prompt = false;
                return Ok(());
            }
            (KeyCode::Char('['), _) if self.input_buffer.is_empty() => {
                self.ui_state.sidebar_collapsed = !self.ui_state.sidebar_collapsed;
                self.update_pty_dimensions().await?;
                return Ok(());
            }
            _ => {}
        }
        
        // Mode-specific handling
        match self.input_mode {
            InputMode::Shell => self.handle_shell_key(key).await?,
            InputMode::LlmQuery => self.handle_llm_query_key(key).await?,
            InputMode::EditCommand => self.handle_edit_command_key(key).await?,
        }
        Ok(())
    }

    async fn handle_session_picker_key(&mut self, key: KeyEvent) -> Result<()> {
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

    async fn handle_model_picker_key(&mut self, key: KeyEvent) -> Result<()> {
        let provider = self.ui_state.model_picker_provider;
        let models_count = if self.ui_state.model_picker_free_only {
            self.config.get_free_models_for_provider(provider).len()
        } else {
            self.config.search_models_for_provider(provider, &self.ui_state.model_picker_search).len()
        };

        match key.code {
            KeyCode::Esc | KeyCode::F(2) => {
                self.ui_state.show_model_picker = false;
                self.ui_state.model_picker_search.clear();
                self.ui_state.model_picker_selected = 0;
            }
            KeyCode::F(5) => {
                let prov = self.ui_state.model_picker_provider;
                self.trigger_fetch_models_for_provider(prov);
                self.add_notification(
                    format!("Fetching models from {}...", prov.as_str()),
                    NotifLevel::Info,
                );
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let prov = self.ui_state.model_picker_provider;
                self.trigger_fetch_models_for_provider(prov);
                self.add_notification(
                    format!("Fetching models from {}...", prov.as_str()),
                    NotifLevel::Info,
                );
            }
            KeyCode::Up => {
                if self.ui_state.model_picker_selected > 0 {
                    self.ui_state.model_picker_selected -= 1;
                }
            }
            KeyCode::Down => {
                if models_count > 0 && self.ui_state.model_picker_selected + 1 < models_count {
                    self.ui_state.model_picker_selected += 1;
                }
            }
            KeyCode::Tab => {
                let configured = self.config.configured_providers();
                if configured.len() > 1 {
                    let cur = self.ui_state.model_picker_provider;
                    let cur_idx = configured.iter().position(|&p| p == cur).unwrap_or(0);
                    let next_idx = (cur_idx + 1) % configured.len();
                    let next_provider = configured[next_idx];
                    self.ui_state.model_picker_provider = next_provider;
                    self.ui_state.model_picker_selected = 0;
                    self.ui_state.model_picker_search.clear();

                    if self.config.get_models_for_provider(next_provider).is_empty() {
                        self.trigger_fetch_models_for_provider(next_provider);
                    }
                }
            }
            KeyCode::BackTab => {
                let configured = self.config.configured_providers();
                if configured.len() > 1 {
                    let cur = self.ui_state.model_picker_provider;
                    let cur_idx = configured.iter().position(|&p| p == cur).unwrap_or(0);
                    let prev_idx = if cur_idx == 0 { configured.len() - 1 } else { cur_idx - 1 };
                    let prev_provider = configured[prev_idx];
                    self.ui_state.model_picker_provider = prev_provider;
                    self.ui_state.model_picker_selected = 0;
                    self.ui_state.model_picker_search.clear();

                    if self.config.get_models_for_provider(prev_provider).is_empty() {
                        self.trigger_fetch_models_for_provider(prev_provider);
                    }
                }
            }
            KeyCode::F(4) => {
                self.ui_state.model_picker_free_only = !self.ui_state.model_picker_free_only;
                self.ui_state.model_picker_selected = 0;
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.ui_state.model_picker_free_only = !self.ui_state.model_picker_free_only;
                self.ui_state.model_picker_selected = 0;
            }
            KeyCode::Enter => {
                let provider = self.ui_state.model_picker_provider;
                let models: Vec<&crate::config::ModelInfo> = if self.ui_state.model_picker_free_only {
                    self.config.get_free_models_for_provider(provider)
                } else {
                    self.config.search_models_for_provider(provider, &self.ui_state.model_picker_search)
                };
                if let Some(selected) = models.get(self.ui_state.model_picker_selected) {
                    let model_id = selected.id.clone();
                    self.config.set_provider(provider, None);
                    self.config.provider_active_models.insert(provider, model_id.clone());
                    self.config.active_model = model_id.clone();
                    if let Err(e) = self.config.save() {
                        self.add_notification(format!("Failed to save config: {}", e), NotifLevel::Error);
                    } else {
                        self.llm_client = crate::llm::LlmClient::from_config(&self.config);
                        self.add_notification(
                            format!("Active model: {} ({})", self.config.active_model, provider.as_str()),
                            NotifLevel::Info,
                        );
                    }
                }
                self.ui_state.show_model_picker = false;
                self.ui_state.model_picker_search.clear();
                self.ui_state.model_picker_selected = 0;
            }
            KeyCode::Char(c) => {
                self.ui_state.model_picker_search.push(c);
                self.ui_state.model_picker_selected = 0;
            }
            KeyCode::Backspace => {
                self.ui_state.model_picker_search.pop();
                self.ui_state.model_picker_selected = 0;
            }
            _ => {}
        }
        Ok(())
    }

    pub async fn switch_to_provider(&mut self, provider: crate::config::ProviderKind) -> Result<()> {
        if self.config.provider == provider {
            return Ok(());
        }
        self.config.set_provider(provider, None);
        let _ = self.config.save();
        self.llm_client = crate::llm::LlmClient::from_config(&self.config);
        self.ui_state.model_picker_selected = 0;
        self.ui_state.model_picker_search.clear();

        if let Some(cached) = self.config.provider_models.get(&provider) {
            if !cached.is_empty() {
                self.config.models_cache = cached.clone();
                if self.config.active_model.is_empty() {
                    if let Some(first) = self.config.models_cache.first() {
                        self.config.active_model = first.id.clone();
                    }
                }
                self.add_notification(
                    format!("Switched to {} ({} models loaded)", provider.as_str(), self.config.models_cache.len()),
                    NotifLevel::Info,
                );
                return Ok(());
            }
        }

        self.add_notification(
            format!("Switched to {}. Fetching models...", provider.as_str()),
            NotifLevel::Info,
        );
        self.trigger_fetch_models();
        Ok(())
    }
    
    fn is_help_key(&self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::F(1) | KeyCode::F(12) => true,
            KeyCode::Char('h') | KeyCode::Char('H') if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) => true,
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::CONTROL) && self.input_buffer.is_empty() => true,
            _ => false,
        }
    }

    fn insert_input_char(&mut self, c: char) {
        if self.ui_state.input_cursor >= self.input_buffer.len() {
            self.input_buffer.push(c);
            self.ui_state.input_cursor = self.input_buffer.len();
        } else {
            self.input_buffer.insert(self.ui_state.input_cursor, c);
            self.ui_state.input_cursor += 1;
        }
    }

    fn delete_char_before_cursor(&mut self) {
        if self.ui_state.input_cursor > 0 && !self.input_buffer.is_empty() {
            self.ui_state.input_cursor -= 1;
            if self.ui_state.input_cursor < self.input_buffer.len() {
                self.input_buffer.remove(self.ui_state.input_cursor);
            }
        }
    }

    fn delete_char_at_cursor(&mut self) {
        if self.ui_state.input_cursor < self.input_buffer.len() {
            self.input_buffer.remove(self.ui_state.input_cursor);
        }
    }

    fn cursor_left(&mut self) {
        if self.ui_state.input_cursor > 0 {
            self.ui_state.input_cursor -= 1;
        }
    }

    fn cursor_right(&mut self) {
        if self.ui_state.input_cursor < self.input_buffer.len() {
            self.ui_state.input_cursor += 1;
        }
    }

    fn cursor_home(&mut self) {
        self.ui_state.input_cursor = 0;
    }

    fn cursor_end(&mut self) {
        self.ui_state.input_cursor = self.input_buffer.len();
    }

    async fn handle_shell_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Enter => {
                let cmd = self.input_buffer.trim().to_string();
                self.input_buffer.clear();
                self.ui_state.input_cursor = 0;
                if !cmd.is_empty() {
                    self.command_history.push_back(cmd.clone());
                    self.history_index = None;
                    
                    // Handle special commands
                    if cmd.starts_with(':') {
                        self.handle_special_command(&cmd).await?;
                    } else if cmd.starts_with('?') {
                        let query = cmd.trim_start_matches('?').trim().to_string();
                        if !query.is_empty() {
                            self.send_llm_query(&query).await?;
                        }
                    } else if self.active_tab == 0 {
                        // Tab 0: execute as shell command in workspace
                        self.execute_workspace_command(&cmd).await?;
                    } else {
                        // Other tabs: send to PTY
                        self.send_to_pty(&cmd).await?;
                    }
                }
            }
            KeyCode::Char(c) => {
                self.ui_state.focus = crate::app::state::WindowFocus::Input;
                if self.suggestion_visible && self.input_buffer.is_empty() {
                    match c {
                        'r' => {
                            if let Some(sug) = self.suggestion.take() {
                                self.suggestion_visible = false;
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
                            if let Some(sug) = &self.suggestion {
                                if let Some(cmd) = &sug.command {
                                    self.input_buffer = cmd.clone();
                                    self.ui_state.input_cursor = self.input_buffer.len();
                                    self.input_mode = InputMode::EditCommand;
                                }
                            }
                            self.suggestion_visible = false;
                            return Ok(());
                        }
                        's' => {
                            self.suggestion_visible = false;
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
                    self.input_buffer = self.command_history.back().unwrap().clone();
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
    
    async fn handle_llm_query_key(&mut self, key: KeyEvent) -> Result<()> {
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
    
    async fn handle_edit_command_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Enter => {
                let cmd = self.input_buffer.trim().to_string();
                self.input_buffer.clear();
                self.ui_state.input_cursor = 0;
                self.input_mode = InputMode::Shell;
                self.suggestion_visible = false;
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
                self.suggestion_visible = false;
            }
            _ => {}
        }
        Ok(())
    }
    
    async fn handle_special_command(&mut self, cmd: &str) -> Result<()> {
        let parts: Vec<&str> = cmd.splitn(2, ' ').collect();
        match parts[0] {
            ":target" => {
                if parts.len() > 1 {
                    let input = parts[1].trim();
                    let target = if let Ok(id) = input.parse::<i64>() {
                        DatabaseQueries::get_target(self.db.conn(), id)?
                    } else {
                        DatabaseQueries::get_target_by_ip(self.db.conn(), input)?
                    };
                    if let Some(target) = target {
                        self.active_target_id = Some(target.id);
                        self.reload_sidebar().await?;
                        self.add_notification(format!("Active target: {} (ID: {})", target.ip_host, target.id), NotifLevel::Info);
                    } else {
                        let new_id = DatabaseQueries::add_target(self.db.conn(), input, None, None)?;
                        self.active_target_id = Some(new_id);
                        self.reload_sidebar().await?;
                        self.add_notification(format!("Added & activated target: {} (ID: {})", input, new_id), NotifLevel::Info);
                    }
                } else if let Some(target_id) = self.active_target_id {
                    if let Some(target) = DatabaseQueries::get_target(self.db.conn(), target_id)? {
                        self.add_notification(format!("Active target: {} (ID: {})", target.ip_host, target.id), NotifLevel::Info);
                    }
                } else {
                    self.add_notification("No active target. Usage: :target <ip_or_id>".to_string(), NotifLevel::Warning);
                }
            }
            ":targets" => self.show_targets_table().await?,
            ":findings" => {
                let target_id = if parts.len() > 1 {
                    parts[1].parse().ok()
                } else {
                    self.active_target_id
                };
                self.show_findings(target_id).await?;
            }
            ":note" => {
                if parts.len() > 1 {
                    if let Some(target_id) = self.active_target_id {
                        DatabaseQueries::add_note(
                            self.db.conn(),
                            target_id,
                            parts[1],
                        )?;
                        self.add_notification("Note added".to_string(), NotifLevel::Info);
                        self.reload_sidebar().await?;
                    }
                }
            }
            ":models" | ":model" => {
                self.ui_state.model_picker_provider = self.config.provider;
                self.ui_state.model_picker_selected = 0;
                self.ui_state.model_picker_search.clear();
                if self.config.get_models_for_provider(self.config.provider).is_empty() {
                    self.trigger_fetch_models();
                }
                self.ui_state.show_model_picker = true;
            }
            ":provider" | ":providers" => {
                if parts.len() > 1 {
                    let subparts: Vec<&str> = parts[1].split_whitespace().collect();
                    match subparts.first().copied().unwrap_or("") {
                        "openrouter" => {
                            self.switch_to_provider(crate::config::ProviderKind::OpenRouter).await?;
                        }
                        "nvidia" | "nvidianim" | "nim" => {
                            self.switch_to_provider(crate::config::ProviderKind::NvidiaNim).await?;
                        }
                        "openai" => {
                            self.switch_to_provider(crate::config::ProviderKind::OpenAi).await?;
                        }
                        "ollama" => {
                            let url = subparts.get(1).map(|u| u.to_string());
                            self.config.set_provider(crate::config::ProviderKind::Ollama, url);
                            let _ = self.config.save();
                            self.llm_client = crate::llm::LlmClient::from_config(&self.config);
                            self.add_notification("Switched provider to Ollama (Local)".to_string(), NotifLevel::Info);
                            self.trigger_fetch_models();
                        }
                        "custom" => {
                            let url = subparts.get(1).map(|u| u.to_string());
                            self.config.set_provider(crate::config::ProviderKind::Custom, url);
                            let _ = self.config.save();
                            self.llm_client = crate::llm::LlmClient::from_config(&self.config);
                            self.add_notification("Switched provider to Custom endpoint".to_string(), NotifLevel::Info);
                            self.trigger_fetch_models();
                        }
                        "setup" | "menu" => {
                            self.ui_state.show_api_key_prompt = true;
                        }
                        other => {
                            self.add_notification(
                                format!("Unknown provider '{}'. Supported: openrouter, nvidia, openai, ollama, custom, setup", other),
                                NotifLevel::Warning,
                            );
                        }
                    }
                } else {
                    self.ui_state.show_api_key_prompt = true;
                }
            }
            ":sessions" => {
                self.ui_state.session_list = crate::app::state::SessionMeta::list_sessions();
                self.ui_state.session_picker_selected = 0;
                self.ui_state.session_naming_mode = false;
                self.ui_state.session_name_input.clear();
                self.ui_state.session_delete_confirm = None;
                self.ui_state.is_initial_session_select = false;
                self.ui_state.show_session_picker = true;
            }
            ":newtab" => {
                self.new_shell_tab().await?;
            }
            ":help" | ":h" => {
                self.ui_state.show_help = !self.ui_state.show_help;
            }
            ":quit" => {
                self.ui_state.should_quit = true;
            }
            ":closeshell" => {
                self.close_current_shell().await?;
            }
            ":exploit" => {
                if self.active_target_id.is_some() {
                    self.send_llm_query("give me a shell on this target").await?;
                }
            }
            _ => {
                self.add_notification(format!("Unknown command: {}", cmd), NotifLevel::Warning);
            }
        }
        Ok(())
    }
    
    async fn execute_workspace_command(&mut self, cmd: &str) -> Result<()> {
        let start = Instant::now();
        
        // Add to chat log
        self.append_chat_message(ChatMessage {
            role: "user".to_string(),
            content: format!("$ {}", cmd),
            tool_calls: None,
            tool_call_id: None,
        });
        
        // Execute command and capture output
        let output = self.run_command_capture(cmd).await?;
        let duration_ms = start.elapsed().as_millis() as i64;
        let exit_code = 0; // Would get actual exit code
        
        // Save to database with full output artifact
        let target_id = self.active_target_id;
        let _ = DatabaseQueries::add_chat_message(self.db.conn(), target_id, "user", cmd);
        let _ = DatabaseQueries::add_chat_message(self.db.conn(), target_id, "assistant", &output);
        DatabaseQueries::add_command(
            self.db.conn(),
            &self.session_meta.path(),
            target_id,
            cmd,
            Some(&output),
            Some(exit_code),
            Some(duration_ms),
        )?;
        
        // Add output to chat log
        self.append_chat_message(ChatMessage {
            role: "output".to_string(),
            content: output.clone(),
            tool_calls: None,
            tool_call_id: None,
        });
        self.ui_state.log_at_bottom = true;
        
        // Trigger post-execution analysis
        self.post_exec_mode = true;
        self.send_post_exec_analysis(cmd, &output).await?;
        
        // Reload sidebar
        self.reload_sidebar().await?;
        
        Ok(())
    }
    
    async fn run_command_capture(&self, cmd: &str) -> Result<String> {
        let ws_path = self.session_meta.workspace_path();
        let _ = std::fs::create_dir_all(&ws_path);

        #[cfg(target_os = "windows")]
        let mut command = tokio::process::Command::new("cmd");
        #[cfg(target_os = "windows")]
        command.args(["/C", cmd]);

        #[cfg(not(target_os = "windows"))]
        let mut command = tokio::process::Command::new("sh");
        #[cfg(not(target_os = "windows"))]
        command.args(["-c", cmd]);

        command.current_dir(&ws_path);
        let output = command.output().await.context("Failed to execute command")?;
        
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        
        if stderr.is_empty() {
            Ok(stdout.to_string())
        } else {
            Ok(format!("{}\n{}", stdout, stderr))
        }
    }
    
    async fn send_to_pty(&mut self, cmd: &str) -> Result<()> {
        if self.active_tab > 0 && self.active_tab - 1 < self.tabs.len() {
            if let Some(tab) = self.tabs.get_mut(self.active_tab - 1) {
                tab.write(format!("{}\n", cmd).as_bytes())?;
            }
        }
        Ok(())
    }
    
    async fn send_llm_query(&mut self, query: &str) -> Result<()> {
        if self.llm_client.provider().requires_api_key() && self.config.api_key.trim().is_empty() {
            self.add_notification(
                format!("{} API key not configured", self.llm_client.provider().name()),
                NotifLevel::Error,
            );
            return Ok(());
        }

        // Add user question to visible chat log
        self.append_chat_message(ChatMessage {
            role: "user".to_string(),
            content: format!("? {}", query),
            tool_calls: None,
            tool_call_id: None,
        });
        self.ui_state.log_at_bottom = true;
        
        // Build context
        let active_target = if let Some(id) = self.active_target_id {
            DatabaseQueries::get_target(self.db.conn(), id)?
        } else {
            None
        };
        
        let (findings, recent_commands, command_count) = if let Some(target) = &active_target {
            let f = DatabaseQueries::get_findings_for_target(self.db.conn(), target.id)?;
            let r = DatabaseQueries::get_recent_commands(self.db.conn(), target.id, 3)?;
            let c = DatabaseQueries::get_command_count(self.db.conn(), target.id)?;
            (f, r, c)
        } else {
            (Vec::new(), Vec::new(), 0)
        };
        let targets_count = DatabaseQueries::get_targets_count(self.db.conn())?;
        let all_targets = DatabaseQueries::get_all_targets(self.db.conn())?;
        
        let system_prompt = build_system_prompt(
            &self.session_meta.name,
            &self.session_meta.created_at.to_rfc3339(),
            targets_count,
            &all_targets,
            active_target.as_ref(),
            &findings,
            &recent_commands,
            command_count,
            recent_commands.first().map(|c| c.executed_at.to_rfc3339()),
            Some(&self.session_meta.workspace_path()),
        );
        
        let mut messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: system_prompt,
                tool_calls: None,
                tool_call_id: None,
            },
        ];

        // Include recent conversational turns (last 6 messages) for multi-turn coherence
        let history_start = self.chat_log.len().saturating_sub(6);
        for msg in &self.chat_log[history_start..] {
            if msg.role == "user" && msg.content.starts_with('?') {
                messages.push(ChatMessage {
                    role: "user".to_string(),
                    content: msg.content.trim_start_matches("? ").to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                });
            } else if msg.role == "assistant" && !msg.content.is_empty() {
                let content = if msg.content.len() > 800 {
                    format!("{}...", &msg.content[..800])
                } else {
                    msg.content.clone()
                };
                messages.push(ChatMessage {
                    role: "assistant".to_string(),
                    content,
                    tool_calls: None,
                    tool_call_id: None,
                });
            }
        }

        messages.push(ChatMessage {
            role: "user".to_string(),
            content: query.to_string(),
            tool_calls: None,
            tool_call_id: None,
        });
        
        let request_id = self.next_request_id();
        let request = crate::llm::types::ChatCompletionRequest {
            model: self.config.active_model.clone(),
            messages,
            tools: Some(get_tool_definitions()),
            tool_choice: Some(serde_json::json!("auto")),
            stream: true,
            temperature: 0.1,
            max_tokens: 2048,
        };
        
        self.llm_request_state = LlmRequestState::Streaming;
        self.llm_response_buffer.clear();
        self.pending_llm_request = Some(query.to_string());
        
        let tx = self.event_tx.clone();
        let client = self.llm_client.clone();
        
        let handle = tokio::spawn(async move {
            let _ = client.stream_chat(request, tx, request_id).await;
        });
        self.llm_abort_handle = Some(handle.abort_handle());
        
        Ok(())
    }
    
    async fn send_post_exec_analysis(&mut self, cmd: &str, output: &str) -> Result<()> {
        if self.llm_client.provider().requires_api_key() && self.config.api_key.trim().is_empty() {
            return Ok(());
        }
        
        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: "You are an automated security recon engine in voidrecon. Analyze the command output. If you identify new targets, open ports, services, operating system details, or security findings, invoke the appropriate tools (set_target, update_target, add_finding, add_target, save_workspace_file) to record them into the session database. Provide a concise 1-2 sentence summary.".to_string(),
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: "user".to_string(),
                content: format!("Command: {}\n\nOutput:\n{}\n\n{}", cmd, output, POST_EXEC_PROMPT),
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        
        let request_id = self.next_request_id();
        let request = crate::llm::types::ChatCompletionRequest {
            model: self.config.active_model.clone(),
            messages,
            tools: Some(get_tool_definitions()),
            tool_choice: Some(serde_json::json!("auto")),
            stream: true,
            temperature: 0.1,
            max_tokens: 1024,
        };
        
        self.llm_request_state = LlmRequestState::Streaming;
        self.llm_response_buffer.clear();
        
        let tx = self.event_tx.clone();
        let client = self.llm_client.clone();
        
        let handle = tokio::spawn(async move {
            let _ = client.stream_chat(request, tx, request_id).await;
        });
        self.llm_abort_handle = Some(handle.abort_handle());
        
        Ok(())
    }
    
    fn handle_llm_chunk(&mut self, request_id: u64, chunk: String) {
        // Ignore stale requests
        if request_id != self.llm_request_id {
            return;
        }
        self.llm_response_buffer.push_str(&chunk);
    }
    
    async fn handle_llm_tool_call(&mut self, request_id: u64, tool_call: ToolCall) -> Result<()> {
        // Ignore stale requests
        if request_id != self.llm_request_id {
            return Ok(());
        }
        self.pending_tool_calls.push(tool_call);
        self.llm_request_state = LlmRequestState::ProcessingToolCalls;
        Ok(())
    }
    
    fn handle_llm_request_started(&mut self, request_id: u64) {
        if request_id == self.llm_request_id {
            self.llm_request_state = LlmRequestState::Streaming;
        }
    }
    
    fn handle_llm_request_failed(&mut self, request_id: u64, error: String) {
        if request_id == self.llm_request_id {
            self.llm_abort_handle = None;
            self.llm_request_state = LlmRequestState::Failed;
            self.add_notification(format!("LLM request failed: {}", error), NotifLevel::Error);
            self.append_chat_message(ChatMessage {
                role: "system".to_string(),
                content: format!("⚠️ LLM Request Failed: {}", error),
                tool_calls: None,
                tool_call_id: None,
            });
            self.ui_state.log_at_bottom = true;
        }
    }
    
    async fn handle_llm_done(&mut self, request_id: u64) -> Result<()> {
        // Ignore stale requests
        if request_id != self.llm_request_id {
            return Ok(());
        }
        
        self.llm_abort_handle = None;
        self.llm_request_state = LlmRequestState::Completed;
        
        if !self.pending_tool_calls.is_empty() {
            // Push assistant message with tool calls into chat log first (required by LLM APIs)
            let tool_calls = std::mem::take(&mut self.pending_tool_calls);
            self.append_chat_message(ChatMessage {
                role: "assistant".to_string(),
                content: self.llm_response_buffer.clone(),
                tool_calls: Some(tool_calls.clone()),
                tool_call_id: None,
            });

            // Execute tool calls
            for tool_call in tool_calls {
                let result = crate::llm::tools::execute_tool_call(
                    &tool_call,
                    &self.db,
                    self.active_target_id,
                    &self.session_meta.workspace_path(),
                    &self.event_tx,
                ).await?;

                if tool_call.function.name == "save_exploit_script" {
                    if let Some(path) = result.get("path").and_then(|v| v.as_str()) {
                        self.add_notification(
                            format!("Saved exploit to {}", path),
                            NotifLevel::Info,
                        );
                    }
                }

                if tool_call.function.name == "save_workspace_file" {
                    if let Some(path) = result.get("path").and_then(|v| v.as_str()) {
                        self.add_notification(
                            format!("Saved file: {}", path),
                            NotifLevel::Info,
                        );
                    }
                }

                if tool_call.function.name == "set_target" || tool_call.function.name == "add_target" {
                    if let Some(target_id) = result.get("id").and_then(|v| v.as_i64()).or_else(|| result.get("target_id").and_then(|v| v.as_i64())) {
                        self.active_target_id = Some(target_id);
                        let ip = result.get("ip_host").and_then(|v| v.as_str()).unwrap_or("target");
                        self.add_notification(format!("Active target: {} (ID: {})", ip, target_id), NotifLevel::Info);
                    }
                }
                
                // Handle special tool results
                if tool_call.function.name == "execute_exploit_script" {
                    let target_id = result.get("target_id").and_then(|v| v.as_i64()).unwrap_or(0);
                    let target = DatabaseQueries::get_target(self.db.conn(), target_id)?;
                    if let Some(target) = target {
                        let script_name = result.get("script_name").and_then(|v| v.as_str()).unwrap_or("exploit.py").to_string();
                        let command_line = result.get("command_line").and_then(|v| v.as_str()).unwrap_or("").to_string();

                        self.pending_exploit_approval = Some(crate::app::state::ExploitApproval {
                            target_id,
                            target_ip: target.ip_host,
                            kind: crate::app::state::ApprovalKind::ExecuteExploit {
                                script_name: script_name.clone(),
                                command_line,
                            },
                            tool_call: tool_call.clone(),
                        });
                        self.add_notification(
                            format!("Approval required to execute {}", script_name),
                            NotifLevel::Warning,
                        );
                        return Ok(());
                    } else {
                        self.append_chat_message(ChatMessage {
                            role: "tool".to_string(),
                            content: serde_json::json!({
                                "error": format!("Target ID {} is not within current assessment scope", target_id)
                            }).to_string(),
                            tool_calls: None,
                            tool_call_id: Some(tool_call.id),
                        });
                        continue;
                    }
                }

                if tool_call.function.name == "open_reverse_shell_tab" {
                    let target_id = result.get("target_id").and_then(|v| v.as_i64()).unwrap_or(0);
                    let target = DatabaseQueries::get_target(self.db.conn(), target_id)?;
                    if let Some(target) = target {
                        let lhost = result.get("lhost").and_then(|v| v.as_str()).unwrap_or("0.0.0.0").to_string();
                        let lport = result.get("lport").and_then(|v| v.as_u64()).unwrap_or(4444) as u16;
                        let shell_type = result.get("shell_type").and_then(|v| v.as_str()).unwrap_or("bash").to_string();

                        self.pending_exploit_approval = Some(crate::app::state::ExploitApproval {
                            target_id,
                            target_ip: target.ip_host,
                            kind: crate::app::state::ApprovalKind::ReverseShellListener {
                                lhost,
                                lport,
                                shell_type,
                            },
                            tool_call: tool_call.clone(),
                        });
                        self.add_notification(
                            format!("Approval required for reverse shell on port {}", lport),
                            NotifLevel::Warning,
                        );
                        return Ok(());
                    } else {
                        self.append_chat_message(ChatMessage {
                            role: "tool".to_string(),
                            content: serde_json::json!({
                                "error": format!("Target ID {} is not within current assessment scope", target_id)
                            }).to_string(),
                            tool_calls: None,
                            tool_call_id: Some(tool_call.id),
                        });
                        continue;
                    }
                }
                
                // Add tool result to conversation for next LLM call
                self.append_chat_message(ChatMessage {
                    role: "tool".to_string(),
                    content: result.to_string(),
                    tool_calls: None,
                    tool_call_id: Some(tool_call.id),
                });
            }
            
            // Reload sidebar to reflect any target/findings DB changes
            self.reload_sidebar().await?;

            // Continue conversation with tool results
            self.continue_llm_conversation().await?;
        } else if self.post_exec_mode {
            // Post-execution analysis complete
            self.post_exec_mode = false;
            // Add LLM comment to chat log
            self.append_chat_message(ChatMessage {
                role: "assistant".to_string(),
                content: self.llm_response_buffer.clone(),
                tool_calls: None,
                tool_call_id: None,
            });
            
            // Check for new asset detection (simplified)
            if self.llm_response_buffer.to_lowercase().contains("detected") {
                self.add_notification("New assets detected - check LLM output".to_string(), NotifLevel::Info);
            }
        } else {
            // Regular LLM query / suggestion
            let suggestion = parse_llm_response(&self.llm_response_buffer)?;
            let content_display = if !suggestion.explanation.is_empty() {
                suggestion.explanation.clone()
            } else {
                self.llm_response_buffer.clone()
            };
            self.append_chat_message(ChatMessage {
                role: "assistant".to_string(),
                content: content_display,
                tool_calls: None,
                tool_call_id: None,
            });
            if suggestion.command.is_some() {
                self.suggestion = Some(suggestion);
                self.suggestion_visible = true;
            }
            self.ui_state.log_at_bottom = true;
        }
        
        self.llm_response_buffer.clear();
        self.pending_llm_request = None;
        
        Ok(())
    }
    
    async fn continue_llm_conversation(&mut self) -> Result<()> {
        let mut messages = Vec::new();

        // System prompt with full context
        let active_target = if let Some(id) = self.active_target_id {
            DatabaseQueries::get_target(self.db.conn(), id).unwrap_or(None)
        } else {
            None
        };
        let (findings, recent_commands, command_count) = if let Some(target) = &active_target {
            let f = DatabaseQueries::get_findings_for_target(self.db.conn(), target.id).unwrap_or_default();
            let r = DatabaseQueries::get_recent_commands(self.db.conn(), target.id, 3).unwrap_or_default();
            let c = DatabaseQueries::get_command_count(self.db.conn(), target.id).unwrap_or(0);
            (f, r, c)
        } else {
            (Vec::new(), Vec::new(), 0)
        };
        let targets_count = DatabaseQueries::get_targets_count(self.db.conn()).unwrap_or(0);
        let all_targets = DatabaseQueries::get_all_targets(self.db.conn()).unwrap_or_default();
        let system_prompt = build_system_prompt(
            &self.session_meta.name,
            &self.session_meta.created_at.to_rfc3339(),
            targets_count,
            &all_targets,
            active_target.as_ref(),
            &findings,
            &recent_commands,
            command_count,
            recent_commands.first().map(|c| c.executed_at.to_rfc3339()),
            Some(&self.session_meta.workspace_path()),
        );
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: system_prompt,
            tool_calls: None,
            tool_call_id: None,
        });

        // Filter valid OpenAI messages within a bounded window (last 12 messages) to protect LLM context limits
        let window_start = self.chat_log.len().saturating_sub(12);
        let mut slice = &self.chat_log[window_start..];
        
        // Ensure we don't start slice with an orphaned 'tool' message without its preceding 'assistant' message
        if let Some(first_tool_pos) = slice.iter().position(|m| m.role == "tool") {
            if first_tool_pos == 0 && window_start > 0 {
                slice = &self.chat_log[window_start - 1..];
            }
        }

        for msg in slice {
            match msg.role.as_str() {
                "user" | "assistant" | "system" | "tool" => {
                    messages.push(msg.clone());
                }
                "output" => {
                    let lines: Vec<&str> = msg.content.lines().collect();
                    let truncated_content = if lines.len() > 25 {
                        let top = lines[..20].join("\n");
                        format!("{}\n... [truncated {} lines; full output stored in workspace/outputs/] ...", top, lines.len() - 20)
                    } else {
                        msg.content.clone()
                    };
                    messages.push(ChatMessage {
                        role: "user".to_string(),
                        content: format!("[Command Output]:\n{}", truncated_content),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
                _ => {}
            }
        }
        
        let request_id = self.next_request_id();
        let request = crate::llm::types::ChatCompletionRequest {
            model: self.config.active_model.clone(),
            messages,
            tools: Some(get_tool_definitions()),
            tool_choice: Some(serde_json::json!("auto")),
            stream: true,
            temperature: 0.1,
            max_tokens: 2048,
        };
        
        self.llm_request_state = LlmRequestState::Streaming;
        self.llm_response_buffer.clear();
        
        let tx = self.event_tx.clone();
        let client = self.llm_client.clone();
        
        let handle = tokio::spawn(async move {
            let _ = client.stream_chat(request, tx, request_id).await;
        });
        self.llm_abort_handle = Some(handle.abort_handle());
        
        Ok(())
    }
    
    async fn handle_pty_data(&mut self, _tab_id: usize, _data: Vec<u8>) -> Result<()> {
        // Data already processed by reader thread
        // Just ensure UI updates - we could force a redraw here if needed
        Ok(())
    }
    
    async fn handle_pty_exited(&mut self, tab_id: usize, exit_code: i32) -> Result<()> {
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
    
    fn handle_tick(&mut self) -> bool {
        let now = Instant::now();
        let had_notifications = !self.notifications.is_empty();
        self.notifications.retain(|n| now.duration_since(n.created_at).as_secs() < 5);
        if self.notifications.len() > 3 {
            self.notifications.drain(0..self.notifications.len() - 3);
        }
        let has_notifications = !self.notifications.is_empty();
        self.last_tick = now;

        let is_animating = matches!(
            self.llm_request_state,
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

    async fn handle_resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        self.terminal_size = (cols, rows);
        self.update_pty_dimensions().await?;
        Ok(())
    }
    
    pub async fn reload_sidebar(&mut self) -> Result<()> {
        let targets = DatabaseQueries::get_all_targets(self.db.conn())?;
        self.sidebar_nodes.clear();
        
        for target in targets {
            let findings = DatabaseQueries::get_findings_for_target(self.db.conn(), target.id)?;
            let mut by_severity: std::collections::HashMap<String, Vec<Finding>> = std::collections::HashMap::new();
            for finding in findings {
                by_severity.entry(finding.severity.clone()).or_default().push(finding);
            }
            let mut findings_by_severity: Vec<_> = by_severity.into_iter().collect();
            findings_by_severity.sort_by(|a, b| {
                let severity_order = |s: &str| match s {
                    "critical" => 0,
                    "high" => 1,
                    "medium" => 2,
                    "low" => 3,
                    "info" => 4,
                    _ => 5,
                };
                severity_order(&a.0).cmp(&severity_order(&b.0))
            });
            
            let ports = if let Some(ports_json) = &target.ports_json {
                serde_json::from_str::<Vec<crate::db::models::PortInfo>>(ports_json).unwrap_or_default()
            } else {
                Vec::new()
            };
            
            self.sidebar_nodes.push(SidebarNode {
                target,
                ports,
                expanded: true,
                findings_by_severity,
            });
        }
        
        // If no active target, select first
        if self.active_target_id.is_none() && !self.sidebar_nodes.is_empty() {
            self.active_target_id = Some(self.sidebar_nodes[0].target.id);
        }
        
        Ok(())
    }
    
    async fn new_shell_tab(&mut self) -> Result<()> {
        let tab = PtyTab::new_shell(self.next_tab_id, self.event_tx.clone())?;
        self.next_tab_id += 1;
        self.tabs.push(tab);
        self.active_tab = self.tabs.len(); // switch focus to the new shell tab (Tab 1..N)
        Ok(())
    }
    
    async fn spawn_listener_tab(&mut self, lport: u16) -> Result<()> {
        let tab = PtyTab::new_listener(self.next_tab_id, lport, self.event_tx.clone())?;
        self.next_tab_id += 1;
        self.tabs.push(tab);
        self.active_tab = self.tabs.len(); // switch focus to the listener tab
        Ok(())
    }
    
    async fn close_current_shell(&mut self) -> Result<()> {
        if self.active_tab > 0 && self.active_tab - 1 < self.tabs.len() {
            self.tabs.remove(self.active_tab - 1);
            if self.active_tab > self.tabs.len() {
                self.active_tab = self.tabs.len();
            }
        }
        Ok(())
    }
    
    fn cycle_tab(&mut self) {
        let total = 1 + self.tabs.len();
        self.active_tab = (self.active_tab + 1) % total;
    }
    
    async fn show_targets_table(&mut self) -> Result<()> {
        let targets = DatabaseQueries::get_all_targets(self.db.conn())?;
        let mut table = String::from("ID | IP/Host | Ports | Findings\n");
        table.push_str("---|---------|-------|---------\n");
        for target in targets {
            let ports_count = target.ports_json.as_ref()
                .and_then(|p| serde_json::from_str::<Vec<crate::db::models::PortInfo>>(p).ok())
                .map(|v| v.len())
                .unwrap_or(0);
            let findings_count = DatabaseQueries::get_findings_for_target(self.db.conn(), target.id)?.len();
            table.push_str(&format!("{} | {} | {} | {}\n", target.id, target.ip_host, ports_count, findings_count));
        }
        self.append_chat_message(ChatMessage {
            role: "assistant".to_string(),
            content: table,
            tool_calls: None,
            tool_call_id: None,
        });
        Ok(())
    }
    
    async fn show_findings(&mut self, target_id: Option<i64>) -> Result<()> {
        let target_id = target_id.or(self.active_target_id);
        if let Some(id) = target_id {
            let findings = DatabaseQueries::get_findings_for_target(self.db.conn(), id)?;
            let mut output = String::from("Findings:\n");
            for f in findings {
                output.push_str(&format!("[{}] {}: {} (from: {})\n", f.severity, f.title, f.description, f.source_cmd.unwrap_or_default()));
            }
            self.append_chat_message(ChatMessage {
                role: "assistant".to_string(),
                content: output,
                tool_calls: None,
                tool_call_id: None,
            });
        }
        Ok(())
    }
    
    pub async fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> Result<()> {
        use crossterm::event::{MouseButton, MouseEventKind};
        use crate::app::state::WindowFocus;

        let term_rows = self.terminal_size.1;
        let sidebar_width = if self.ui_state.sidebar_collapsed { 0 } else { 40 };

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left)
            | MouseEventKind::Up(MouseButton::Left)
            | MouseEventKind::Drag(MouseButton::Left) => {
                // If any modal is showing, do not change focus
                if self.ui_state.show_model_picker
                    || self.ui_state.show_session_picker
                    || self.ui_state.show_api_key_prompt
                    || self.ui_state.show_help
                {
                    return Ok(());
                }

                // Check tab bar click (row 1)
                if mouse.row == 1 && mouse.column >= sidebar_width {
                    let col_in_tabs = mouse.column.saturating_sub(sidebar_width);
                    if col_in_tabs < 24 {
                        self.active_tab = 0;
                    } else {
                        let shell_tab_idx = (col_in_tabs.saturating_sub(24) / 16) as usize;
                        if shell_tab_idx < self.tabs.len() {
                            self.active_tab = shell_tab_idx + 1;
                        }
                    }
                    return Ok(());
                }

                // In Tab 0:
                if self.active_tab == 0 {
                    // Check if clicked in sidebar
                    if !self.ui_state.sidebar_collapsed && mouse.column < sidebar_width {
                        self.ui_state.focus = WindowFocus::Sidebar;
                        let target_idx = (mouse.row.saturating_sub(2) as usize) / 2;
                        if target_idx < self.sidebar_nodes.len() {
                            let tid = self.sidebar_nodes[target_idx].target.id;
                            self.active_target_id = Some(tid);
                        }
                        return Ok(());
                    }

                    // Input line is at row term_rows - 2 (footer is term_rows - 1)
                    let input_row = term_rows.saturating_sub(2);
                    let suggestion_height = if self.suggestion_visible { 5 } else { 0 };
                    let approval_height = if self.pending_exploit_approval.is_some() { 5 } else { 0 };
                    let panels_height = suggestion_height + approval_height;
                    let input_area_start = input_row.saturating_sub(panels_height as u16);

                    if mouse.row >= input_area_start {
                        // Clicked on input line or action panels -> focus input
                        self.ui_state.focus = WindowFocus::Input;
                        let prompt_len = 2; // "> " or "? "
                        let click_col = mouse.column.saturating_sub(sidebar_width + prompt_len);
                        self.ui_state.input_cursor = (click_col as usize).min(self.input_buffer.len());
                    } else if mouse.row >= 2 {
                        // Clicked inside the chat / log window -> focus chat window
                        self.ui_state.focus = WindowFocus::ChatLog;
                    }
                }
            }
            MouseEventKind::ScrollUp => {
                if self.ui_state.show_model_picker {
                    self.ui_state.model_picker_selected = self.ui_state.model_picker_selected.saturating_sub(1);
                    return Ok(());
                }
                if self.ui_state.show_session_picker {
                    self.ui_state.session_picker_selected = self.ui_state.session_picker_selected.saturating_sub(1);
                    return Ok(());
                }
                if self.active_tab == 0 {
                    self.ui_state.focus = WindowFocus::ChatLog;
                    self.ui_state.log_at_bottom = false;
                    self.ui_state.log_scroll = self.ui_state.log_scroll.saturating_sub(3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.ui_state.show_model_picker {
                    self.ui_state.model_picker_selected = self.ui_state.model_picker_selected.saturating_add(1);
                    return Ok(());
                }
                if self.ui_state.show_session_picker {
                    self.ui_state.session_picker_selected = self.ui_state.session_picker_selected.saturating_add(1);
                    return Ok(());
                }
                if self.active_tab == 0 {
                    self.ui_state.focus = WindowFocus::ChatLog;
                    self.ui_state.log_scroll = self.ui_state.log_scroll.saturating_add(3);
                }
            }
            _ => {}
        }

        Ok(())
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

/// Convert crossterm KeyEvent to raw terminal bytes for PTY master stream
fn key_to_pty_bytes(key: KeyEvent) -> Option<Vec<u8>> {
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