//! Tool definitions and execution for voidrecon LLM

use anyhow::{Context, Result};
use serde_json::Value;

use crate::db::{Database, DatabaseQueries};
use crate::llm::types::{ToolCall, ToolDefinition};

/// Get all tool definitions for the LLM
pub fn get_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new_function(
            "run_command",
            "Suggest or execute a shell command or scan in the session workspace (e.g. nmap, gobuster, nikto, hydra, curl, whoami) against the target. The command is presented to the user in the voidrecon TUI with [r]un / [e]dit / [s]kip confirmation before execution, and results are captured into the workspace and database.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The exact shell command to execute, e.g. 'nmap -sS -sV -T4 10.129.90.124'"
                    },
                    "explanation": {
                        "type": "string",
                        "description": "Concise explanation of what this command does and why it should be executed"
                    }
                },
                "required": ["command", "explanation"]
            }),
        ),
        ToolDefinition::new_function(
            "set_target",
            "Set or switch the active target in scope. If the target exists in the database, sets it as active; if not, inserts it and sets it as active.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "ip_host": {"type": "string", "description": "Target IP address or hostname"},
                    "ports_json": {"type": "string", "description": "Optional JSON array of ports [{port,proto,service,version}]"},
                    "metadata_json": {"type": "string", "description": "Optional JSON metadata {os, hostnames, tags}"},
                    "notes": {"type": "string", "description": "Optional analyst notes"}
                },
                "required": ["ip_host"]
            }),
        ),
        ToolDefinition::new_function(
            "list_targets",
            "List all targets currently recorded in the session database with IDs, IPs, open ports count, findings count, and notes.",
            serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        ),
        ToolDefinition::new_function(
            "get_target_info",
            "Get full info for a target (or active target if omitted): open ports, OS, metadata, and all findings.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "target_id": {"type": "integer", "description": "ID of the target to query (defaults to active target)"},
                    "ip_host": {"type": "string", "description": "Optional IP/hostname to look up"}
                }
            }),
        ),
        ToolDefinition::new_function(
            "get_findings",
            "Query security findings recorded in the database for a target or across all targets, optionally filtered by severity.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "target_id": {"type": "integer", "description": "Target ID to query (defaults to active target if omitted)"},
                    "severity": {"type": "string", "enum": ["info","low","medium","high","critical"], "description": "Filter by severity"}
                }
            }),
        ),
        ToolDefinition::new_function(
            "get_recent_commands",
            "Get the last N commands run against a target (or active target if omitted), with output previews and workspace output paths.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "target_id": {"type": "integer", "description": "Target ID (defaults to active target)"},
                    "limit": {"type": "integer", "default": 5}
                }
            }),
        ),
        ToolDefinition::new_function(
            "update_target",
            "Update ports, OS or metadata for a target in the database after a scan reveals new info.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "target_id": {"type": "integer", "description": "Target ID (defaults to active target if omitted)"},
                    "ports_json": {"type": "string", "description": "JSON array of {port,proto,service,version}"},
                    "metadata_json": {"type": "string", "description": "JSON object: {os, hostnames, tags}"}
                }
            }),
        ),
        ToolDefinition::new_function(
            "add_finding",
            "Record a security finding discovered in command or scan output into the database.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "target_id": {"type": "integer", "description": "Target ID (defaults to active target if omitted)"},
                    "title": {"type": "string"},
                    "description": {"type": "string"},
                    "severity": {"type": "string", "enum": ["info","low","medium","high","critical"]},
                    "source_cmd": {"type": "string"},
                    "confidence": {"type": "integer", "description": "Confidence percentage (0-100)", "default": 100},
                    "evidence_path": {"type": "string", "description": "Relative path to evidence file in workspace"}
                },
                "required": ["title", "severity"]
            }),
        ),
        ToolDefinition::new_function(
            "add_target",
            "Add a newly discovered host to the session targets in the database.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "ip_host": {"type": "string"},
                    "ports_json": {"type": "string"},
                    "notes": {"type": "string"}
                },
                "required": ["ip_host"]
            }),
        ),
        ToolDefinition::new_function(
            "save_workspace_file",
            "Save evidence, artifacts, or notes into the session workspace (workspace/evidence/ or workspace/artifacts/). NOTE: Requires interactive user approval in the TUI before the file is written to disk.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "folder": {
                        "type": "string",
                        "enum": ["evidence", "artifacts", "exploits"],
                        "description": "Destination subfolder in workspace"
                    },
                    "filename": {
                        "type": "string",
                        "description": "Filename, e.g. 'nmap_scan_full.txt', 'admin_creds.txt', 'notes.md'"
                    },
                    "content": {
                        "type": "string",
                        "description": "File text content"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of the file or evidence for user approval"
                    },
                    "target_id": {
                        "type": "integer",
                        "description": "Target ID associated with this file (defaults to active target)"
                    }
                },
                "required": ["folder", "filename", "content"]
            }),
        ),
        ToolDefinition::new_function(
            "read_workspace_file",
            "Read lines from any workspace file (in outputs/, evidence/, artifacts/, or exploits/), such as inspecting full raw scan outputs from workspace/outputs/.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to file, e.g. 'workspace/outputs/abc.txt' or 'evidence/scan.txt'"
                    },
                    "folder": {
                        "type": "string",
                        "enum": ["outputs", "evidence", "artifacts", "exploits"],
                        "description": "Workspace folder if path is not specified"
                    },
                    "filename": {
                        "type": "string",
                        "description": "Filename within folder"
                    },
                    "start_line": {
                        "type": "integer",
                        "description": "Line number to start reading from (1-based, default 1)",
                        "default": 1
                    },
                    "max_lines": {
                        "type": "integer",
                        "description": "Maximum number of lines to read (default 120)",
                        "default": 120
                    }
                }
            }),
        ),
        ToolDefinition::new_function(
            "open_reverse_shell_tab",
            "Set up a reverse shell listener tab. Call this ONLY when the user explicitly asks to exploit, gain shell access, or start a reverse shell listener.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "target_id": {"type": "integer"},
                    "lhost": {"type": "string", "description": "Our listening IP (auto-detected from active interface if omitted)"},
                    "lport": {"type": "integer", "default": 4444},
                    "shell_type": {"type": "string", "enum": ["bash", "sh", "python", "powershell"], "default": "bash"}
                },
                "required": ["target_id"]
            }),
        ),
        ToolDefinition::new_function(
            "get_shell_context",
            "Get current shell session state: target info, what has been run in this shell so far, current working directory if known.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "target_id": {"type": "integer"}
                },
                "required": ["target_id"]
            }),
        ),
        ToolDefinition::new_function(
            "delete_evidence",
            "Delete an evidence file from the workspace (workspace/evidence/) and remove its corresponding finding entry from the target database so it no longer appears in the targets list or sidebar.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "filename": {
                        "type": "string",
                        "description": "Filename of the evidence file to delete (e.g. 'web_curl_80.txt' or 'smb_enum_shares.txt')"
                    },
                    "target_id": {
                        "type": "integer",
                        "description": "Target ID associated with the finding (optional, defaults to active target)"
                    }
                },
                "required": ["filename"]
            }),
        ),
        ToolDefinition::new_function(
            "delete_workspace_file",
            "Delete any workspace file (in evidence/, artifacts/, outputs/, or exploits/) and clean up any related database findings or notes.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "folder": {
                        "type": "string",
                        "enum": ["evidence", "artifacts", "outputs", "exploits"],
                        "description": "Folder in workspace containing the file"
                    },
                    "filename": {
                        "type": "string",
                        "description": "Filename to delete"
                    }
                },
                "required": ["folder", "filename"]
            }),
        ),
        ToolDefinition::new_function(
            "save_exploit_script",
            "Save or update a Python exploit / PoC script in workspace/exploits/. Automatically validates Python syntax (py_compile). Returns compact status.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Exploit filename, e.g. 'cve_2023_xxx.py' or 'vsftpd_backdoor.py'"
                    },
                    "script_code": {
                        "type": "string",
                        "description": "Full Python script source code"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short explanation of the vulnerability, exploit logic, and usage"
                    },
                    "target_id": {
                        "type": "integer",
                        "description": "Target ID if associated with a target"
                    }
                },
                "required": ["name", "script_code"]
            }),
        ),
        ToolDefinition::new_function(
            "read_exploit_script",
            "Read the source code or docstring of an existing exploit in workspace/exploits/ to inspect parameters or diagnose errors.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Script name (e.g. 'cve_2023_xxx.py')"
                    },
                    "max_lines": {
                        "type": "integer",
                        "description": "Maximum lines to read (default 120)",
                        "default": 120
                    }
                },
                "required": ["name"]
            }),
        ),
        ToolDefinition::new_function(
            "list_workspace_exploits",
            "List all saved Python exploit scripts in workspace/exploits/ with file sizes, usage, and summaries.",
            serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        ),
        ToolDefinition::new_function(
            "execute_exploit_script",
            "Execute a saved Python exploit from workspace/exploits/ against a target with specified arguments. Prompts the user for Action Approval before executing.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Exploit script name, e.g. 'cve_2023_xxx.py'"
                    },
                    "args": {
                        "type": "string",
                        "description": "Command line arguments passed to the Python script, e.g. '192.168.1.1 21' or '--target 192.168.1.1'"
                    },
                    "target_id": {
                        "type": "integer",
                        "description": "Target ID against which the exploit is run"
                    }
                },
                "required": ["name", "target_id"]
            }),
        ),
        ToolDefinition::new_function(
            "list_workspace_files",
            "List saved files in the workspace (exploits, artifacts, evidence, outputs).",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "category": {
                        "type": "string",
                        "enum": ["all", "exploits", "artifacts", "evidence", "outputs"],
                        "default": "all"
                    }
                }
            }),
        ),
    ]
}

/// Execute a tool call and return the result
pub async fn execute_tool_call(
    tool_call: &ToolCall,
    db: &Database,
    active_target_id: Option<i64>,
    workspace_path: &std::path::Path,
    app_tx: &tokio::sync::mpsc::UnboundedSender<crate::app::AppEvent>,
) -> Result<Value> {
    validate_tool_call(tool_call)?;
    let name = &tool_call.function.name;
    let args: Value = serde_json::from_str(&tool_call.function.arguments)
        .context("Failed to parse tool arguments")?;

    let conn = db.conn();

    match name.as_str() {
        "set_target" => {
            let ip_host = args.get("ip_host").and_then(|v| v.as_str()).context("Missing ip_host")?;
            let ports_json = args.get("ports_json").and_then(|v| v.as_str());
            let metadata_json = args.get("metadata_json").and_then(|v| v.as_str());
            let notes = args.get("notes").and_then(|v| v.as_str());

            let target = DatabaseQueries::get_target_by_ip(conn, ip_host)?;
            let (target_id, is_new) = if let Some(t) = target {
                if ports_json.is_some() || metadata_json.is_some() {
                    DatabaseQueries::update_target(conn, t.id, ports_json, metadata_json)?;
                }
                (t.id, false)
            } else {
                let id = DatabaseQueries::add_target(conn, ip_host, ports_json, notes)?;
                if metadata_json.is_some() {
                    DatabaseQueries::update_target(conn, id, None, metadata_json)?;
                }
                (id, true)
            };

            let _ = app_tx.send(crate::app::AppEvent::SetActiveTarget(target_id));
            Ok(serde_json::json!({
                "status": "active_target_set",
                "id": target_id,
                "target_id": target_id,
                "ip_host": ip_host,
                "is_new": is_new,
                "message": format!("Target {} (ID: {}) set as active in session scope", ip_host, target_id)
            }))
        }
        "list_targets" => {
            let targets = DatabaseQueries::get_all_targets(conn)?;
            let mut list = Vec::new();
            for t in targets {
                let findings_count = DatabaseQueries::get_findings_for_target(conn, t.id)
                    .map(|f| f.len())
                    .unwrap_or(0);
                let ports_count = t.ports_json.as_ref()
                    .and_then(|p| serde_json::from_str::<Vec<crate::db::models::PortInfo>>(p).ok())
                    .map(|ports| ports.len())
                    .unwrap_or(0);
                list.push(serde_json::json!({
                    "id": t.id,
                    "ip_host": t.ip_host,
                    "is_active": active_target_id == Some(t.id),
                    "open_ports_count": ports_count,
                    "findings_count": findings_count,
                    "notes": t.notes
                }));
            }
            Ok(serde_json::json!({
                "targets_count": list.len(),
                "active_target_id": active_target_id,
                "targets": list
            }))
        }
        "get_target_info" => {
            let target_id_arg = args.get("target_id").and_then(|v| v.as_i64());
            let ip_host_arg = args.get("ip_host").and_then(|v| v.as_str());

            let target = if let Some(id) = target_id_arg {
                DatabaseQueries::get_target(conn, id)?
            } else if let Some(ip) = ip_host_arg {
                DatabaseQueries::get_target_by_ip(conn, ip)?
            } else if let Some(active_id) = active_target_id {
                DatabaseQueries::get_target(conn, active_id)?
            } else {
                let all = DatabaseQueries::get_all_targets(conn)?;
                all.into_iter().next()
            };

            if let Some(t) = target {
                let (_, findings) = DatabaseQueries::get_target_with_findings(conn, t.id)?
                    .unwrap_or((t.clone(), Vec::new()));
                Ok(serde_json::json!({
                    "id": t.id,
                    "ip_host": t.ip_host,
                    "is_active": active_target_id == Some(t.id),
                    "ports_json": t.ports_json,
                    "metadata_json": t.metadata_json,
                    "notes": t.notes,
                    "findings": findings
                }))
            } else {
                Ok(serde_json::json!({
                    "status": "not_found",
                    "message": "No targets found in database. Use set_target to add one."
                }))
            }
        }
        "get_findings" => {
            let target_id = args.get("target_id").and_then(|v| v.as_i64()).or(active_target_id);
            let severity_filter = args.get("severity").and_then(|v| v.as_str());

            let findings = if let Some(tid) = target_id {
                DatabaseQueries::get_findings_for_target(conn, tid)?
            } else {
                DatabaseQueries::get_all_findings(conn)?
            };

            let filtered: Vec<_> = findings.into_iter()
                .filter(|f| {
                    if let Some(sev) = severity_filter {
                        f.severity.eq_ignore_ascii_case(sev)
                    } else {
                        true
                    }
                })
                .map(|f| serde_json::json!({
                    "id": f.id,
                    "target_id": f.target_id,
                    "title": f.title,
                    "description": f.description,
                    "severity": f.severity,
                    "source_cmd": f.source_cmd,
                    "confidence": f.confidence,
                    "evidence_path": f.evidence_path
                }))
                .collect();

            Ok(serde_json::json!({
                "findings_count": filtered.len(),
                "target_id": target_id,
                "findings": filtered
            }))
        }
        "get_recent_commands" => {
            let target_id = args.get("target_id").and_then(|v| v.as_i64()).or(active_target_id);
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

            if let Some(tid) = target_id {
                let commands = DatabaseQueries::get_recent_commands(conn, tid, limit)?;
                let cmd_json: Vec<_> = commands.into_iter().map(|c| serde_json::json!({
                    "raw_cmd": c.raw_cmd,
                    "output_preview": c.output,
                    "output_path": c.output_path,
                    "exit_code": c.exit_code,
                    "duration_ms": c.duration_ms,
                    "executed_at": c.executed_at.to_rfc3339()
                })).collect();
                Ok(serde_json::json!(cmd_json))
            } else {
                Ok(serde_json::json!({
                    "message": "No active target selected. Use set_target to select one."
                }))
            }
        }
        "update_target" => {
            let target_id = args.get("target_id").and_then(|v| v.as_i64()).or(active_target_id)
                .context("Missing target_id and no active target selected")?;
            let ports_json = args.get("ports_json").and_then(|v| v.as_str());
            let metadata_json = args.get("metadata_json").and_then(|v| v.as_str());
            DatabaseQueries::update_target(conn, target_id, ports_json, metadata_json)?;
            let _ = app_tx.send(crate::app::AppEvent::SidebarReload);
            Ok(serde_json::json!({"success": true, "target_id": target_id}))
        }
        "add_finding" => {
            let target_id = args.get("target_id").and_then(|v| v.as_i64()).or(active_target_id)
                .context("Missing target_id and no active target selected")?;
            let title = args.get("title").and_then(|v| v.as_str()).context("Missing title")?;
            let description = args.get("description").and_then(|v| v.as_str()).unwrap_or("");
            let severity = args.get("severity").and_then(|v| v.as_str()).context("Missing severity")?;
            let source_cmd = args.get("source_cmd").and_then(|v| v.as_str());
            let confidence = args.get("confidence").and_then(|v| v.as_i64()).map(|c| c as i32);
            let evidence_path = args.get("evidence_path").and_then(|v| v.as_str());
            let id = DatabaseQueries::add_finding(conn, target_id, title, description, severity, source_cmd, confidence, evidence_path)?;
            let _ = app_tx.send(crate::app::AppEvent::SidebarReload);
            Ok(serde_json::json!({"success": true, "finding_id": id, "target_id": target_id}))
        }
        "add_target" => {
            let ip_host = args.get("ip_host").and_then(|v| v.as_str()).context("Missing ip_host")?;
            let ports_json = args.get("ports_json").and_then(|v| v.as_str());
            let notes = args.get("notes").and_then(|v| v.as_str());
            let id = DatabaseQueries::add_target(conn, ip_host, ports_json, notes)?;
            let _ = app_tx.send(crate::app::AppEvent::SetActiveTarget(id));
            Ok(serde_json::json!({"id": id, "target_id": id, "ip_host": ip_host, "success": true}))
        }
        "open_reverse_shell_tab" => {
            let target_id = args.get("target_id").and_then(|v| v.as_i64())
                .context("Missing target_id")?;
            let lhost = args.get("lhost").and_then(|v| v.as_str());
            let lport = args.get("lport").and_then(|v| v.as_u64()).unwrap_or(4444) as u16;
            let shell_type = args.get("shell_type").and_then(|v| v.as_str()).unwrap_or("bash");
            
            // The actual PTY spawn is handled by the app, we just return the info
            Ok(serde_json::json!({
                "target_id": target_id,
                "lhost": lhost,
                "lport": lport,
                "shell_type": shell_type,
                "action": "spawn_listener_tab"
            }))
        }
        "get_shell_context" => {
            let target_id = args.get("target_id").and_then(|v| v.as_i64())
                .context("Missing target_id")?;
            let target = DatabaseQueries::get_target(conn, target_id)?.context("Target not found")?;
            let commands = DatabaseQueries::get_recent_commands(conn, target_id, 50)?;
            Ok(serde_json::json!({
                "ip_host": target.ip_host,
                "shell_type": "bash",
                "commands_in_shell": commands,
                "findings_added_this_session": []
            }))
        }
        "save_exploit_script" => {
            let raw_name = args.get("name").and_then(|v| v.as_str()).context("Missing exploit name")?;
            let script_code = args.get("script_code").and_then(|v| v.as_str()).context("Missing script_code")?;
            let description = args.get("description").and_then(|v| v.as_str()).unwrap_or("Exploit script");
            let target_id = args.get("target_id").and_then(|v| v.as_i64()).or(active_target_id);

            // Clean filename: take only file name without directory traversal
            let file_stem = std::path::Path::new(raw_name)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(raw_name);
            let mut clean_name: String = file_stem
                .chars()
                .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' { c } else { '_' })
                .collect();
            if !clean_name.ends_with(".py") {
                clean_name.push_str(".py");
            }

            let exploits_dir = workspace_path.join("exploits");
            std::fs::create_dir_all(&exploits_dir)?;
            let file_path = exploits_dir.join(&clean_name);
            std::fs::write(&file_path, script_code)?;
            let bytes_written = script_code.len();

            // Run automated Python syntax validation via py_compile
            let syntax_check = tokio::process::Command::new("python")
                .args(["-m", "py_compile", file_path.to_str().unwrap_or(raw_name)])
                .current_dir(workspace_path)
                .output()
                .await;

            let (syntax_valid, syntax_error) = match syntax_check {
                Ok(out) => {
                    if out.status.success() {
                        (true, None)
                    } else {
                        let err = String::from_utf8_lossy(&out.stderr).to_string();
                        (false, Some(err))
                    }
                }
                Err(_) => (true, None),
            };

            // Record finding in database if target exists
            if let Some(tid) = target_id {
                let finding_title = format!("Exploit Script: {}", clean_name);
                let finding_desc = format!("{} (saved to workspace/exploits/{})", description, clean_name);
                let source_cmd = format!("python workspace/exploits/{}", clean_name);
                let rel_path = format!("workspace/exploits/{}", clean_name);
                let _ = DatabaseQueries::add_finding(
                    conn,
                    tid,
                    &finding_title,
                    &finding_desc,
                    "high",
                    Some(&source_cmd),
                    Some(100),
                    Some(&rel_path),
                );
                let _ = app_tx.send(crate::app::AppEvent::SidebarReload);
            }

            if let Some(err) = syntax_error {
                Ok(serde_json::json!({
                    "status": "syntax_error",
                    "file": clean_name,
                    "path": format!("workspace/exploits/{}", clean_name),
                    "syntax_error": err,
                    "message": "Script was saved, but Python syntax validation failed. Please fix the error and update with save_exploit_script."
                }))
            } else {
                Ok(serde_json::json!({
                    "status": "saved",
                    "syntax_valid": syntax_valid,
                    "file": clean_name,
                    "path": format!("workspace/exploits/{}", clean_name),
                    "bytes": bytes_written,
                    "run_command": format!("python workspace/exploits/{}", clean_name),
                    "message": format!("Exploit saved to workspace/exploits/{} with valid Python syntax. Call execute_exploit_script to execute it against the target.", clean_name)
                }))
            }
        }
        "read_exploit_script" => {
            let raw_name = args.get("name").and_then(|v| v.as_str()).context("Missing exploit name")?;
            let max_lines = args.get("max_lines").and_then(|v| v.as_u64()).unwrap_or(120) as usize;

            let file_stem = std::path::Path::new(raw_name)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(raw_name);
            let mut clean_name: String = file_stem
                .chars()
                .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' { c } else { '_' })
                .collect();
            if !clean_name.ends_with(".py") {
                clean_name.push_str(".py");
            }

            let file_path = workspace_path.join("exploits").join(&clean_name);
            if !file_path.exists() {
                return Ok(serde_json::json!({
                    "status": "error",
                    "error": format!("Exploit script 'workspace/exploits/{}' not found", clean_name)
                }));
            }

            let content = match std::fs::read_to_string(&file_path) {
                Ok(c) => c,
                Err(e) => {
                    return Ok(serde_json::json!({
                        "status": "error",
                        "error": format!("Failed to read exploit script '{}': {}", clean_name, e)
                    }));
                }
            };
            let lines: Vec<&str> = content.lines().collect();
            let total_lines = lines.len();
            let truncated = total_lines > max_lines;
            let code = if truncated {
                format!("{}\n# ... [truncated, total {} lines] ...", lines[..max_lines].join("\n"), total_lines)
            } else {
                content
            };

            let max_bytes = 24_000;
            let (code, byte_truncated) = if code.len() > max_bytes {
                let mut end = max_bytes;
                while !code.is_char_boundary(end) {
                    end -= 1;
                }
                (
                    format!("{}\n# ... [truncated, code exceeds 24KB (total {} bytes)] ...", &code[..end], code.len()),
                    true
                )
            } else {
                (code, false)
            };

            Ok(serde_json::json!({
                "file": clean_name,
                "total_lines": total_lines,
                "truncated": truncated || byte_truncated,
                "code": code,
                "run_command": format!("python workspace/exploits/{}", clean_name)
            }))
        }
        "list_workspace_exploits" => {
            let exploits_dir = workspace_path.join("exploits");
            let mut exploits = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&exploits_dir) {
                for entry in entries.flatten() {
                    if let Ok(meta) = entry.metadata() {
                        if meta.is_file() {
                            let fname = entry.file_name().to_string_lossy().to_string();
                            if fname.ends_with(".py") {
                                let mut summary = String::new();
                                if let Ok(content) = std::fs::read_to_string(entry.path()) {
                                    for line in content.lines().take(10) {
                                        let trimmed = line.trim();
                                        if trimmed.starts_with('#') || trimmed.starts_with("\"\"\"") || trimmed.starts_with("'''") {
                                            let clean = trimmed.trim_matches(|c| c == '#' || c == '"' || c == '\'').trim();
                                            if !clean.is_empty() {
                                                summary = clean.to_string();
                                                break;
                                            }
                                        }
                                    }
                                }
                                exploits.push(serde_json::json!({
                                    "file": fname,
                                    "size_bytes": meta.len(),
                                    "summary": if summary.is_empty() { "Python exploit".to_string() } else { summary },
                                    "run_command": format!("python workspace/exploits/{}", fname)
                                }));
                            }
                        }
                    }
                }
            }

            Ok(serde_json::json!({
                "exploits_count": exploits.len(),
                "exploits": exploits
            }))
        }
        "execute_exploit_script" => {
            let raw_name = args.get("name").and_then(|v| v.as_str()).context("Missing exploit name")?;
            let target_id = args.get("target_id").and_then(|v| v.as_i64()).or(active_target_id)
                .context("Missing target_id")?;
            let script_args = if let Some(arr) = args.get("args").and_then(|v| v.as_array()) {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<&str>>()
                    .join(" ")
            } else if let Some(s) = args.get("args").and_then(|v| v.as_str()) {
                s.to_string()
            } else {
                String::new()
            };

            let file_stem = std::path::Path::new(raw_name)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(raw_name);
            let mut clean_name: String = file_stem
                .chars()
                .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' { c } else { '_' })
                .collect();
            if !clean_name.ends_with(".py") {
                clean_name.push_str(".py");
            }

            let file_path = workspace_path.join("exploits").join(&clean_name);
            if !file_path.exists() {
                anyhow::bail!("Exploit script 'workspace/exploits/{}' not found. Save it first using save_exploit_script.", clean_name);
            }

            let command_line = if script_args.is_empty() {
                format!("python workspace/exploits/{}", clean_name)
            } else {
                format!("python workspace/exploits/{} {}", clean_name, script_args)
            };

            Ok(serde_json::json!({
                "action": "execute_exploit",
                "target_id": target_id,
                "script_name": clean_name,
                "command_line": command_line
            }))
        }
        "list_workspace_files" => {
            let category = args.get("category").and_then(|v| v.as_str()).unwrap_or("all");
            let categories = if category == "all" {
                vec!["exploits", "artifacts", "evidence", "outputs"]
            } else {
                vec![category]
            };

            let mut files_by_category = serde_json::Map::new();
            for cat in categories {
                let dir = workspace_path.join(cat);
                let mut file_list = Vec::new();
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        if let Ok(meta) = entry.metadata() {
                            if meta.is_file() {
                                if let Some(fname) = entry.file_name().to_str() {
                                    file_list.push(serde_json::json!({
                                        "name": fname,
                                        "bytes": meta.len()
                                    }));
                                }
                            }
                        }
                    }
                }
                files_by_category.insert(cat.to_string(), serde_json::Value::Array(file_list));
            }

            Ok(serde_json::Value::Object(files_by_category))
        }
        "run_command" => {
            let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let explanation = args.get("explanation").and_then(|v| v.as_str()).unwrap_or("");
            Ok(serde_json::json!({
                "status": "suggested",
                "command": command,
                "explanation": explanation
            }))
        }
        "save_workspace_file" => {
            let folder = args.get("folder").and_then(|v| v.as_str()).unwrap_or("evidence");
            let filename = args.get("filename").and_then(|v| v.as_str()).context("Missing filename")?;
            let content = args.get("content").and_then(|v| v.as_str()).context("Missing content")?;
            let description = args.get("description").and_then(|v| v.as_str()).unwrap_or("");
            let target_id = args.get("target_id").and_then(|v| v.as_i64()).or(active_target_id);

            let valid_folder = match folder {
                "evidence" | "artifacts" | "exploits" => folder,
                _ => "evidence",
            };

            Ok(serde_json::json!({
                "status": "requires_approval",
                "folder": valid_folder,
                "filename": filename,
                "description": description,
                "content": content,
                "target_id": target_id
            }))
        }
        "read_workspace_file" => {
            let path_arg = args.get("path").and_then(|v| v.as_str());
            let folder_arg = args.get("folder").and_then(|v| v.as_str());
            let filename_arg = args.get("filename").and_then(|v| v.as_str());
            let start_line = args.get("start_line").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
            let max_lines = args.get("max_lines").and_then(|v| v.as_u64()).unwrap_or(120) as usize;

            let file_path = if let Some(p) = path_arg {
                let clean_p = p.trim_start_matches("workspace/").trim_start_matches("workspace\\");
                let rel = std::path::Path::new(clean_p);
                // Security: ensure no traversal outside workspace
                if rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                    anyhow::bail!("Directory traversal rejected");
                }
                workspace_path.join(rel)
            } else if let (Some(folder), Some(filename)) = (folder_arg, filename_arg) {
                let clean_folder = match folder {
                    "outputs" | "evidence" | "artifacts" | "exploits" => folder,
                    _ => anyhow::bail!("Invalid folder: {}", folder),
                };
                let stem = std::path::Path::new(filename)
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or(filename);
                let clean_name: String = stem
                    .chars()
                    .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' { c } else { '_' })
                    .collect();
                workspace_path.join(clean_folder).join(clean_name)
            } else {
                anyhow::bail!("Specify either 'path' or 'folder' + 'filename'");
            };

            if !file_path.exists() {
                return Ok(serde_json::json!({
                    "status": "error",
                    "error": format!("File '{}' not found in workspace", file_path.display())
                }));
            }

            let content = match std::fs::read_to_string(&file_path) {
                Ok(c) => c,
                Err(e) => {
                    return Ok(serde_json::json!({
                        "status": "error",
                        "error": format!("Failed to read file '{}': {}", file_path.display(), e)
                    }));
                }
            };
            let all_lines: Vec<&str> = content.lines().collect();
            let total_lines = all_lines.len();

            let offset = start_line.saturating_sub(1).min(total_lines);
            let end = (offset + max_lines).min(total_lines);
            let slice = &all_lines[offset..end];
            let truncated = end < total_lines;

            let display_content = if truncated {
                format!("{}\n... [truncated, total {} lines, showing {}-{}] ...", slice.join("\n"), total_lines, offset + 1, end)
            } else {
                slice.join("\n")
            };

            let max_bytes = 24_000;
            let (display_content, byte_truncated) = if display_content.len() > max_bytes {
                let mut end = max_bytes;
                while !display_content.is_char_boundary(end) {
                    end -= 1;
                }
                (
                    format!("{}\n... [truncated, content exceeds 24KB (total {} bytes)] ...", &display_content[..end], display_content.len()),
                    true
                )
            } else {
                (display_content, false)
            };

            Ok(serde_json::json!({
                "file": file_path.file_name().and_then(|f| f.to_str()).unwrap_or(""),
                "total_lines": total_lines,
                "start_line": offset + 1,
                "lines_returned": slice.len(),
                "truncated": truncated || byte_truncated,
                "content": display_content
            }))
        }
        "delete_evidence" => {
            let filename = args.get("filename").and_then(|v| v.as_str()).context("Missing filename")?;
            let target_id = args.get("target_id").and_then(|v| v.as_i64()).or(active_target_id);

            let stem = std::path::Path::new(filename)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(filename);
            let clean_name: String = stem
                .chars()
                .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' { c } else { '_' })
                .collect();

            let file_path = workspace_path.join("evidence").join(&clean_name);
            let file_existed = file_path.exists();
            if file_existed {
                let _ = std::fs::remove_file(&file_path);
            }

            let removed_from_db = DatabaseQueries::delete_findings_by_evidence(conn, target_id, &clean_name)?;
            let _ = app_tx.send(crate::app::AppEvent::SidebarReload);

            Ok(serde_json::json!({
                "status": "deleted",
                "filename": clean_name,
                "file_removed_from_disk": file_existed,
                "findings_removed_from_db": removed_from_db,
                "message": format!(
                    "Evidence '{}' deleted: file removed from disk ({}) and {} finding(s) deleted from database.",
                    clean_name,
                    if file_existed { "yes" } else { "was already missing" },
                    removed_from_db
                )
            }))
        }
        "delete_workspace_file" => {
            let folder = args.get("folder").and_then(|v| v.as_str()).unwrap_or("evidence");
            let filename = args.get("filename").and_then(|v| v.as_str()).context("Missing filename")?;
            let clean_folder = match folder {
                "outputs" | "evidence" | "artifacts" | "exploits" => folder,
                _ => anyhow::bail!("Invalid folder: {}", folder),
            };

            let stem = std::path::Path::new(filename)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(filename);
            let clean_name: String = stem
                .chars()
                .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' { c } else { '_' })
                .collect();

            let file_path = workspace_path.join(clean_folder).join(&clean_name);
            let file_existed = file_path.exists();
            if file_existed {
                let _ = std::fs::remove_file(&file_path);
            }

            let removed_from_db = if clean_folder == "evidence" {
                DatabaseQueries::delete_findings_by_evidence(conn, active_target_id, &clean_name)?
            } else {
                0
            };

            let _ = app_tx.send(crate::app::AppEvent::SidebarReload);

            Ok(serde_json::json!({
                "status": "deleted",
                "folder": clean_folder,
                "filename": clean_name,
                "file_removed_from_disk": file_existed,
                "findings_removed_from_db": removed_from_db,
                "message": format!(
                    "Workspace file '{}/{}' deleted from disk ({}) and {} finding(s) cleaned up.",
                    clean_folder,
                    clean_name,
                    if file_existed { "yes" } else { "was already missing" },
                    removed_from_db
                )
            }))
        }
        _ => anyhow::bail!("Unknown tool: {}", name),
    }
}

/// Validate tool call arguments against schema
pub fn validate_tool_call(tool_call: &ToolCall) -> Result<()> {
    let definitions = get_tool_definitions();
    let definition = definitions.iter().find(|d| d.name() == tool_call.function.name)
        .context(format!("Unknown tool: {}", tool_call.function.name))?;
    
    let args: Value = serde_json::from_str(&tool_call.function.arguments)
        .context("Invalid tool arguments JSON")?;
    
    // Basic required field validation
    if let Some(required) = definition.parameters().get("required").and_then(|v| v.as_array()) {
        for field in required {
            if let Some(field_name) = field.as_str() {
                if args.get(field_name).is_none() {
                    anyhow::bail!("Missing required field: {}", field_name);
                }
            }
        }
    }
    
    Ok(())
}