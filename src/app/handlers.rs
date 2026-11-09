//! Event handlers for voidrecon

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::Backend;
use std::time::Instant;

use crate::app::events::AppEvent;
use crate::app::state::{App, InputMode, NotifLevel, SidebarNode, WindowFocus};
use crate::db::DatabaseQueries;
use crate::db::models::Finding;
use crate::llm::{ChatMessage, LlmRequestState};

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
                    if let Ok(client) = crate::llm::LlmClient::from_config(&self.config) {
                        self.llm_client = client;
                    } else {
                        self.add_notification("Failed to rebuild LLM client".to_string(), NotifLevel::Error);
                    }
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
                                        &self.db.conn(),
                                        &self.session_meta.path(),
                                        Some(approval.target_id),
                                        command_line,
                                        Some(&out),
                                        Some(0),
                                        Some(duration_ms),
                                    );
                                    let _ = DatabaseQueries::add_chat_message(&self.db.conn(), Some(approval.target_id), "user", &format!("$ {}", command_line));
                                    let _ = DatabaseQueries::add_chat_message(&self.db.conn(), Some(approval.target_id), "assistant", &out);

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
                        crate::app::state::ApprovalKind::SaveWorkspaceFile { folder, filename, description, content } => {
                            let target_dir = self.session_meta.workspace_path().join(&folder);
                            let _ = std::fs::create_dir_all(&target_dir);
                            let file_path = target_dir.join(&filename);
                            match std::fs::write(&file_path, &content) {
                                Ok(_) => {
                                    let rel_path = format!("workspace/{}/{}", folder, filename);
                                    if folder == "evidence" {
                                        let title = if description.is_empty() {
                                            format!("Evidence: {}", filename)
                                        } else {
                                            format!("Evidence: {} ({})", filename, description)
                                        };
                                        let _ = DatabaseQueries::add_finding(
                                            &self.db.conn(),
                                            approval.target_id,
                                            &title,
                                            &description,
                                            "info",
                                            None,
                                            Some(100),
                                            Some(&rel_path),
                                        );
                                    } else if folder == "artifacts" {
                                        let note_text = if description.is_empty() {
                                            format!("Saved artifact: {}", rel_path)
                                        } else {
                                            format!("Saved artifact {}: {}", rel_path, description)
                                        };
                                        let _ = DatabaseQueries::add_note(&self.db.conn(), approval.target_id, &note_text);
                                    }

                                    self.append_chat_message(ChatMessage {
                                        role: "tool".to_string(),
                                        content: serde_json::json!({
                                            "status": "approved_and_saved",
                                            "folder": folder,
                                            "file": filename,
                                            "path": rel_path,
                                            "bytes": content.len()
                                        }).to_string(),
                                        tool_calls: None,
                                        tool_call_id: Some(approval.tool_call.id),
                                    });
                                    self.add_notification(format!("Approved & saved: {}", rel_path), NotifLevel::Info);
                                    let _ = self.reload_sidebar().await;
                                }
                                Err(e) => {
                                    self.append_chat_message(ChatMessage {
                                        role: "tool".to_string(),
                                        content: serde_json::json!({
                                            "status": "save_failed",
                                            "error": e.to_string()
                                        }).to_string(),
                                        tool_calls: None,
                                        tool_call_id: Some(approval.tool_call.id),
                                    });
                                    self.add_notification(format!("Failed to save file: {}", e), NotifLevel::Error);
                                }
                            }
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
                        crate::app::state::ApprovalKind::SaveWorkspaceFile { .. } => "User rejected saving workspace file",
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
        if matches!(self.llm.request_state, LlmRequestState::Streaming | LlmRequestState::ProcessingToolCalls) {
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
                (KeyCode::F(4), _) | (KeyCode::Char('m'), KeyModifiers::ALT) => {
                    self.toggle_mouse_capture()?;
                    return Ok(());
                }
                (KeyCode::BackTab, _) => {
                    self.cycle_focus_backward();
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
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                if let Some(handle) = self.command_run.abort_handle.take() {
                    handle.abort();
                    self.command_run.running = None;
                    self.command_run.stdin_tx = None;
                    self.command_run.output.clear();
                    self.command_run.raw.clear();
                    self.append_chat_message(ChatMessage {
                        role: "system".to_string(),
                        content: "⚠️ Command cancelled by user (Ctrl+C)".to_string(),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                    self.add_notification("Command cancelled".to_string(), NotifLevel::Warning);
                    return Ok(());
                }
                if let Some(handle) = self.llm.abort_handle.take() {
                    handle.abort();
                    self.llm.request_state = LlmRequestState::Idle;
                    self.append_chat_message(ChatMessage {
                        role: "system".to_string(),
                        content: "⚠️ LLM generation cancelled by user (Ctrl+C)".to_string(),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                    self.add_notification("LLM cancelled".to_string(), NotifLevel::Warning);
                    return Ok(());
                }
            }
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
            (KeyCode::F(4), _) | (KeyCode::Char('m'), KeyModifiers::ALT) => {
                self.toggle_mouse_capture()?;
                return Ok(());
            }
            (KeyCode::BackTab, _) => {
                self.cycle_focus_backward();
                return Ok(());
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
    
    pub async fn reload_sidebar(&mut self) -> Result<()> {
        let _ = DatabaseQueries::deduplicate_targets_and_findings(&self.db.conn());
        let targets = DatabaseQueries::get_all_targets(&self.db.conn())?;
        self.sidebar_nodes.clear();

        if let Some(act_id) = self.active_target_id {
            if !targets.iter().any(|t| t.id == act_id) {
                self.active_target_id = targets.first().map(|t| t.id);
            }
        }
        
        for target in targets {
            let findings = DatabaseQueries::get_findings_for_target(&self.db.conn(), target.id)?;
            let mut by_severity: std::collections::HashMap<String, Vec<Finding>> = std::collections::HashMap::new();
            for finding in findings {
                // If finding references an evidence file in workspace/evidence, verify it still exists on disk.
                // If user deleted the file manually, automatically prune the orphaned finding from the DB.
                if let Some(ref ep) = finding.evidence_path {
                    if ep.starts_with("workspace/evidence") || ep.starts_with("evidence") {
                        let rel = ep.trim_start_matches("workspace/").trim_start_matches("workspace\\");
                        let sub = rel.trim_start_matches("evidence/").trim_start_matches("evidence\\");
                        let full_p = self.session_meta.workspace_path().join("evidence").join(sub);
                        let direct_p = self.session_meta.path().join(ep);
                        if !full_p.exists() && !direct_p.exists() {
                            let _ = DatabaseQueries::delete_finding(&self.db.conn(), finding.id);
                            continue;
                        }
                    }
                }
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
    
    pub fn cycle_focus_backward(&mut self) {
        if self.active_tab > 0 {
            if self.active_tab == 1 {
                self.active_tab = 0;
                self.ui_state.focus = WindowFocus::Input;
            } else {
                self.active_tab -= 1;
            }
            return;
        }

        match self.ui_state.focus {
            WindowFocus::Input => self.ui_state.focus = WindowFocus::ChatLog,
            WindowFocus::ChatLog if !self.ui_state.sidebar_collapsed => {
                self.ui_state.focus = WindowFocus::Sidebar;
            }
            WindowFocus::ChatLog | WindowFocus::Sidebar if !self.tabs.is_empty() => {
                self.active_tab = self.tabs.len();
            }
            WindowFocus::ChatLog => self.ui_state.focus = WindowFocus::Input,
            WindowFocus::Sidebar => self.ui_state.focus = WindowFocus::Input,
        }
    }
    
    pub async fn show_targets_table(&mut self) -> Result<()> {
        let targets = DatabaseQueries::get_all_targets(&self.db.conn())?;
        let mut table = String::from("ID | IP/Host | Ports | Findings\n");
        table.push_str("---|---------|-------|---------\n");
        for target in targets {
            let ports_count = target.ports_json.as_ref()
                .and_then(|p| serde_json::from_str::<Vec<crate::db::models::PortInfo>>(p).ok())
                .map(|v| v.len())
                .unwrap_or(0);
            let findings_count = DatabaseQueries::get_findings_for_target(&self.db.conn(), target.id)?.len();
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
    
    pub async fn show_findings(&mut self, target_id: Option<i64>) -> Result<()> {
        let target_id = target_id.or(self.active_target_id);
        if let Some(id) = target_id {
            let findings = DatabaseQueries::get_findings_for_target(&self.db.conn(), id)?;
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

    pub async fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> Result<()> {
        if !self.mouse_capture_enabled {
            return Ok(());
        }

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
                    let suggestion_height = if self.llm.suggestion_visible { 5 } else { 0 };
                    let approval_height = if self.pending_exploit_approval.is_some() { 5 } else { 0 };
                    let panels_height = suggestion_height + approval_height;
                    let input_area_start = input_row.saturating_sub(panels_height as u16);

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