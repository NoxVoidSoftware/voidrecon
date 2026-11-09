//! LLM conversation flow
use crate::app::state::{App, NotifLevel};
use crate::db::DatabaseQueries;
use crate::llm::{ChatMessage, LlmProvider, LlmRequestState, POST_EXEC_PROMPT, build_system_prompt, parse_llm_response};
use crate::tools::get_tool_definitions;
use crate::llm::types::ToolCall;
use anyhow::Result;

impl App {
    pub fn build_chat_messages(&self) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        for msg in &self.llm.chat_log {
            match msg.role.as_str() {
                "user" => {
                    let content = if msg.content.starts_with("? ") {
                        format!("User query: {}", msg.content.trim_start_matches("? "))
                    } else if msg.content.starts_with("$ ") {
                        format!("Executed command: {}", msg.content)
                    } else if msg.content.starts_with("[stdin] >") {
                        format!("User input to process: {}", msg.content)
                    } else {
                        msg.content.clone()
                    };
                    messages.push(ChatMessage {
                        role: "user".to_string(),
                        content,
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
                "output" => {
                    // Provide recent command outputs so the LLM sees what actually happened and what worked or failed
                    let content = if msg.content.len() > 3000 {
                        let lines: Vec<&str> = msg.content.lines().collect();
                        if lines.len() > 50 {
                            let head = lines[..30].join("\n");
                            let tail = lines[lines.len().saturating_sub(15)..].join("\n");
                            format!("{}\n... [truncated {} lines] ...\n{}", head, lines.len() - 45, tail)
                        } else {
                            format!("{}... [truncated]", &msg.content[..3000])
                        }
                    } else {
                        msg.content.clone()
                    };
                    messages.push(ChatMessage {
                        role: "user".to_string(),
                        content: format!("Command output:\n{}", content),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
                "assistant" => {
                    if !msg.content.is_empty() || msg.tool_calls.is_some() {
                        let content = if msg.content.len() > 1500 {
                            format!("{}... [truncated]", &msg.content[..1500])
                        } else {
                            msg.content.clone()
                        };
                        messages.push(ChatMessage {
                            role: "assistant".to_string(),
                            content,
                            tool_calls: msg.tool_calls.clone(),
                            tool_call_id: None,
                        });
                    }
                }
                "tool" => {
                    let content = if msg.content.len() > 1500 {
                        format!("{}... [truncated]", &msg.content[..1500])
                    } else {
                        msg.content.clone()
                    };
                    messages.push(ChatMessage {
                        role: "tool".to_string(),
                        content,
                        tool_calls: None,
                        tool_call_id: msg.tool_call_id.clone(),
                    });
                }
                "system" => {
                    // Persisted system messages are compaction summaries; UI-only notices are skipped.
                    if msg.content.starts_with("[Compacted earlier conversation summary]") {
                        messages.push(ChatMessage {
                            role: "system".to_string(),
                            content: msg.content.clone(),
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }
                }
                _ => {}
            }
        }
        messages
    }

    /// Build per-target findings and important facts mirroring the sidebar.
    fn build_sidebar_context(
        &self,
        all_targets: &[crate::db::models::Target],
    ) -> (
        Vec<(i64, Vec<crate::db::models::Finding>)>,
        Vec<(i64, Vec<crate::db::models::ImportantFact>)>,
    ) {
        let conn = self.db.conn();
        let mut findings = Vec::with_capacity(all_targets.len());
        let mut facts = Vec::with_capacity(all_targets.len());
        for t in all_targets {
            let f = DatabaseQueries::get_findings_for_target(&conn, t.id).unwrap_or_default();
            let fa = DatabaseQueries::get_important_facts(&conn, t.id).unwrap_or_default();
            findings.push((t.id, f));
            facts.push((t.id, fa));
        }
        (findings, facts)
    }

    /// Token budget above which the conversation history is compacted.
    /// 1/3 of the model context when known, otherwise 200k.
    fn compaction_limit(&self) -> usize {
        self.config
            .active_model_context_length()
            .map(|c| (c as usize) / 3)
            .unwrap_or(200_000)
    }

    fn estimate_text_tokens(text: &str) -> usize {
        text.chars().count() / 4 + 8
    }

    fn estimate_chat_tokens(chat_log: &[ChatMessage]) -> usize {
        chat_log
            .iter()
            .map(|m| Self::estimate_text_tokens(&m.content))
            .sum()
    }

    /// Approximate token cost of the tool schemas sent with every request.
    fn tool_definitions_tokens() -> usize {
        serde_json::to_string(&get_tool_definitions())
            .map(|s| Self::estimate_text_tokens(&s))
            .unwrap_or(0)
    }

    /// When the history grows past the token budget, summarize the oldest part
    /// with the LLM and replace it with a single summary message.
    /// `overhead_tokens` accounts for the system prompt and tool schemas.
    async fn maybe_compact_history(&mut self, overhead_tokens: usize) -> Result<()> {
        let limit = self.compaction_limit();
        if Self::estimate_chat_tokens(&self.llm.chat_log) + overhead_tokens <= limit {
            return Ok(());
        }

        // Keep the most recent messages verbatim; summarize everything older.
        const KEEP_RECENT: usize = 10;
        if self.llm.chat_log.len() <= KEEP_RECENT + 2 {
            return Ok(());
        }
        let split = self.llm.chat_log.len() - KEEP_RECENT;

        let transcript = {
            let mut t = String::new();
            for m in &self.llm.chat_log[..split] {
                t.push_str(&format!("[{}]: {}\n", m.role, m.content));
            }
            t
        };

        let request = crate::llm::types::ChatCompletionRequest {
            model: self.config.active_model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: "You compact penetration-testing session conversations. Produce a dense factual summary preserving: targets and open ports, vulnerabilities/findings, saved evidence/artifacts/exploits, credentials, important facts, commands run and their key results, user instructions/preferences, and the current objective. Output only the summary.".to_string(),
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: format!("Summarize this conversation:\n\n{}", transcript),
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            tools: None,
            tool_choice: None,
            stream: false,
            temperature: 0.2,
            max_tokens: 1500,
        };

        let client = self.llm_client.clone();
        match client.complete_chat(request).await {
            Ok(summary) if !summary.trim().is_empty() => {
                let summary_msg = ChatMessage {
                    role: "system".to_string(),
                    content: format!(
                        "[Compacted earlier conversation summary]\n{}",
                        summary.trim()
                    ),
                    tool_calls: None,
                    tool_call_id: None,
                };
                let mut new_log = Vec::with_capacity(1 + self.llm.chat_log.len() - split);
                new_log.push(summary_msg);
                new_log.extend_from_slice(&self.llm.chat_log[split..]);
                self.llm.chat_log = new_log;
                self.save_chat_history();
                self.add_notification(
                    "Conversation history compacted".to_string(),
                    NotifLevel::Info,
                );
            }
            Ok(_) => {}
            Err(e) => {
                self.add_notification(
                    format!("History compaction failed: {}", e),
                    NotifLevel::Warning,
                );
            }
        }
        Ok(())
    }

    pub async fn send_llm_query(&mut self, query: &str) -> Result<()> {
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
            DatabaseQueries::get_target(&self.db.conn(), id)?
        } else {
            None
        };
        
        let (recent_commands, command_count) = if let Some(target) = &active_target {
            let r = DatabaseQueries::get_recent_commands(&self.db.conn(), target.id, 3)?;
            let c = DatabaseQueries::get_command_count(&self.db.conn(), target.id)?;
            (r, c)
        } else {
            (Vec::new(), 0)
        };
        let targets_count = DatabaseQueries::get_targets_count(&self.db.conn())?;
        let all_targets = DatabaseQueries::get_all_targets(&self.db.conn())?;
        let (all_findings, all_facts) = self.build_sidebar_context(&all_targets);

        let system_prompt = build_system_prompt(
            &self.session_meta.name,
            &self.session_meta.created_at.to_rfc3339(),
            targets_count,
            &all_targets,
            active_target.as_ref(),
            &all_findings,
            &all_facts,
            &recent_commands,
            command_count,
            recent_commands.first().map(|c| c.executed_at.to_rfc3339()),
            false,
            Some(&self.session_meta.workspace_path()),
        );

        let overhead = Self::estimate_text_tokens(&system_prompt) + Self::tool_definitions_tokens();
        self.maybe_compact_history(overhead).await?;

        let mut messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: system_prompt,
                tool_calls: None,
                tool_call_id: None,
            },
        ];

        // Include the full conversation history (compacted periodically to stay within limits)
        let mut history = self.build_chat_messages();
        messages.append(&mut history);
        
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
        
        self.llm.request_state = LlmRequestState::Streaming;
        self.llm.response_buffer.clear();
        self.llm.pending_request = Some(query.to_string());
        
        let tx = self.event_tx.clone();
        let client = self.llm_client.clone();
        
        let handle = tokio::spawn(async move {
            let _ = client.stream_chat(request, tx, request_id).await;
        });
        self.llm.abort_handle = Some(handle.abort_handle());
        
        Ok(())
    }
    
    pub async fn send_post_exec_analysis(&mut self, cmd: &str, output: &str) -> Result<()> {
        if self.llm_client.provider().requires_api_key() && self.config.api_key.trim().is_empty() {
            return Ok(());
        }

        let active_target = if let Some(id) = self.active_target_id {
            DatabaseQueries::get_target(&self.db.conn(), id).unwrap_or(None)
        } else {
            None
        };
        let (recent_commands, command_count) = if let Some(target) = &active_target {
            let r = DatabaseQueries::get_recent_commands(&self.db.conn(), target.id, 3).unwrap_or_default();
            let c = DatabaseQueries::get_command_count(&self.db.conn(), target.id).unwrap_or(0);
            (r, c)
        } else {
            (Vec::new(), 0)
        };
        let targets_count = DatabaseQueries::get_targets_count(&self.db.conn()).unwrap_or(0);
        let all_targets = DatabaseQueries::get_all_targets(&self.db.conn()).unwrap_or_default();
        let (all_findings, all_facts) = self.build_sidebar_context(&all_targets);

        let base_prompt = build_system_prompt(
            &self.session_meta.name,
            &self.session_meta.created_at.to_rfc3339(),
            targets_count,
            &all_targets,
            active_target.as_ref(),
            &all_findings,
            &all_facts,
            &recent_commands,
            command_count,
            recent_commands.first().map(|c| c.executed_at.to_rfc3339()),
            true,
            Some(&self.session_meta.workspace_path()),
        );

        let system_prompt = format!(
            "{}\n\n## Post-Execution Recon & Analysis Mode\nYou are analyzing the output of a command that just completed.\nCRITICAL RULES:\n1. STRICTLY RESPECT USER INSTRUCTIONS: Review previous messages. If the user previously instructed NOT to do something or to avoid certain tools, techniques, flags, or endpoints, DO NOT attempt or suggest them.\n2. DO NOT DUPLICATE TARGETS OR FINDINGS: Use `update_target` on the active target rather than `add_target`. Only invoke `add_finding` if the vulnerability is genuine, novel, and not already listed above.\n3. RECORD DURABLE KNOWLEDGE: If the output confirms something worth remembering later (credentials, confirmed access, a key service detail or path), invoke `save_important_fact`. Be selective and skip transient/noisy output.\n4. SAVE EVIDENCE SELECTIVELY: Only save evidence/artifacts when the result is significant (confirmed vulnerability, key scan result, exploit artifact). Intermediate output is already archived and can be read on demand.\n5. Suggest the next logical pentest step that strictly complies with the user's directions.",
            base_prompt
        );

        let overhead = Self::estimate_text_tokens(&system_prompt) + Self::tool_definitions_tokens();
        self.maybe_compact_history(overhead).await?;

        let mut messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: system_prompt,
                tool_calls: None,
                tool_call_id: None,
            },
        ];

        let mut history = self.build_chat_messages();
        // The command output was just appended to the chat log; it is included explicitly
        // below with a larger truncation limit, so drop the duplicate history entry.
        if self.llm.chat_log.last().map(|m| m.role == "output").unwrap_or(false)
            && history.last().map(|m| m.content.starts_with("Command output:")).unwrap_or(false)
        {
            history.pop();
        }
        messages.append(&mut history);

        let truncated_output = if output.len() > 16_000 {
            let lines: Vec<&str> = output.lines().collect();
            if lines.len() > 80 {
                let head = lines[..50].join("\n");
                let tail = lines[lines.len().saturating_sub(25)..].join("\n");
                format!("{}\n... [truncated {} lines, full output archived to workspace] ...\n{}", head, lines.len() - 75, tail)
            } else {
                format!("{}... [truncated]", &output[..16_000])
            }
        } else {
            output.to_string()
        };

        messages.push(ChatMessage {
            role: "user".to_string(),
            content: format!("Command executed: {}\n\nOutput:\n{}\n\n{}", cmd, truncated_output, POST_EXEC_PROMPT),
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
            max_tokens: 1024,
        };
        
        self.llm.request_state = LlmRequestState::Streaming;
        self.llm.response_buffer.clear();
        
        let tx = self.event_tx.clone();
        let client = self.llm_client.clone();
        
        let handle = tokio::spawn(async move {
            let _ = client.stream_chat(request, tx, request_id).await;
        });
        self.llm.abort_handle = Some(handle.abort_handle());
        
        Ok(())
    }
    
    pub fn handle_llm_chunk(&mut self, request_id: u64, chunk: String) {
        // Ignore stale requests
        if request_id != self.llm.request_id {
            return;
        }
        self.llm.response_buffer.push_str(&chunk);
    }
    
    pub async fn handle_llm_tool_call(&mut self, request_id: u64, tool_call: ToolCall) -> Result<()> {
        // Ignore stale requests
        if request_id != self.llm.request_id {
            return Ok(());
        }
        self.llm.pending_tool_calls.push(tool_call);
        self.llm.request_state = LlmRequestState::ProcessingToolCalls;
        Ok(())
    }
    
    pub fn handle_llm_request_started(&mut self, request_id: u64) {
        if request_id == self.llm.request_id {
            self.llm.request_state = LlmRequestState::Streaming;
        }
    }
    
    pub fn handle_llm_request_failed(&mut self, request_id: u64, error: String) {
        if request_id == self.llm.request_id {
            self.llm.abort_handle = None;
            self.llm.request_state = LlmRequestState::Failed;
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
    
    pub async fn handle_llm_done(&mut self, request_id: u64) -> Result<()> {
        // Ignore stale requests
        if request_id != self.llm.request_id {
            return Ok(());
        }
        
        self.llm.abort_handle = None;
        self.llm.request_state = LlmRequestState::Completed;
        
        if !self.llm.pending_tool_calls.is_empty() {
            // Push assistant message with tool calls into chat log first (required by LLM APIs)
            let tool_calls = std::mem::take(&mut self.llm.pending_tool_calls);
            self.append_chat_message(ChatMessage {
                role: "assistant".to_string(),
                content: self.llm.response_buffer.clone(),
                tool_calls: Some(tool_calls.clone()),
                tool_call_id: None,
            });

            // Execute tool calls
            for tool_call in tool_calls {
                let result = match crate::tools::execute_tool_call(
                    &tool_call,
                    &self.db,
                    self.active_target_id,
                    &self.session_meta.workspace_path(),
                    &self.event_tx,
                ).await {
                    Ok(res) => res,
                    Err(e) => {
                        self.add_notification(
                            format!("Tool {} error: {}", tool_call.function.name, e),
                            NotifLevel::Error,
                        );
                        serde_json::json!({
                            "status": "error",
                            "error": e.to_string(),
                        })
                    }
                };

                if tool_call.function.name == "save_exploit_script" {
                    if let Some(path) = result.get("path").and_then(|v| v.as_str()) {
                        self.add_notification(
                            format!("Saved exploit to {}", path),
                            NotifLevel::Info,
                        );
                    }
                }

                if tool_call.function.name == "run_command" {
                    if let Ok(args) = serde_json::from_str::<serde_json::Value>(&tool_call.function.arguments) {
                        let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
                        let explanation = args.get("explanation").and_then(|v| v.as_str()).unwrap_or("");
                        if !cmd.is_empty() {
                            // Intercept reverse-shell listener commands (nc/ncat/socat) that would
                            // block the terminal, and spawn a dedicated listener tab instead.
                            let lower = cmd.to_lowercase();
                            let is_listener = lower.contains("nc -lv") || lower.contains("nc -l ")
                                || lower.contains("ncat -l") || lower.contains("socat tcp-listen")
                                || lower.contains("nc -lvp") || lower.contains("nc -lvnp");
                            if is_listener {
                                // extract port
                                let mut port: u16 = 4444;
                                for tok in cmd.split_whitespace() {
                                    if let Ok(p) = tok.parse::<u16>() {
                                        if p > 1024 { port = p; break; }
                                    }
                                }
                                let target_id = self.active_target_id.unwrap_or(0);
                                let target_ip = if target_id > 0 {
                                    DatabaseQueries::get_target(&self.db.conn(), target_id)
                                        .ok()
                                        .flatten()
                                        .map(|t| t.ip_host)
                                        .unwrap_or_else(|| "target".to_string())
                                } else {
                                    "active_target".to_string()
                                };
                                self.pending_exploit_approval = Some(crate::app::state::ExploitApproval {
                                    target_id,
                                    target_ip,
                                    kind: crate::app::state::ApprovalKind::ReverseShellListener {
                                        lhost: "0.0.0.0".to_string(),
                                        lport: port,
                                        shell_type: "bash".to_string(),
                                    },
                                    tool_call: tool_call.clone(),
                                });
                                self.add_notification(
                                    format!("Approval required for reverse shell on port {}", port),
                                    crate::app::state::NotifLevel::Warning,
                                );
                            } else {
                                self.llm.suggestion = Some(crate::llm::types::LlmSuggestion {
                                    command: Some(cmd.to_string()),
                                    explanation: explanation.to_string(),
                                });
                                self.llm.suggestion_visible = true;
                            }
                        }
                    }
                }

                if tool_call.function.name == "save_workspace_file" {
                    let target_id = result.get("target_id").and_then(|v| v.as_i64()).or(self.active_target_id).unwrap_or(0);
                    let target_ip = if target_id > 0 {
                        DatabaseQueries::get_target(&self.db.conn(), target_id)
                            .ok()
                            .flatten()
                            .map(|t| t.ip_host)
                            .unwrap_or_else(|| "target".to_string())
                    } else {
                        "active_target".to_string()
                    };

                    let folder = result.get("folder").and_then(|v| v.as_str()).unwrap_or("evidence").to_string();
                    let filename = result.get("filename").and_then(|v| v.as_str()).unwrap_or("file.txt").to_string();
                    let description = result.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let content = result.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();

                    self.pending_exploit_approval = Some(crate::app::state::ExploitApproval {
                        target_id,
                        target_ip,
                        kind: crate::app::state::ApprovalKind::SaveWorkspaceFile {
                            folder,
                            filename: filename.clone(),
                            description,
                            content,
                        },
                        tool_call: tool_call.clone(),
                    });
                    self.add_notification(
                        format!("Approval required to save file to {}", filename),
                        NotifLevel::Warning,
                    );
                    return Ok(());
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
                    let target = DatabaseQueries::get_target(&self.db.conn(), target_id)?;
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

            // If an action requires user approval or a command is suggested for user execution,
            // STOP querying the LLM and wait for user input!
            if self.llm.suggestion_visible || self.pending_exploit_approval.is_some() {
                self.llm.request_state = LlmRequestState::Completed;
                self.ui_state.log_at_bottom = true;
                return Ok(());
            }

            // Continue conversation with tool results only when no pending user action
            self.continue_llm_conversation().await?;
        } else if self.llm.post_exec_mode {
            // Post-execution analysis complete
            self.llm.post_exec_mode = false;
            // Add LLM comment to chat log
            self.append_chat_message(ChatMessage {
                role: "assistant".to_string(),
                content: self.llm.response_buffer.clone(),
                tool_calls: None,
                tool_call_id: None,
            });
            
            // Check for new asset detection (simplified)
            if self.llm.response_buffer.to_lowercase().contains("detected") {
                self.add_notification("New assets detected - check LLM output".to_string(), NotifLevel::Info);
            }
        } else {
            // Regular LLM query / suggestion
            let suggestion = parse_llm_response(&self.llm.response_buffer)?;
            let content_display = if !suggestion.explanation.is_empty() {
                suggestion.explanation.clone()
            } else {
                self.llm.response_buffer.clone()
            };
            self.append_chat_message(ChatMessage {
                role: "assistant".to_string(),
                content: content_display,
                tool_calls: None,
                tool_call_id: None,
            });
            if suggestion.command.is_some() {
                self.llm.suggestion = Some(suggestion);
                self.llm.suggestion_visible = true;
            }
            self.ui_state.log_at_bottom = true;
        }
        
        self.llm.response_buffer.clear();
        self.llm.pending_request = None;
        
        Ok(())
    }
    
    pub async fn continue_llm_conversation(&mut self) -> Result<()> {
        let mut messages = Vec::new();

        // System prompt with full context
        let active_target = if let Some(id) = self.active_target_id {
            DatabaseQueries::get_target(&self.db.conn(), id).unwrap_or(None)
        } else {
            None
        };
        let (recent_commands, command_count) = if let Some(target) = &active_target {
            let r = DatabaseQueries::get_recent_commands(&self.db.conn(), target.id, 3).unwrap_or_default();
            let c = DatabaseQueries::get_command_count(&self.db.conn(), target.id).unwrap_or(0);
            (r, c)
        } else {
            (Vec::new(), 0)
        };
        let targets_count = DatabaseQueries::get_targets_count(&self.db.conn()).unwrap_or(0);
        let all_targets = DatabaseQueries::get_all_targets(&self.db.conn()).unwrap_or_default();
        let (all_findings, all_facts) = self.build_sidebar_context(&all_targets);
        let system_prompt = build_system_prompt(
            &self.session_meta.name,
            &self.session_meta.created_at.to_rfc3339(),
            targets_count,
            &all_targets,
            active_target.as_ref(),
            &all_findings,
            &all_facts,
            &recent_commands,
            command_count,
            recent_commands.first().map(|c| c.executed_at.to_rfc3339()),
            true,
            Some(&self.session_meta.workspace_path()),
        );

        let overhead = Self::estimate_text_tokens(&system_prompt) + Self::tool_definitions_tokens();
        self.maybe_compact_history(overhead).await?;

        messages.push(ChatMessage {
            role: "system".to_string(),
            content: system_prompt,
            tool_calls: None,
            tool_call_id: None,
        });

        // Full conversation history (already compacted to fit the token budget).
        let mut history = self.build_chat_messages();
        messages.append(&mut history);
        
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
        
        self.llm.request_state = LlmRequestState::Streaming;
        self.llm.response_buffer.clear();
        
        let tx = self.event_tx.clone();
        let client = self.llm_client.clone();
        
        let handle = tokio::spawn(async move {
            let _ = client.stream_chat(request, tx, request_id).await;
        });
        self.llm.abort_handle = Some(handle.abort_handle());
        
        Ok(())
    }
    

}
