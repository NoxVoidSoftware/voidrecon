//! Context building for LLM requests

use anyhow::Result;
use crate::db::models::{Target, Finding, Command, PortInfo, TargetMetadata};

/// Build the system prompt with context from database
pub fn build_system_prompt(
    session_name: &str,
    session_created: &str,
    targets_count: i64,
    active_target: Option<&Target>,
    findings: &[Finding],
    _recent_commands: &[Command],
    command_count: i64,
    last_activity: Option<String>,
    workspace_path: Option<&std::path::Path>,
) -> String {
    let mut prompt = String::from(
        r#"You are an expert cybersecurity auditor and penetration tester in voidrecon.
Be concise and actionable. If suggesting a shell command, format your response as JSON: {"command": "...", "explanation": "..."}
If answering a general question or explaining concepts, provide clear direct assistance.
"#,
    );
    prompt.push_str("\n## Session progress\nSession: ");
    prompt.push_str(session_name);
    prompt.push_str("\nStarted: ");
    prompt.push_str(session_created);
    prompt.push_str("\nTargets in scope: ");
    prompt.push_str(&targets_count.to_string());

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

        prompt.push_str(
            r#"

Exploit Workflow:
1. To inspect already-saved exploits, call `read_exploit_script` with its filename.
2. To create or refine an exploit, call `save_exploit_script`. Voidrecon automatically runs a Python syntax check (`py_compile`).
3. To test or execute an exploit, call `execute_exploit_script`. Voidrecon will ask the user for Action Approval before running."#,
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
    let trimmed = response.trim();
    // Try to extract JSON object if wrapped in markdown code blocks or text
    let json_slice = if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            if end > start {
                &trimmed[start..=end]
            } else {
                trimmed
            }
        } else {
            trimmed
        }
    } else {
        trimmed
    };

    let suggestion = if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_slice) {
        let command = parsed.get("command").and_then(|v| v.as_str()).map(|s| s.to_string());
        let explanation = parsed.get("explanation").and_then(|v| v.as_str()).unwrap_or(trimmed).to_string();
        crate::llm::types::LlmSuggestion { command, explanation }
    } else {
        crate::llm::types::LlmSuggestion {
            command: None,
            explanation: trimmed.to_string(),
        }
    };

    let _ = suggestion.validate();
    Ok(suggestion)
}