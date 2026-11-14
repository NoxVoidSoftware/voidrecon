//! Sidebar data loading and target/finding listings

use anyhow::{Context, Result};

use crate::app::state::{App, SidebarNode};
use crate::db::models::Finding;
use crate::db::DatabaseQueries;
use crate::llm::ChatMessage;

impl App {
    pub async fn reload_sidebar(&mut self) -> Result<()> {
        let db = self.db.clone();
        let workspace_path = self.session_meta.workspace_path();
        let session_path = self.session_meta.path();
        let active_target_id = self.active_target_id;

        let result = tokio::task::spawn_blocking(move || -> Result<Vec<SidebarNode>> {
            let _ = DatabaseQueries::deduplicate_targets_and_findings(&db.conn());
            let targets = DatabaseQueries::get_all_targets(&db.conn())?;
            let mut nodes = Vec::new();

            for target in targets {
                let findings = DatabaseQueries::get_findings_for_target(&db.conn(), target.id)?;
                let mut by_severity: std::collections::HashMap<String, Vec<Finding>> = std::collections::HashMap::new();
                for finding in findings {
                    // If finding references an evidence file in workspace/evidence, verify it still exists on disk.
                    // If user deleted the file manually, automatically prune the orphaned finding from the DB.
                    if let Some(ref ep) = finding.evidence_path {
                        if ep.starts_with("workspace/evidence") || ep.starts_with("evidence") {
                            let rel = ep.trim_start_matches("workspace/").trim_start_matches("workspace\\");
                            let sub = rel.trim_start_matches("evidence/").trim_start_matches("evidence\\");
                            let full_p = workspace_path.join("evidence").join(sub);
                            let direct_p = session_path.join(ep);
                            if !full_p.exists() && !direct_p.exists() {
                                let _ = DatabaseQueries::delete_finding(&db.conn(), finding.id);
                                continue;
                            }
                        }
                    }
                    by_severity.entry(finding.severity.clone()).or_default().push(finding);
                }
                let mut findings_by_severity: Vec<_> = by_severity.into_iter().collect();
                findings_by_severity.sort_by(|a, b| {
                    let severity_order = |s: &str| match s {
                        "critical" => 0,
                        "high" => 1,
                        "medium" => 2,
                        "low" => 3,
                        "info" => 4,
                        _ => 5,
                    };
                    severity_order(&a.0).cmp(&severity_order(&b.0))
                });

                let ports = if let Some(ports_json) = &target.ports_json {
                    serde_json::from_str::<Vec<crate::db::models::PortInfo>>(ports_json).unwrap_or_default()
                } else {
                    Vec::new()
                };

                let facts = DatabaseQueries::get_important_facts(&db.conn(), target.id)?;
                nodes.push(SidebarNode {
                    target,
                    ports,
                    expanded: true,
                    findings_by_severity,
                    facts,
                });
            }
            Ok(nodes)
        })
        .await
        .context("Sidebar reload task panicked")??;

        // Determine active target validity from the reloaded data
        if let Some(act_id) = active_target_id {
            if !result.iter().any(|n| n.target.id == act_id) {
                self.active_target_id = result.first().map(|n| n.target.id);
            }
        }
        self.sidebar_nodes = result;

        // If no active target, select first
        if self.active_target_id.is_none() && !self.sidebar_nodes.is_empty() {
            self.active_target_id = Some(self.sidebar_nodes[0].target.id);
        }

        Ok(())
    }

    pub async fn show_targets_table(&mut self) -> Result<()> {
        // Blocking SQLite queries run off the event loop.
        let db = self.db.clone();
        let rows = tokio::task::spawn_blocking(move || -> Result<Vec<(i64, String, usize, usize)>> {
            let targets = DatabaseQueries::get_all_targets(&db.conn())?;
            let mut rows = Vec::with_capacity(targets.len());
            for target in targets {
                let ports_count = target.ports_json.as_ref()
                    .and_then(|p| serde_json::from_str::<Vec<crate::db::models::PortInfo>>(p).ok())
                    .map(|v| v.len())
                    .unwrap_or(0);
                let findings_count = DatabaseQueries::get_findings_for_target(&db.conn(), target.id)?.len();
                rows.push((target.id, target.ip_host, ports_count, findings_count));
            }
            Ok(rows)
        })
        .await
        .context("Targets table task panicked")??;

        let mut table = String::from("ID | IP/Host | Ports | Findings\n");
        table.push_str("---|---------|-------|---------\n");
        for (id, ip_host, ports_count, findings_count) in rows {
            table.push_str(&format!("{} | {} | {} | {}\n", id, ip_host, ports_count, findings_count));
        }
        self.append_chat_message(ChatMessage {
            role: "assistant".to_string(),
            content: table,
            tool_calls: None,
            tool_call_id: None,
        }).await;
        Ok(())
    }
    
    pub async fn show_findings(&mut self, target_id: Option<i64>) -> Result<()> {
        let target_id = target_id.or(self.active_target_id);
        if let Some(id) = target_id {
            // Blocking SQLite queries run off the event loop.
            let db = self.db.clone();
            let findings = tokio::task::spawn_blocking(move || -> Result<Vec<Finding>> {
                DatabaseQueries::get_findings_for_target(&db.conn(), id)
            })
            .await
            .context("Findings task panicked")??;
            let mut output = String::from("Findings:\n");
            for f in findings {
                output.push_str(&format!("[{}] {}: {} (from: {})\n", f.severity, f.title, f.description, f.source_cmd.unwrap_or_default()));
            }
            self.append_chat_message(ChatMessage {
                role: "assistant".to_string(),
                content: output,
                tool_calls: None,
                tool_call_id: None,
            }).await;
        }
        Ok(())
    }
}
