//! Context building for LLM requests

use anyhow::Context;
use crate::db::models::{Target, Finding, Command, PortInfo, TargetMetadata};

/// Build the system prompt with context from database
pub fn build_system_prompt(
    session_name: &str,
    session_created: &str,
    targets_count: i64,
    active_target: &Target,
    findings: &[Finding],
    _recent_commands: &[Command],
    command_count: i64,
    last_activity: Option<String>,
) -> String {
    let ports_json: Vec<PortInfo> = active_target
        .ports_json
        .as_ref()
        .and_then(|p| serde_json::from_str(p).ok())
        .unwrap_or_default();

    let metadata: TargetMetadata = active_target
        .metadata_json
        .as_ref()
        .and_then(|m| serde_json::from_str(m).ok())
        .unwrap_or_default();

    let mut prompt = String::from(
        r#"You are an expert penetration tester working an active session.
Be concise. Respond ONLY with JSON: {"command": "...", "explanation": "..."}
If no shell command is needed, set command to null.

## Session progress
Session: "#,
    );
    prompt.push_str(session_name);
    prompt.push_str("\nStarted: ");
    prompt.push_str(session_created);
    prompt.push_str("\nTargets in scope: ");
    prompt.push_str(&targets_count.to_string());

    prompt.push_str("\n\n## Active target: ");
    prompt.push_str(&active_target.ip_host);

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

    prompt.push_str("\n\n## What we have done on this target");
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

## What has NOT been done yet (infer from findings and ports)
Suggest next logical step. Do not repeat commands already run.
Do not start from scratch — build on what is already known."#,
    );

    prompt
}

/// Post-execution analysis prompt
pub const POST_EXEC_PROMPT: &str = "In 2 sentences max: what is interesting in this output? List any new IPs, open ports, or software versions found.";

/// Parse LLM response JSON into typed suggestion
pub fn parse_llm_response(response: &str) -> anyhow::Result<crate::llm::types::LlmSuggestion> {
    let parsed: serde_json::Value = serde_json::from_str(response)
        .context("Failed to parse LLM response as JSON")?;
    
    let command = parsed.get("command").and_then(|v| v.as_str()).map(|s| s.to_string());
    let explanation = parsed.get("explanation").and_then(|v| v.as_str()).unwrap_or("").to_string();
    
    let suggestion = crate::llm::types::LlmSuggestion { command, explanation };
    suggestion.validate()?;
    Ok(suggestion)
}