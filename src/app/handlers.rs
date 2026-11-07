//! Event handlers for voidrecon

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::Backend;
use std::time::Instant;

use crate::app::events::AppEvent;
use crate::app::state::{App, InputMode, NotifLevel, SidebarNode};
use crate::db::DatabaseQueries;
use crate::db::models::Finding;
use crate::llm::{ChatMessage, LlmRequestState, build_system_prompt, get_tool_definitions, parse_llm_response, POST_EXEC_PROMPT};
use crate::llm::types::ToolCall;
use crate::pty::tab::PtyTab;
use crate::ui::render_ui;

impl App {
    pub async fn run<B: Backend>(&mut self, terminal: &mut ratatui::Terminal<B>) -> Result<()> {
        loop {
            // Render
            terminal.draw(|f| render_ui(f, self))?;
            
            // Handle events
            if let Some(event) = self.event_rx.recv().await {
                self.handle_event(event).await?;
            }
            
            // Check for quit
            if self.ui_state.should_quit {
                break;
            }
        }
        Ok(())
    }
    
    async fn handle_event(&mut self, event: AppEvent) -> Result<()> {
        match event {
            AppEvent::Key(key) => self.handle_key(key).await?,
            AppEvent::PtyData(tab_id, data) => self.handle_pty_data(tab_id, data).await?,
            AppEvent::PtyExited(tab_id, exit_code) => self.handle_pty_exited(tab_id, exit_code).await?,
            AppEvent::LlmChunk(request_id, chunk) => self.handle_llm_chunk(request_id, chunk),
            AppEvent::LlmToolCall(request_id, tool_call) => self.handle_llm_tool_call(request_id, tool_call).await?,
            AppEvent::LlmRequestStarted(request_id) => self.handle_llm_request_started(request_id),
            AppEvent::LlmRequestFailed(request_id, error) => self.handle_llm_request_failed(request_id, error),
            AppEvent::LlmDone(request_id) => self.handle_llm_done(request_id).await?,
            AppEvent::Tick => self.handle_tick(),
            AppEvent::Resize(cols, rows) => self.handle_resize(cols, rows).await?,
            AppEvent::Notification(msg, level) => self.add_notification(msg, level),
            AppEvent::SidebarReload => self.reload_sidebar().await?,
        }
        Ok(())
    }
    
    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Global shortcuts
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), KeyModifiers::CONTROL) | (KeyCode::Char(':'), _) if self.input_buffer == "quit" => {
                self.ui_state.should_quit = true;
                return Ok(());
            }
            (KeyCode::Char('l'), KeyModifiers::CONTROL) => {
                self.input_mode = InputMode::LlmQuery;
                self.input_buffer.clear();
                return Ok(());
            }
            (KeyCode::F(1), _) => {
                self.ui_state.show_help = !self.ui_state.show_help;
                return Ok(());
            }
            (KeyCode::F(2), _) => {
                self.ui_state.show_model_picker = true;
                return Ok(());
            }
            (KeyCode::F(3), _) => {
                self.new_shell_tab().await?;
                return Ok(());
            }
            (KeyCode::Tab, _) => {
                self.cycle_tab();
                return Ok(());
            }
            (KeyCode::Esc, _) => {
                self.input_mode = InputMode::Shell;
                self.suggestion_visible = false;
                self.history_index = None;
                return Ok(());
            }
            (KeyCode::Char('['), _) => {
                self.ui_state.sidebar_collapsed = !self.ui_state.sidebar_collapsed;
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
    
    async fn handle_shell_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Enter => {
                let cmd = self.input_buffer.trim().to_string();
                if !cmd.is_empty() {
                    self.command_history.push_back(cmd.clone());
                    self.history_index = None;
                    self.input_buffer.clear();
                    
                    // Handle special commands
                    if cmd.starts_with(':') {
                        self.handle_special_command(&cmd).await?;
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
                self.input_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
            }
            KeyCode::Up => {
                if let Some(idx) = self.history_index {
                    if idx > 0 {
                        self.history_index = Some(idx - 1);
                        self.input_buffer = self.command_history[idx - 1].clone();
                    }
                } else if !self.command_history.is_empty() {
                    self.history_index = Some(self.command_history.len() - 1);
                    self.input_buffer = self.command_history.back().unwrap().clone();
                }
                // Scroll up in log
                if self.ui_state.log_at_bottom {
                    self.ui_state.log_at_bottom = false;
                }
            }
            KeyCode::Down => {
                if let Some(idx) = self.history_index {
                    if idx + 1 < self.command_history.len() {
                        self.history_index = Some(idx + 1);
                        self.input_buffer = self.command_history[idx + 1].clone();
                    } else {
                        self.history_index = None;
                        self.input_buffer.clear();
                    }
                }
                // Scroll down in log
                if !self.ui_state.log_at_bottom {
                    // Check if we reached bottom - would need viewport height
                }
            }
            KeyCode::PageUp => {
                self.ui_state.log_at_bottom = false;
                // Scroll by viewport - would need viewport height
            }
            KeyCode::PageDown => {
                // Scroll by viewport
            }
            _ => {}
        }
        Ok(())
    }
    
    async fn handle_llm_query_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Enter => {
                let query = self.input_buffer.trim().to_string();
                if !query.is_empty() {
                    self.input_buffer.clear();
                    self.input_mode = InputMode::Shell;
                    self.send_llm_query(&query).await?;
                }
            }
            KeyCode::Char(c) => {
                self.input_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Shell;
                self.input_buffer.clear();
            }
            _ => {}
        }
        Ok(())
    }
    
    async fn handle_edit_command_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Enter => {
                let cmd = self.input_buffer.trim().to_string();
                if !cmd.is_empty() {
                    self.command_history.push_back(cmd.clone());
                    self.input_buffer.clear();
                    self.input_mode = InputMode::Shell;
                    self.suggestion_visible = false;
                    if self.active_tab == 0 {
                        self.execute_workspace_command(&cmd).await?;
                    } else {
                        self.send_to_pty(&cmd).await?;
                    }
                }
            }
            KeyCode::Char(c) => {
                self.input_buffer.push(c);
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
            }
            KeyCode::Esc => {
                self.input_mode = InputMode::Shell;
                self.suggestion_visible = false;
            }
            _ => {}
        }
        Ok(())
    }
    
    async fn handle_special_command(&mut self, cmd: &str) -> Result<()> {
        let parts: Vec<&str> = cmd.splitn(2, ' ').collect();
        match parts[0] {
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
                        DatabaseQueries::add_finding(
                            self.db.conn(),
                            target_id,
                            "note",
                            parts[1],
                            "info",
                            None,
                            Some(100),
                            None,
                        )?;
                        self.add_notification("Note added".to_string(), NotifLevel::Info);
                        self.reload_sidebar().await?;
                    }
                }
            }
            ":models" => {
                self.ui_state.show_model_picker = true;
            }
            ":newtab" => {
                self.new_shell_tab().await?;
            }
            ":quit" => {
                self.ui_state.should_quit = true;
            }
            ":closeshell" => {
                self.close_tab(self.active_tab).await?;
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
        self.chat_log.push(ChatMessage {
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
        self.chat_log.push(ChatMessage {
            role: "assistant".to_string(),
            content: output.clone(),
            tool_calls: None,
            tool_call_id: None,
        });
        
        // Trigger post-execution analysis
        self.post_exec_mode = true;
        self.send_post_exec_analysis(cmd, &output).await?;
        
        // Reload sidebar
        self.reload_sidebar().await?;
        
        Ok(())
    }
    
    async fn run_command_capture(&self, cmd: &str) -> Result<String> {
        let output = tokio::process::Command::new("cmd")
            .args(["/C", cmd])
            .output()
            .await
            .context("Failed to execute command")?;
        
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        
        if stderr.is_empty() {
            Ok(stdout.to_string())
        } else {
            Ok(format!("{}\n{}", stdout, stderr))
        }
    }
    
    async fn send_to_pty(&mut self, cmd: &str) -> Result<()> {
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.write(format!("{}\n", cmd).as_bytes())?;
        }
        Ok(())
    }
    
    async fn send_llm_query(&mut self, query: &str) -> Result<()> {
        if self.config.api_key.is_empty() {
            self.add_notification("OpenRouter API key not configured".to_string(), NotifLevel::Error);
            return Ok(());
        }
        
        // Build context
        let active_target = if let Some(id) = self.active_target_id {
            DatabaseQueries::get_target(self.db.conn(), id)?
        } else {
            self.add_notification("No active target selected".to_string(), NotifLevel::Warning);
            return Ok(());
        };
        
        let active_target = active_target.context("Active target not found")?;
        let findings = DatabaseQueries::get_findings_for_target(self.db.conn(), active_target.id)?;
        let recent_commands = DatabaseQueries::get_recent_commands(self.db.conn(), active_target.id, 3)?;
        let command_count = DatabaseQueries::get_command_count(self.db.conn(), active_target.id)?;
        let targets_count = DatabaseQueries::get_targets_count(self.db.conn())?;
        
        let system_prompt = build_system_prompt(
            &self.session_meta.name,
            &self.session_meta.created_at.to_rfc3339(),
            targets_count,
            &active_target,
            &findings,
            &recent_commands,
            command_count,
            recent_commands.first().map(|c| c.executed_at.to_rfc3339()),
        );
        
        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: system_prompt,
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: "user".to_string(),
                content: query.to_string(),
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
            max_tokens: 2048,
        };
        
        self.llm_request_state = LlmRequestState::Streaming;
        self.llm_response_buffer.clear();
        self.pending_llm_request = Some(query.to_string());
        
        let tx = self.event_tx.clone();
        let client = self.llm_client.clone();
        
        tokio::spawn(async move {
            let _ = client.stream_chat(request, tx, request_id).await;
        });
        
        Ok(())
    }
    
    async fn send_post_exec_analysis(&mut self, cmd: &str, output: &str) -> Result<()> {
        if self.config.api_key.is_empty() {
            return Ok(());
        }
        
        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: "You are a security analyst. Analyze command output for interesting findings.".to_string(),
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
        
        tokio::spawn(async move {
            let _ = client.stream_chat(request, tx, request_id).await;
        });
        
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
            self.llm_request_state = LlmRequestState::Failed;
            self.add_notification(format!("LLM request failed: {}", error), NotifLevel::Error);
        }
    }
    
    async fn handle_llm_done(&mut self, request_id: u64) -> Result<()> {
        // Ignore stale requests
        if request_id != self.llm_request_id {
            return Ok(());
        }
        
        self.llm_request_state = LlmRequestState::Completed;
        
        if !self.pending_tool_calls.is_empty() {
            // Collect tool calls first to avoid borrow issues
            let tool_calls = std::mem::take(&mut self.pending_tool_calls);
            
            // Execute tool calls
            for tool_call in tool_calls {
                let result = crate::llm::tools::execute_tool_call(
                    &tool_call,
                    &self.db,
                    self.active_target_id,
                    &self.event_tx,
                ).await?;
                
                // Handle special tool results
                if tool_call.function.name == "open_reverse_shell_tab" {
                    if let Some(action) = result.get("action").and_then(|v| v.as_str()) {
                        if action == "spawn_listener_tab" {
                            let lport = result.get("lport").and_then(|v| v.as_u64()).unwrap_or(4444) as u16;
                            self.spawn_listener_tab(lport).await?;
                        }
                    }
                }
                
                // Add tool result to conversation for next LLM call
                self.chat_log.push(ChatMessage {
                    role: "tool".to_string(),
                    content: result.to_string(),
                    tool_calls: None,
                    tool_call_id: Some(tool_call.id),
                });
            }
            
            // Continue conversation with tool results
            self.continue_llm_conversation().await?;
        } else if self.post_exec_mode {
            // Post-execution analysis complete
            self.post_exec_mode = false;
            // Add LLM comment to chat log
            self.chat_log.push(ChatMessage {
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
            // Regular LLM suggestion
            let suggestion = parse_llm_response(&self.llm_response_buffer)?;
            self.suggestion = Some(suggestion);
            self.suggestion_visible = true;
        }
        
        self.llm_response_buffer.clear();
        self.pending_llm_request = None;
        
        Ok(())
    }
    
    async fn continue_llm_conversation(&mut self) -> Result<()> {
        let messages = self.chat_log.clone();
        
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
        
        tokio::spawn(async move {
            let _ = client.stream_chat(request, tx, request_id).await;
        });
        
        Ok(())
    }
    
    async fn handle_pty_data(&mut self, _tab_id: usize, _data: Vec<u8>) -> Result<()> {
        // Data already processed by reader thread
        // Just ensure UI updates - we could force a redraw here if needed
        Ok(())
    }
    
    async fn handle_pty_exited(&mut self, tab_id: usize, exit_code: i32) -> Result<()> {
        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == tab_id) {
            tab.mark_exited(exit_code);
        }
        Ok(())
    }
    
    fn handle_tick(&mut self) {
        // Clean up old notifications
        let now = Instant::now();
        self.notifications.retain(|n| now.duration_since(n.created_at).as_secs() < 5);
        if self.notifications.len() > 3 {
            self.notifications.drain(0..self.notifications.len() - 3);
        }
        self.last_tick = now;
    }
    
    async fn handle_resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        // Resize all PTY tabs
        for tab in &mut self.tabs {
            tab.resize(cols, rows)?;
        }
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
            
            self.sidebar_nodes.push(SidebarNode {
                target,
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
        self.active_tab = self.tabs.len() - 1;
        Ok(())
    }
    
    async fn spawn_listener_tab(&mut self, lport: u16) -> Result<()> {
        let tab = PtyTab::new_listener(self.next_tab_id, lport, self.event_tx.clone())?;
        self.next_tab_id += 1;
        self.tabs.push(tab);
        self.active_tab = self.tabs.len() - 1;
        Ok(())
    }
    
    async fn close_tab(&mut self, index: usize) -> Result<()> {
        if index < self.tabs.len() {
            self.tabs.remove(index);
            if self.active_tab >= self.tabs.len() && !self.tabs.is_empty() {
                self.active_tab = self.tabs.len() - 1;
            }
        }
        Ok(())
    }
    
    fn cycle_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
        }
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
        self.chat_log.push(ChatMessage {
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
            self.chat_log.push(ChatMessage {
                role: "assistant".to_string(),
                content: output,
                tool_calls: None,
                tool_call_id: None,
            });
        }
        Ok(())
    }
    
    fn add_notification(&mut self, message: String, level: NotifLevel) {
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