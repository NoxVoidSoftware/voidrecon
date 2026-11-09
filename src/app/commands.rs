//! Special commands and workspace command execution
use crate::app::state::{App, NotifLevel};
use crate::app::events::AppEvent;
use crate::db::DatabaseQueries;
use crate::llm::ChatMessage;
use anyhow::{Context, Result};
use std::time::Instant;

impl App {
    pub async fn handle_special_command(&mut self, cmd: &str) -> Result<()> {
        let parts: Vec<&str> = cmd.splitn(2, ' ').collect();
        match parts[0] {
            ":target" => {
                if parts.len() > 1 {
                    let input = parts[1].trim();
                    let target = if let Ok(id) = input.parse::<i64>() {
                        DatabaseQueries::get_target(&self.db.conn(), id)?
                    } else {
                        DatabaseQueries::get_target_by_ip(&self.db.conn(), input)?
                    };
                    if let Some(target) = target {
                        self.active_target_id = Some(target.id);
                        self.reload_sidebar().await?;
                        self.add_notification(format!("Active target: {} (ID: {})", target.ip_host, target.id), NotifLevel::Info);
                    } else {
                        let new_id = DatabaseQueries::add_target(&self.db.conn(), input, None, None)?;
                        self.active_target_id = Some(new_id);
                        self.reload_sidebar().await?;
                        self.add_notification(format!("Added & activated target: {} (ID: {})", input, new_id), NotifLevel::Info);
                    }
                } else if let Some(target_id) = self.active_target_id {
                    let _target_res = {
                        let conn = self.db.conn();
                        DatabaseQueries::get_target(&conn, target_id)?
                    };
                    if let Some(target) = _target_res {
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
                            &self.db.conn(),
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
                            if let Ok(client) = crate::llm::LlmClient::from_config(&self.config) {
                        self.llm_client = client;
                    } else {
                        self.add_notification("Failed to rebuild LLM client".to_string(), NotifLevel::Error);
                    }
                            self.add_notification("Switched provider to Ollama (Local)".to_string(), NotifLevel::Info);
                            self.trigger_fetch_models();
                        }
                        "custom" => {
                            let url = subparts.get(1).map(|u| u.to_string());
                            self.config.set_provider(crate::config::ProviderKind::Custom, url);
                            let _ = self.config.save();
                            if let Ok(client) = crate::llm::LlmClient::from_config(&self.config) {
                        self.llm_client = client;
                    } else {
                        self.add_notification("Failed to rebuild LLM client".to_string(), NotifLevel::Error);
                    }
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
            ":mouse" => {
                if parts.len() > 1 {
                    match parts[1].trim() {
                        "on" | "enable" | "1" => {
                            if !self.mouse_capture_enabled {
                                self.toggle_mouse_capture()?;
                            }
                        }
                        "off" | "disable" | "0" => {
                            if self.mouse_capture_enabled {
                                self.toggle_mouse_capture()?;
                            }
                        }
                        _ => {
                            self.toggle_mouse_capture()?;
                        }
                    }
                } else {
                    self.toggle_mouse_capture()?;
                }
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
    
    pub fn handle_command_started(&mut self, request_id: u64, cmd: String) {
        self.command_run.running = Some((request_id, cmd, Instant::now()));
        self.command_run.output.clear();
        self.command_run.raw.clear();
    }

    pub fn handle_command_chunk(&mut self, request_id: u64, chunk: String) {
        if let Some((cur_id, _, _)) = &self.command_run.running {
            if *cur_id != request_id {
                return;
            }
        } else {
            return;
        }

        // Cap raw buffer to prevent unbounded memory growth during large scans
        if self.command_run.raw.len() > 100_000 {
            let keep_from = self.command_run.raw.len() - 80_000;
            self.command_run.raw = self.command_run.raw[keep_from..].to_string();
        }
        self.command_run.raw.push_str(&chunk);

        let sanitized = crate::ui::layout::sanitize_terminal_output(&self.command_run.raw);
        let mut lines = Vec::new();
        for line in sanitized.lines() {
            let trimmed = line.trim_end_matches('\r');
            if trimmed.len() > 160 {
                for chunk_str in trimmed.as_bytes().chunks(160) {
                    if let Ok(s) = std::str::from_utf8(chunk_str) {
                        lines.push(s.to_string());
                    }
                }
            } else {
                lines.push(trimmed.to_string());
            }
        }

        // Retain last 100 lines for live preview box
        if lines.len() > 100 {
            let start = lines.len() - 100;
            lines = lines[start..].to_vec();
        }
        self.command_run.output = lines;

        if self.ui_state.log_at_bottom {
            self.ui_state.log_scroll = usize::MAX;
        }
    }

    pub async fn handle_command_completed(&mut self, request_id: u64, result: crate::app::events::CommandResult) -> Result<()> {
        if let Some((cur_id, _, _)) = &self.command_run.running {
            if *cur_id != request_id {
                return Ok(());
            }
        }
        self.command_run.running = None;
        self.command_run.abort_handle = None;
        self.command_run.stdin_tx = None;
        self.command_run.output.clear();
        self.command_run.raw.clear();

        let target_id = self.active_target_id;

        // Save to database with full output artifact
        let _ = DatabaseQueries::add_chat_message(&self.db.conn(), target_id, "user", &result.cmd);
        let _ = DatabaseQueries::add_chat_message(&self.db.conn(), target_id, "assistant", &result.output);
        let _ = DatabaseQueries::add_command(
            &self.db.conn(),
            &self.session_meta.path(),
            target_id,
            &result.cmd,
            Some(&result.output),
            Some(result.exit_code),
            Some(result.duration_ms),
        );

        // Add output to chat log
        self.append_chat_message(ChatMessage {
            role: "output".to_string(),
            content: result.output.clone(),
            tool_calls: None,
            tool_call_id: None,
        });
        self.ui_state.log_at_bottom = true;

        // Auto-parse scan outputs (e.g. nmap) to update target and open ports immediately
        let parse_result = {
            let conn = self.db.conn();
            DatabaseQueries::parse_nmap_and_update_target(
                &conn,
                &result.cmd,
                &result.output,
                self.active_target_id,
            )
        };
        if let Ok(Some(discovered_target_id)) = parse_result {
            self.active_target_id = Some(discovered_target_id);
            self.add_notification(
                "Recon engine: Discovered target and updated open ports in DB".to_string(),
                NotifLevel::Info,
            );
        }

        // Reload sidebar
        self.reload_sidebar().await?;

        // Trigger post-execution analysis
        self.llm.post_exec_mode = true;
        self.send_post_exec_analysis(&result.cmd, &result.output).await?;

        Ok(())
    }

    pub async fn execute_workspace_command(&mut self, cmd: &str) -> Result<()> {
        let cmd_str = cmd.trim().to_string();
        if cmd_str.is_empty() {
            return Ok(());
        }

        let req_id = self.next_request_id();

        // Add to chat log
        self.append_chat_message(ChatMessage {
            role: "user".to_string(),
            content: format!("$ {}", cmd_str),
            tool_calls: None,
            tool_call_id: None,
        });
        self.ui_state.log_at_bottom = true;

        self.command_run.running = Some((req_id, cmd_str.clone(), Instant::now()));
        self.command_run.output.clear();
        self.command_run.raw.clear();

        let ws_path = self.session_meta.workspace_path();
        let _ = std::fs::create_dir_all(&ws_path);

        let tx = self.event_tx.clone();
        let cmd_for_task = cmd_str.clone();

        let _ = tx.send(AppEvent::CommandStarted(req_id, cmd_str.clone()));

        let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        self.command_run.stdin_tx = Some(stdin_tx);

        let handle = tokio::spawn(async move {
            let start = Instant::now();

            #[cfg(target_os = "windows")]
            let mut command = tokio::process::Command::new("cmd");
            #[cfg(target_os = "windows")]
            command.args(["/C", &cmd_for_task]);

            #[cfg(not(target_os = "windows"))]
            let mut command = tokio::process::Command::new("sh");
            #[cfg(not(target_os = "windows"))]
            command.args(["-c", &cmd_for_task]);

            command.current_dir(&ws_path);
            command.stdout(std::process::Stdio::piped());
            command.stderr(std::process::Stdio::piped());
            command.stdin(std::process::Stdio::piped());
            command.kill_on_drop(true);

            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(e) => {
                    let _ = tx.send(AppEvent::CommandCompleted(
                        req_id,
                        crate::app::events::CommandResult {
                            cmd: cmd_for_task,
                            output: format!("Failed to execute process: {}", e),
                            exit_code: 1,
                            duration_ms: 0,
                        },
                    ));
                    return;
                }
            };

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            let stdin = child.stdin.take();

            if let Some(mut stdin_writer) = stdin {
                tokio::spawn(async move {
                    use tokio::io::AsyncWriteExt;
                    while let Some(line) = stdin_rx.recv().await {
                        if stdin_writer.write_all(line.as_bytes()).await.is_err() {
                            break;
                        }
                        let _ = stdin_writer.flush().await;
                    }
                });
            }

            let tx_out = tx.clone();
            let stdout_task = tokio::spawn(async move {
                let mut full = Vec::new();
                if let Some(mut r) = stdout {
                    use tokio::io::AsyncReadExt;
                    let mut buf = [0u8; 4096];
                    while let Ok(n) = r.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                        full.push(chunk.clone());
                        let _ = tx_out.send(AppEvent::CommandChunk(req_id, chunk));
                    }
                }
                full.concat()
            });

            let tx_err = tx.clone();
            let stderr_task = tokio::spawn(async move {
                let mut full = Vec::new();
                if let Some(mut r) = stderr {
                    use tokio::io::AsyncReadExt;
                    let mut buf = [0u8; 4096];
                    while let Ok(n) = r.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                        full.push(chunk.clone());
                        let _ = tx_err.send(AppEvent::CommandChunk(req_id, chunk));
                    }
                }
                full.concat()
            });

            let status = child.wait().await;
            let exit_code = match status {
                Ok(s) => s.code().unwrap_or(0),
                Err(_) => 1,
            };

            let stdout_res = stdout_task.await.unwrap_or_default();
            let stderr_res = stderr_task.await.unwrap_or_default();
            let duration_ms = start.elapsed().as_millis() as i64;

            let combined = if stderr_res.is_empty() {
                stdout_res
            } else if stdout_res.is_empty() {
                stderr_res
            } else {
                format!("{}\n{}", stdout_res, stderr_res)
            };
            let cleaned = crate::ui::layout::sanitize_terminal_output(&combined);

            let _ = tx.send(AppEvent::CommandCompleted(
                req_id,
                crate::app::events::CommandResult {
                    cmd: cmd_for_task,
                    output: cleaned,
                    exit_code,
                    duration_ms,
                },
            ));
        });

        self.command_run.abort_handle = Some(handle.abort_handle());

        Ok(())
    }

    pub async fn run_command_capture(&self, cmd: &str) -> Result<String> {
        let ws_path = self.session_meta.workspace_path();
        let _ = std::fs::create_dir_all(&ws_path);
        let (output, _) = Self::run_process_in_dir(cmd, &ws_path).await?;
        Ok(output)
    }

    pub async fn run_process_in_dir(cmd: &str, dir: &std::path::Path) -> Result<(String, i32)> {
    #[cfg(target_os = "windows")]
    let mut command = tokio::process::Command::new("cmd");
    #[cfg(target_os = "windows")]
    command.args(["/C", cmd]);

    #[cfg(not(target_os = "windows"))]
    let mut command = tokio::process::Command::new("sh");
    #[cfg(not(target_os = "windows"))]
    command.args(["-c", cmd]);

    command.current_dir(dir);
    let output = command.output().await.context("Failed to execute process")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    let combined = if stderr.is_empty() {
        stdout.to_string()
    } else if stdout.is_empty() {
        stderr.to_string()
    } else {
        format!("{}\n{}", stdout, stderr)
    };

    let cleaned = crate::ui::layout::sanitize_terminal_output(&combined);
    let exit_code = output.status.code().unwrap_or(0);
    Ok((cleaned, exit_code))
}

    pub async fn send_to_pty(&mut self, cmd: &str) -> Result<()> {
        if self.active_tab > 0 && self.active_tab - 1 < self.tabs.len() {
            if let Some(tab) = self.tabs.get_mut(self.active_tab - 1) {
                tab.write(format!("{}\n", cmd).as_bytes())?;
            }
        }
        Ok(())
    }
    

}
