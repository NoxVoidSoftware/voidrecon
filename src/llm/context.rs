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
- To execute or run any command or scan (nmap, gobuster, nikto, hydra, curl, whoami, etc.):
  Call the `run_command` tool with the command and explanation.
  The command will be presented to the user in the TUI Suggestion panel ([r]un / [e]dit / [s]kip) for interactive execution.
  NEVER write commands to files to execute them.
- When saving any files, artifacts, evidence, or exploits into the workspace:
  Call `save_workspace_file` or `save_exploit_script`. NOTE: Saving requires interactive user Action Approval in the TUI before any file is written.
- To read existing workspace files (command outputs, saved evidence, artifacts, or exploits):
  Call `read_workspace_file`.
- To delete evidence or clean up workspace files and their database entries:
  Call `delete_evidence(filename="...")` or `delete_workspace_file(folder="...", filename="...")`. This deletes the file from disk and removes the corresponding finding from the database and targets list.
- When answering informational questions (about targets, findings, open ports, status):
  Use database read tools (`get_target_info`, `get_findings`, `list_targets`, `get_recent_commands`) and answer clearly.
- Voidrecon automatically archives all command outputs into `workspace/outputs/`.
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
1. `run_command`: Suggest or run a shell command/scan (nmap, gobuster, nikto, curl). The user is prompted in the TUI to approve and run it.
2. `set_target`: Set or switch the active target in scope (e.g. `set_target(ip_host="192.168.1.1")`).
3. `list_targets` / `get_target_info` / `get_findings`: Query targets, ports, and findings stored in the database.
4. `update_target` / `add_finding`: Record discovered ports, OS details, and vulnerabilities into the database.
5. `save_workspace_file`: Save evidence or artifacts into `workspace/evidence/` or `workspace/artifacts/` (requires user Action Approval).
6. `read_workspace_file`: Read full raw command outputs (`workspace/outputs/<uuid>.txt`) or saved evidence/artifacts.
7. `delete_evidence` / `delete_workspace_file`: Delete an evidence/workspace file and remove its finding from the database so it disappears from the targets list.
8. `save_exploit_script` / `execute_exploit_script`: Save Python exploits into `workspace/exploits/` and execute with Action Approval."#,
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
pub const POST_EXEC_PROMPT: &str = r#"Analyze the command output in the context of our ongoing assessment.
- If the output reveals new open ports, services, or OS details for the target, invoke `update_target`. Do NOT re-add existing targets with `add_target`.
- If new vulnerabilities are found, invoke `add_finding`. Do NOT re-add findings already listed above.
- STRICTLY RESPECT USER INSTRUCTIONS: Review previous conversation messages. If the user asked not to use certain tools, flags, or techniques, do NOT suggest or invoke them.
- Provide a concise 1-2 sentence summary, and if appropriate, suggest the next logical pentest step via `run_command`."#;

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
                } else if let Some(sug) = extract_malformed_suggestion(candidate, &text[..start]) {
                    break 'parse sug;
                }
                search_end = start;
            } else {
                break;
            }
        }

        // Fallback: check if malformed JSON exists anywhere in text
        if let Some(start) = text.find('{') {
            if let Some(end) = text.rfind('}') {
                if end > start {
                    if let Some(sug) = extract_malformed_suggestion(&text[start..=end], &text[..start]) {
                        break 'parse sug;
                    }
                }
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

fn extract_malformed_suggestion(candidate: &str, preamble: &str) -> Option<crate::llm::types::LlmSuggestion> {
    let cmd_key = "\"command\"";
    let cmd_idx = candidate.find(cmd_key)?;
    let after_cmd_key = &candidate[cmd_idx + cmd_key.len()..];
    let colon_idx = after_cmd_key.find(':')?;
    let val_after = after_cmd_key[colon_idx + 1..].trim_start();

    if val_after.starts_with("null") {
        let explanation = extract_explanation_string(candidate).unwrap_or_else(|| {
            if !preamble.is_empty() {
                preamble.to_string()
            } else {
                "Information query answered".to_string()
            }
        });
        return Some(crate::llm::types::LlmSuggestion {
            command: None,
            explanation,
        });
    }

    let raw_cmd = if let Some(expl_idx) = candidate.find("\"explanation\"") {
        if expl_idx > cmd_idx {
            let slice = &candidate[cmd_idx + cmd_key.len() + colon_idx + 1..expl_idx];
            slice.trim().trim_end_matches(',').trim()
        } else {
            val_after.trim_end_matches('}').trim()
        }
    } else {
        val_after.trim_end_matches('}').trim()
    };

    let clean_cmd = clean_extracted_command(raw_cmd);
    if clean_cmd.is_empty() {
        return None;
    }

    let explanation = extract_explanation_string(candidate).unwrap_or_else(|| {
        if !preamble.is_empty() {
            preamble.to_string()
        } else {
            "Suggested command for execution".to_string()
        }
    });

    Some(crate::llm::types::LlmSuggestion {
        command: Some(clean_cmd),
        explanation,
    })
}

fn clean_extracted_command(raw: &str) -> String {
    let mut s = raw.trim();
    if s.starts_with('"') || s.starts_with('\'') {
        s = &s[1..];
    }
    if s.ends_with('"') || s.ends_with('\'') {
        s = &s[..s.len().saturating_sub(1)];
    }
    s = s.trim();

    if let Some(dot_idx) = s.find('.') {
        let prefix = &s[..dot_idx];
        if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
            s = s[dot_idx + 1..].trim_start();
        }
    }

    s = s.trim_matches(|c: char| c == '"' || c == '\'' || c == '.' || c.is_whitespace());
    s.to_string()
}

fn extract_explanation_string(candidate: &str) -> Option<String> {
    let expl_key = "\"explanation\"";
    let expl_idx = candidate.find(expl_key)?;
    let after_key = &candidate[expl_idx + expl_key.len()..];
    let colon_idx = after_key.find(':')?;
    let val_after = after_key[colon_idx + 1..].trim();
    let trimmed = val_after.trim_end_matches('}').trim();
    let cleaned = trimmed.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
    if !cleaned.is_empty() {
        Some(cleaned.to_string())
    } else {
        None
    }
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

    #[test]
    fn test_parse_nemotron_malformed_quote_json() {
        let raw = r#"The user wants to run an nmap scan. I need to execute a proper nmap command. Let me run a comprehensive nmap scan against the target 10.129.90.12{"command": "4. "nmap -sS -sV -O -p- -T4 10.129.90.124", "explanation": "Comprehensive nmap scan: SYN scan, service version detection, OS detection, all ports, fast timing"}"#;
        let res = parse_llm_response(raw).unwrap();
        assert_eq!(res.command.as_deref(), Some("nmap -sS -sV -O -p- -T4 10.129.90.124"));
        assert_eq!(res.explanation, "Comprehensive nmap scan: SYN scan, service version detection, OS detection, all ports, fast timing");
    }
}