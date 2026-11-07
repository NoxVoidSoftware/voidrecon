//! Context building for LLM requests

use anyhow::Result;
use crate::db::models::{Target, Finding, Command, PortInfo, TargetMetadata};

/// Build the system prompt with context from database
pub fn build_system_prompt(
    session_name: &str,
    session_created: &str,
    targets_count: i64,
    all_targets: &[Target],
    active_target: Option<&Target>,
    findings: &[Finding],
    _recent_commands: &[Command],
    command_count: i64,
    last_activity: Option<String>,
    workspace_path: Option<&std::path::Path>,
) -> String {
    let mut prompt = String::from(
        r#"You are an expert penetration tester working an active session in voidrecon.
Be concise.
- When the user asks you to do something, perform a test, or run a scan:
  Respond ONLY with JSON: {"command": "<shell_command>", "explanation": "<concise explanation>"}
- When the user asks an informational question (about targets, findings, status, or past outputs):
  Inspect the database using the available read tools (get_target_info, get_findings, list_targets, get_recent_commands, read_workspace_file) and answer clearly. Set "command": null.
- If no shell command is needed, set command to null.
Never output raw monologues before the JSON.
"#,
    );
    prompt.push_str("\n## Session progress\nSession: ");
    prompt.push_str(session_name);
    prompt.push_str("\nStarted: ");
    prompt.push_str(session_created);
    prompt.push_str("\nTargets in scope: ");
    prompt.push_str(&targets_count.to_string());

    if !all_targets.is_empty() {
        prompt.push_str("\nAll targets in scope:");
        for t in all_targets {
            let is_act = active_target.map(|a| a.id == t.id).unwrap_or(false);
            let marker = if is_act { " [ACTIVE]" } else { "" };
            prompt.push_str(&format!("\n  - ID {}: {}{}", t.id, t.ip_host, marker));
        }
    }

    if let Some(ws) = workspace_path {
        prompt.push_str("\n\n## Session Workspace: ");
        prompt.push_str(&ws.to_string_lossy());
        prompt.push_str(
            r#"
Workspace structure:
  - outputs/   : Raw outputs of executed commands
  - evidence/  : Reconnaissance & scan evidence
  - artifacts/ : Reports and assessment notes
  - exploits/  : Python exploit & PoC scripts"#,
        );

        let exploits_dir = ws.join("exploits");
        let mut exploits_list = Vec::new();
        if let Ok(entries) = std::fs::read_dir(exploits_dir) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        let fname = entry.file_name().to_string_lossy().to_string();
                        if fname.ends_with(".py") {
                            exploits_list.push(format!("  - {} ({} bytes) -> `python workspace/exploits/{}`", fname, meta.len(), fname));
                        }
                    }
                }
            }
        }

        if !exploits_list.is_empty() {
            prompt.push_str("\n\n## Saved Exploits in Workspace:\n");
            prompt.push_str(&exploits_list.join("\n"));
        } else {
            prompt.push_str("\n\n## Saved Exploits:\n  (None saved yet. Use `save_exploit_script` when you identify or write an exploit).");
        }

        let evidence_dir = ws.join("evidence");
        let mut evidence_list = Vec::new();
        if let Ok(entries) = std::fs::read_dir(evidence_dir) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        let fname = entry.file_name().to_string_lossy().to_string();
                        evidence_list.push(format!("  - evidence/{} ({} bytes)", fname, meta.len()));
                    }
                }
            }
        }
        if !evidence_list.is_empty() {
            prompt.push_str("\n\n## Saved Evidence in Workspace:\n");
            prompt.push_str(&evidence_list.join("\n"));
        }

        let artifacts_dir = ws.join("artifacts");
        let mut artifacts_list = Vec::new();
        if let Ok(entries) = std::fs::read_dir(artifacts_dir) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        let fname = entry.file_name().to_string_lossy().to_string();
                        artifacts_list.push(format!("  - artifacts/{} ({} bytes)", fname, meta.len()));
                    }
                }
            }
        }
        if !artifacts_list.is_empty() {
            prompt.push_str("\n\n## Saved Artifacts in Workspace:\n");
            prompt.push_str(&artifacts_list.join("\n"));
        }

        prompt.push_str(
            r#"

Tools & Workspace Workflow:
1. `set_target`: Set or switch the active target in scope (e.g. `set_target(ip_host="192.168.1.1")`).
2. `list_targets` / `get_target_info` / `get_findings`: Query targets, ports, and findings stored in the database.
3. `update_target` / `add_finding`: Record discovered ports, OS details, and vulnerabilities into the database so they persist.
4. `save_workspace_file`: Save evidence into `workspace/evidence/` (automatically linked as finding in DB) or artifacts into `workspace/artifacts/`.
5. `read_workspace_file`: Read full raw command outputs (`workspace/outputs/<uuid>.txt`) or saved evidence/artifacts.
6. `save_exploit_script` / `execute_exploit_script`: Save Python exploits with automatic syntax validation and execute with user Action Approval."#,
        );
    }

    if let Some(target) = active_target {
        let ports_json: Vec<PortInfo> = target
            .ports_json
            .as_ref()
            .and_then(|p| serde_json::from_str(p).ok())
            .unwrap_or_default();

        let metadata: TargetMetadata = target
            .metadata_json
            .as_ref()
            .and_then(|m| serde_json::from_str(m).ok())
            .unwrap_or_default();

        prompt.push_str("\n\n## Active target: ");
        prompt.push_str(&target.ip_host);

        if let Some(os) = metadata.os {
            prompt.push_str("\nOS: ");
            prompt.push_str(&os);
        }
        if let Some(hostnames) = metadata.hostnames {
            if !hostnames.is_empty() {
                prompt.push_str("\nHostnames: ");
                prompt.push_str(&hostnames.join(", "));
            }
        }
        if !ports_json.is_empty() {
            prompt.push_str("\nOpen ports:");
            for port in ports_json {
                prompt.push_str(&format!(
                    "\n  {}/{}  {}  {}",
                    port.port, port.proto, port.service, port.version
                ));
            }
        }

        prompt.push_str("\n\n## Activity on this target");
        prompt.push_str("\nCommands run: ");
        prompt.push_str(&command_count.to_string());
        if let Some(activity) = last_activity {
            prompt.push_str("\nLast activity: ");
            prompt.push_str(&activity);
        }

        if !findings.is_empty() {
            prompt.push_str("\n\nFindings so far (");
            prompt.push_str(&findings.len().to_string());
            prompt.push_str(" total):");
            for finding in findings {
                prompt.push_str(&format!(
                    "\n  [{}] {} — {}",
                    finding.severity, finding.title, finding.description
                ));
            }
        }

        prompt.push_str(
            r#"

## Next steps
Suggest the next logical step based on findings and open ports. Do not repeat commands already run."#,
        );
    } else {
        prompt.push_str("\n\nNote: No specific target is currently selected. Assist the user with discovery, tool guidance, or questions.");
    }

    prompt
}

/// Post-execution analysis prompt
pub const POST_EXEC_PROMPT: &str = "In 2 sentences max: what is interesting in this output? List any new IPs, open ports, or software versions found.";

/// Parse LLM response JSON into typed suggestion
pub fn parse_llm_response(response: &str) -> Result<crate::llm::types::LlmSuggestion> {
    let mut text = response.trim();

    // Strip <think>...</think> if present (reasoning models)
    let cleaned_text;
    if let Some(think_start) = text.find("<think>") {
        if let Some(think_end) = text.find("</think>") {
            if think_end > think_start {
                cleaned_text = format!("{}{}", &text[..think_start], &text[think_end + 8..]);
                text = cleaned_text.trim();
            }
        }
    }

    let suggestion = 'parse: {
        // 1. Direct JSON parse
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(text) {
            if val.is_object() {
                let command = extract_clean_command(&val);
                let explanation = val.get("explanation").and_then(|v| v.as_str()).unwrap_or(text).to_string();
                break 'parse crate::llm::types::LlmSuggestion { command, explanation };
            }
        }

        // 2. Markdown block: ```json ... ```
        if let Some(start) = text.rfind("```json") {
            let after = &text[start + 7..];
            if let Some(end) = after.find("```") {
                let candidate = after[..end].trim();
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(candidate) {
                    if val.is_object() {
                        let command = extract_clean_command(&val);
                        let explanation = val.get("explanation").and_then(|v| v.as_str()).unwrap_or(candidate).to_string();
                        break 'parse crate::llm::types::LlmSuggestion { command, explanation };
                    }
                }
            }
        }

        // 3. Scan backwards from the end for the last valid JSON object containing "command" or "explanation"
        let mut search_end = text.len();
        while let Some(end_idx) = text[..search_end].rfind('}') {
            let mut depth = 0;
            let mut start_idx = None;
            let bytes = text.as_bytes();
            for i in (0..=end_idx).rev() {
                if bytes[i] == b'}' {
                    depth += 1;
                } else if bytes[i] == b'{' {
                    depth -= 1;
                    if depth == 0 {
                        start_idx = Some(i);
                        break;
                    }
                }
            }

            if let Some(start) = start_idx {
                let candidate = &text[start..=end_idx];
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(candidate) {
                    if val.is_object() && (val.get("command").is_some() || val.get("explanation").is_some()) {
                        let command = extract_clean_command(&val);
                        let mut explanation = val.get("explanation").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        if explanation.is_empty() {
                            let preamble = text[..start].trim();
                            explanation = if !preamble.is_empty() {
                                preamble.to_string()
                            } else {
                                "Command suggested".to_string()
                            };
                        }
                        break 'parse crate::llm::types::LlmSuggestion { command, explanation };
                    }
                }
                search_end = start;
            } else {
                break;
            }
        }

        // Fallback: plain text answer with no command
        crate::llm::types::LlmSuggestion {
            command: None,
            explanation: text.to_string(),
        }
    };

    let _ = suggestion.validate();
    Ok(suggestion)
}

fn extract_clean_command(val: &serde_json::Value) -> Option<String> {
    val.get("command").and_then(|v| {
        if v.is_null() {
            None
        } else {
            v.as_str().and_then(|s| {
                let trimmed = s.trim();
                let clean = trimmed.trim_start_matches('.').trim();
                if clean.is_empty() || clean.eq_ignore_ascii_case("null") {
                    None
                } else {
                    Some(clean.to_string())
                }
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_nemotron_mixed_output() {
        let raw = r#"The user wants to scan for open ports on the target 10.129.90.124. I need to run a port scan. Since I'm in a cybersecurity auditing context, I should use nmap for a comprehensive scan. I'll run a TCP SYN scan with service version detection on all ports. I'll use the command tool to execute nmap. However, I don't have a direct "run command" tool; I have to use the tools provided. The tools are for managing the session, not for executing arbitrary commands. But the user expects me to run a scan. I need to use the available tools: I can save workspace files, but not execute commands directly. Wait, the system prompt says: "If suggesting a shell command, format your response as JSON: {"command": "...", "explanation": "..."}". So I can suggest a shell command for the user to run. But the user is asking me to scan for open ports. I should provide the nmap command for them to run. However, the user might be expecting me to run it via some tool. Looking at the tools, there's no "execute_command" tool. The tools are for database management and file saving. The user is in a session where they can run commands themselves. I should suggest the nmap command. Let me provide a JSON with the command. I'll suggest a comprehensive nmap scan: nmap -sS -sV -p- 10.129.90.124. Also maybe add -T4 for speed. I'll output the JSON{"command": ".nmap -sS -sV -p- -T4 10.129.90.124", "explanation": "Perform a comprehensive TCP SYN scan with service version detection on all 65535 ports. -sS:SYN scan (stealthy), -sV: version detection, -p-: scan all ports, -T4: faster timing template."}"#;

        let res = parse_llm_response(raw).unwrap();
        assert_eq!(res.command.as_deref(), Some("nmap -sS -sV -p- -T4 10.129.90.124"));
        assert!(res.explanation.contains("Perform a comprehensive TCP SYN scan"));
    }

    #[test]
    fn test_parse_clean_json() {
        let raw = r#"{"command": "whoami", "explanation": "Check current user privileges"}"#;
        let res = parse_llm_response(raw).unwrap();
        assert_eq!(res.command.as_deref(), Some("whoami"));
        assert_eq!(res.explanation, "Check current user privileges");
    }

    #[test]
    fn test_parse_null_command() {
        let raw = r#"{"command": null, "explanation": "Target 10.129.90.124 has ports 22 and 80 open."}"#;
        let res = parse_llm_response(raw).unwrap();
        assert!(res.command.is_none());
        assert_eq!(res.explanation, "Target 10.129.90.124 has ports 22 and 80 open.");
    }
}