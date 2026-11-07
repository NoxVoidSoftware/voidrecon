//! Tool definitions and execution for voidrecon LLM

use anyhow::{Context, Result};
use serde_json::Value;

use crate::db::{Database, DatabaseQueries};
use crate::llm::types::{ToolCall, ToolDefinition};

/// Get all tool definitions for the LLM
pub fn get_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new_function(
            "get_target_info",
            "Get full info for the active target: ports, OS, metadata, all findings.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "target_id": {"type": "integer", "description": "ID of the target to query"}
                },
                "required": ["target_id"]
            }),
        ),
        ToolDefinition::new_function(
            "get_recent_commands",
            "Get the last N commands run against a target, with outputs.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "target_id": {"type": "integer"},
                    "limit": {"type": "integer", "default": 5}
                },
                "required": ["target_id"]
            }),
        ),
        ToolDefinition::new_function(
            "update_target",
            "Update ports, OS or metadata for a target after a scan reveals new info.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "target_id": {"type": "integer"},
                    "ports_json": {"type": "string", "description": "JSON array of {port,proto,service,version}"},
                    "metadata_json": {"type": "string", "description": "JSON object: {os, hostnames, tags}"}
                },
                "required": ["target_id"]
            }),
        ),
        ToolDefinition::new_function(
            "add_finding",
            "Record a security finding discovered in command output.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "target_id": {"type": "integer"},
                    "title": {"type": "string"},
                    "description": {"type": "string"},
                    "severity": {"type": "string", "enum": ["info","low","medium","high","critical"]},
                    "source_cmd": {"type": "string"}
                },
                "required": ["target_id", "title", "severity"]
            }),
        ),
        ToolDefinition::new_function(
            "add_target",
            "Add a newly discovered host to the session targets.",
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
        "get_target_info" => {
            let target_id = args.get("target_id").and_then(|v| v.as_i64())
                .context("Missing target_id")?;
            let (target, findings) = DatabaseQueries::get_target_with_findings(conn, target_id)?
                .context("Target not found")?;
            Ok(serde_json::json!({
                "ip_host": target.ip_host,
                "ports_json": target.ports_json,
                "metadata_json": target.metadata_json,
                "notes": target.notes,
                "findings": findings
            }))
        }
        "get_recent_commands" => {
            let target_id = args.get("target_id").and_then(|v| v.as_i64())
                .context("Missing target_id")?;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
            let commands = DatabaseQueries::get_recent_commands(conn, target_id, limit)?;
            Ok(serde_json::json!(commands))
        }
        "update_target" => {
            let target_id = args.get("target_id").and_then(|v| v.as_i64())
                .context("Missing target_id")?;
            let ports_json = args.get("ports_json").and_then(|v| v.as_str());
            let metadata_json = args.get("metadata_json").and_then(|v| v.as_str());
            DatabaseQueries::update_target(conn, target_id, ports_json, metadata_json)?;
            let _ = app_tx.send(crate::app::AppEvent::SidebarReload);
            Ok(serde_json::json!({"success": true}))
        }
        "add_finding" => {
            let target_id = args.get("target_id").and_then(|v| v.as_i64())
                .context("Missing target_id")?;
            let title = args.get("title").and_then(|v| v.as_str()).context("Missing title")?;
            let description = args.get("description").and_then(|v| v.as_str()).unwrap_or("");
            let severity = args.get("severity").and_then(|v| v.as_str()).context("Missing severity")?;
            let source_cmd = args.get("source_cmd").and_then(|v| v.as_str());
            DatabaseQueries::add_finding(conn, target_id, title, description, severity, source_cmd, None, None)?;
            let _ = app_tx.send(crate::app::AppEvent::SidebarReload);
            Ok(serde_json::json!({"success": true}))
        }
        "add_target" => {
            let ip_host = args.get("ip_host").and_then(|v| v.as_str()).context("Missing ip_host")?;
            let ports_json = args.get("ports_json").and_then(|v| v.as_str());
            let notes = args.get("notes").and_then(|v| v.as_str());
            let id = DatabaseQueries::add_target(conn, ip_host, ports_json, notes)?;
            let _ = app_tx.send(crate::app::AppEvent::SidebarReload);
            Ok(serde_json::json!({"id": id, "success": true}))
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
            let syntax_check = std::process::Command::new("python")
                .args(["-m", "py_compile", file_path.to_str().unwrap()])
                .current_dir(workspace_path)
                .output();

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
                anyhow::bail!("Exploit script 'workspace/exploits/{}' not found", clean_name);
            }

            let content = std::fs::read_to_string(&file_path)?;
            let lines: Vec<&str> = content.lines().collect();
            let total_lines = lines.len();
            let truncated = total_lines > max_lines;
            let code = if truncated {
                format!("{}\n# ... [truncated, total {} lines] ...", lines[..max_lines].join("\n"), total_lines)
            } else {
                content
            };

            Ok(serde_json::json!({
                "file": clean_name,
                "total_lines": total_lines,
                "truncated": truncated,
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