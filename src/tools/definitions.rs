//! Tool definitions and execution for voidrecon LLM

use crate::llm::types::ToolDefinition;

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

