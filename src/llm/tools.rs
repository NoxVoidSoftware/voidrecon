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
    ]
}

/// Execute a tool call and return the result
pub async fn execute_tool_call(
    tool_call: &ToolCall,
    db: &Database,
    _active_target_id: Option<i64>,
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