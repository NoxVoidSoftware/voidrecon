//! Special commands and workspace command execution
use crate::app::state::{App, NotifLevel};
use crate::app::events::AppEvent;
use crate::config::{ProviderKind, ALL_PROVIDERS};
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
                    let input = parts[1].trim().to_string();
                    // Blocking SQLite lookup/insert runs off the event loop.
                    let db = self.db.clone();
                    let resolved = tokio::task::spawn_blocking(move || -> Result<(i64, String, bool)> {
                        let target = if let Ok(id) = input.parse::<i64>() {
                            DatabaseQueries::get_target(&db.conn(), id)?
                        } else {
                            DatabaseQueries::get_target_by_ip(&db.conn(), &input)?
                        };
                        if let Some(t) = target {
                            Ok((t.id, t.ip_host, false))
                        } else {
                            let id = DatabaseQueries::add_target(&db.conn(), &input, None, None)?;
                            Ok((id, input.clone(), true))
                        }
                    })
                    .await
                    .context("Target lookup task panicked")??;
                    let (id, ip_host, is_new) = resolved;
                    self.active_target_id = Some(id);
                    self.reload_sidebar().await?;
                    if is_new {
                        self.add_notification(format!("Added & activated target: {} (ID: {})", ip_host, id), NotifLevel::Info);
                    } else {
                        self.add_notification(format!("Active target: {} (ID: {})", ip_host, id), NotifLevel::Info);
                    }
                } else if let Some(target_id) = self.active_target_id {
                    let db = self.db.clone();
                    let target = tokio::task::spawn_blocking(move || -> Result<Option<crate::db::models::Target>> {
                        DatabaseQueries::get_target(&db.conn(), target_id)
                    })
                    .await
                    .context("Target lookup task panicked")??;
                    if let Some(target) = target {
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
                        let note = parts[1].to_string();
                        let db = self.db.clone();
                        tokio::task::spawn_blocking(move || -> Result<i64> {
                            DatabaseQueries::add_note(&db.conn(), target_id, &note)
                        })
                        .await
                        .context("Add note task panicked")??;
                        self.add_notification("Note added".to_string(), NotifLevel::Info);
                        self.reload_sidebar().await?;
                    }
                }
            }
            ":models" | ":model" => {
                self.open_model_picker();
            }
            ":provider" | ":providers" => {
                if parts.len() > 1 {
                    let subparts: Vec<&str> = parts[1].split_whitespace().collect();
                    let name = subparts.first().copied().unwrap_or("");
                    if matches!(name, "setup" | "menu") {
                        self.ui_state.show_api_key_prompt = true;
                    } else if let Some(provider) = ProviderKind::from_alias(name) {
                        let custom_url = subparts.get(1).map(|url| url.to_string());
                        if provider.spec().accepts_custom_url() {
                            self.activate_provider_at(provider, custom_url).await;
                        } else {
                            self.switch_to_provider(provider).await?;
                        }
                    } else {
                        let supported: Vec<&str> = ALL_PROVIDERS
                            .iter()
                            .filter_map(|kind| kind.spec().aliases().first().copied())
                            .collect();
                        self.add_notification(
                            format!(
                                "Unknown provider '{}'. Supported: {}, setup",
                                name,
                                supported.join(", ")
                            ),
                            NotifLevel::Warning,
                        );
                    }
                } else {
                    self.ui_state.show_api_key_prompt = true;
                }
            }
            ":sessions" => {
                self.ui_state.session_list = crate::app::state::SessionMeta::list_sessions().await;
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
                } else {
                    self.add_notification("No active target selected".to_string(), NotifLevel::Warning);
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

        let sanitized = crate::ui::text::sanitize_terminal_output(&self.command_run.raw);
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
        self.finish_command_result(result).await
    }

    /// Records a finished command (database, chat log, nmap auto-parse) and
    /// starts the LLM's post-execution analysis of its output.
    pub async fn finish_command_result(&mut self, result: crate::app::events::CommandResult) -> Result<()> {
        let target_id = self.active_target_id;
        let active_target_id = self.active_target_id;
        let db = self.db.clone();
        let session_path = self.session_meta.path();
        let cmd = result.cmd.clone();
        let output = result.output.clone();
        let exit_code = result.exit_code;
        let duration_ms = result.duration_ms;

        // Save to database with full output artifact (blocking SQLite I/O off the event loop)
        let (parse_result, output_path) = tokio::task::spawn_blocking(move || -> Result<(Option<i64>, Option<String>)> {
            let _ = DatabaseQueries::add_chat_message(&db.conn(), target_id, "user", &cmd);
            let _ = DatabaseQueries::add_chat_message(&db.conn(), target_id, "assistant", &output);
            let output_path = DatabaseQueries::add_command(
                &db.conn(),
                &session_path,
                target_id,
                &cmd,
                Some(&output),
                Some(exit_code),
                Some(duration_ms),
            )
            .ok()
            .and_then(|(_, path)| path);

            // Auto-parse scan outputs (e.g. nmap) to update target and open ports immediately
            let parse_result = DatabaseQueries::parse_nmap_and_update_target(
                &db.conn(),
                &cmd,
                &output,
                active_target_id,
            )?;
            Ok((parse_result, output_path))
        })
        .await
        .context("Command completion DB task panicked")??;

        // Add output to chat log. Long output gets a header naming the saved
        // file, so excerpts sent to the LLM later can point at the full text.
        let chat_output = match &output_path {
            Some(path) if result.output.len() > Self::CHAT_OUTPUT_BYTES => {
                Self::output_with_saved_path(&result.output, path)
            }
            _ => result.output.clone(),
        };
        self.append_chat_message(ChatMessage::output(chat_output)).await;
        self.ui_state.log_at_bottom = true;

        if let Some(discovered_target_id) = parse_result {
            self.active_target_id = Some(discovered_target_id);
            self.add_notification(
                "Recon engine: Discovered target and updated open ports in DB".to_string(),
                NotifLevel::Info,
            );
        }

        // Reload sidebar
        self.reload_sidebar().await?;
        self.refresh_exploit_cache().await;

        // Trigger post-execution analysis
        self.llm.post_exec_mode = true;
        self.send_post_exec_analysis(&result.cmd, &result.output, output_path.as_deref())
            .await?;

        Ok(())
    }

    pub async fn execute_workspace_command(&mut self, cmd: &str) -> Result<()> {
        let cmd_str = cmd.trim().to_string();
        if cmd_str.is_empty() {
            return Ok(());
        }
        if Self::is_interactive_command(&cmd_str) {
            return self.spawn_command_tab(&cmd_str).await;
        }

        let req_id = self.next_request_id();

        // Add to chat log
        self.append_chat_message(ChatMessage::user(format!("$ {}", cmd_str))).await;
        self.ui_state.log_at_bottom = true;

        self.command_run.running = Some((req_id, cmd_str.clone(), Instant::now()));
        self.command_run.output.clear();
        self.command_run.raw.clear();

        let ws_path = self.session_meta.workspace_path();
        let _ = tokio::fs::create_dir_all(&ws_path).await;

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
            let cleaned = crate::ui::text::sanitize_terminal_output(&combined);

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

    pub async fn send_to_pty(&mut self, cmd: &str) -> Result<()> {
        if self.active_tab > 0 && self.active_tab - 1 < self.tabs.len() {
            if let Some(tab) = self.tabs.get_mut(self.active_tab - 1) {
                tab.write(format!("{}\n", cmd).as_bytes())?;
            }
        }
        Ok(())
    }

    /// Opens `command` in a new PTY tab and focuses it. Returns (tab id, tab number).
    pub(super) async fn open_command_tab(&mut self, command: &str) -> Result<(usize, usize)> {
        let workspace = self.session_meta.workspace_path();
        tokio::fs::create_dir_all(&workspace)
            .await
            .with_context(|| format!("Failed to create workspace {}", workspace.display()))?;
        let id = self.allocate_tab_id();
        let tab = crate::pty::tab::PtyTab::new_command(
            id,
            command,
            &workspace,
            self.event_tx.clone(),
            self.pty_dimensions(),
        )?;
        let label = tab.label.clone();
        let number = self.add_tab(tab);
        self.add_notification(format!("Started {} in Tab {}", label, number), NotifLevel::Info);
        Ok((id, number))
    }

    /// Interactive command (ssh, nc, REPLs): runs in its own tab until the user
    /// or the LLM closes it; the LLM reads it with `read_terminal_tab`.
    pub async fn spawn_command_tab(&mut self, command: &str) -> Result<()> {
        let (_, number) = self.open_command_tab(command).await?;
        self.append_chat_message(ChatMessage::user(format!(
            "Started interactive command in tab {}: {}",
            number, command
        )))
        .await;
        Ok(())
    }

    /// Runs a command the LLM asked for in its own terminal tab, so commands
    /// that need a TTY or block on input never stall the workspace. When it
    /// exits, the output is recorded and analyzed like a workspace command.
    pub async fn run_llm_command(&mut self, command: &str, interactive: bool) -> Result<()> {
        let command = command.trim();
        if command.is_empty() {
            return Ok(());
        }
        if interactive || Self::is_interactive_command(command) {
            self.spawn_command_tab(command).await?;
            return self.continue_llm_conversation().await;
        }
        self.append_chat_message(ChatMessage::user(format!("$ {}", command))).await;
        self.ui_state.log_at_bottom = true;
        let (id, _) = self.open_command_tab(command).await?;
        self.tab_command_runs.insert(
            id,
            crate::app::state::TabCommandRun {
                command: command.to_string(),
                started: Instant::now(),
            },
        );
        Ok(())
    }

    pub fn is_interactive_command(command: &str) -> bool {
        let parts = command.split_whitespace().collect::<Vec<_>>();
        let executable_index = if parts.first().copied() == Some("sudo") {
            1
        } else {
            0
        };
        let executable = parts
            .get(executable_index)
            .copied()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .rsplit('\\')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let args = parts.iter().skip(executable_index + 1).copied();
        match executable.as_str() {
            "evil-winrm" | "ssh" | "telnet" | "ftp" | "sftp" | "mysql" | "psql"
            | "sqlplus" | "redis-cli" | "smbclient" | "msfconsole" | "irb" => true,
            "python" | "python3" | "node" | "pwsh" | "powershell" => {
                parts.len() == executable_index + 1 || args.clone().any(|arg| arg == "-i")
            }
            "nc" | "ncat" => args.clone().any(|arg| arg.starts_with('-') && arg.contains('l')),
            "socat" => args.clone().any(|arg| arg.to_ascii_uppercase().contains("TCP-LISTEN")),
            _ => false,
        }
    }

}
