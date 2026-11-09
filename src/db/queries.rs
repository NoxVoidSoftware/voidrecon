//! Database queries for voidrecon

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use uuid::Uuid;

use crate::db::models::{Command, Finding, ImportantFact, Target};

/// Parse datetime from SQLite TEXT
fn parse_datetime(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .or_else(|_| DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S"))
        .or_else(|_| DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f"))
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

/// Truncate string to max length with ellipsis
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

/// Truncate output for preview: first 250 + truncation marker + last 250
fn truncate_output(s: &str) -> String {
    const MAX_LEN: usize = 500;
    const HALF: usize = 250;
    if s.len() <= MAX_LEN {
        s.to_string()
    } else {
        format!(
            "{}\n...[truncated]...\n{}",
            &s[..HALF],
            &s[s.len() - HALF..]
        )
    }
}

/// Compute SHA-256 hash of content
fn compute_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Save full output to workspace/outputs/ and return relative path and hash
fn save_output_artifact(session_path: &Path, output: &str) -> Result<(String, String)> {
    let outputs_dir = session_path.join("workspace").join("outputs");
    fs::create_dir_all(&outputs_dir).context("Failed to create outputs directory")?;
    
    let filename = format!("{}.txt", Uuid::new_v4());
    let filepath = outputs_dir.join(&filename);
    fs::write(&filepath, output).context("Failed to write output artifact")?;
    
    let hash = compute_hash(output);
    let relative_path = format!("workspace/outputs/{}", filename);
    
    Ok((relative_path, hash))
}

pub struct DatabaseQueries;

pub fn normalize_target_host(raw: &str) -> String {
    let mut s = raw.trim();
    if let Some(idx) = s.find("://") {
        s = &s[idx + 3..];
    }
    if let Some(slash) = s.find('/') {
        s = &s[..slash];
    }
    if let Some(colon) = s.rfind(':') {
        if !s.contains(']') {
            s = &s[..colon];
        }
    }
    s.trim_matches(|c| c == '\'' || c == '"' || c == '/' || c == '[' || c == ']').to_lowercase()
}

impl DatabaseQueries {
    // Target operations
    pub fn add_target(conn: &Connection, ip_host: &str, ports_json: Option<&str>, notes: Option<&str>) -> Result<i64> {
        let normalized = normalize_target_host(ip_host);
        let host_to_store = if normalized.is_empty() { ip_host.trim().to_string() } else { normalized };

        if let Ok(Some(existing)) = Self::get_target_by_ip(conn, &host_to_store) {
            if ports_json.is_some() || notes.is_some() {
                let _ = Self::update_target(conn, existing.id, ports_json, None);
            }
            return Ok(existing.id);
        }

        conn.execute(
            "INSERT INTO targets (ip_host, ports_json, notes) VALUES (?1, ?2, ?3)",
            params![host_to_store, ports_json, notes],
        )
        .context("Failed to insert target")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn update_target(conn: &Connection, id: i64, ports_json: Option<&str>, metadata_json: Option<&str>) -> Result<()> {
        conn.execute(
            "UPDATE targets SET ports_json = COALESCE(?1, ports_json), metadata_json = COALESCE(?2, metadata_json) WHERE id = ?3",
            params![ports_json, metadata_json, id],
        )
        .context("Failed to update target")?;
        Ok(())
    }

    pub fn get_target(conn: &Connection, id: i64) -> Result<Option<Target>> {
        conn.query_row(
            "SELECT id, ip_host, ports_json, metadata_json, notes, created_at FROM targets WHERE id = ?1",
            params![id],
            |row| Self::row_to_target(row),
        )
        .optional()
        .context("Failed to query target")
    }

    pub fn get_target_by_ip(conn: &Connection, ip_host: &str) -> Result<Option<Target>> {
        let normalized = normalize_target_host(ip_host);
        conn.query_row(
            "SELECT id, ip_host, ports_json, metadata_json, notes, created_at FROM targets WHERE LOWER(ip_host) = ?1 OR LOWER(ip_host) = ?2 LIMIT 1",
            params![ip_host.to_lowercase(), normalized],
            |row| Self::row_to_target(row),
        )
        .optional()
        .context("Failed to query target by IP")
    }

    pub fn get_all_targets(conn: &Connection) -> Result<Vec<Target>> {
        let mut stmt = conn
            .prepare("SELECT id, ip_host, ports_json, metadata_json, notes, created_at FROM targets ORDER BY created_at DESC")
            .context("Failed to prepare query")?;
        let targets = stmt
            .query_map([], |row| Self::row_to_target(row))
            .context("Failed to query targets")?
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to collect targets")?;
        Ok(targets)
    }

    pub fn get_target_with_findings(conn: &Connection, id: i64) -> Result<Option<(Target, Vec<Finding>)>> {
        let target = Self::get_target(conn, id)?;
        if let Some(target) = target {
            let findings = Self::get_findings_for_target(conn, id)?;
            Ok(Some((target, findings)))
        } else {
            Ok(None)
        }
    }

    fn row_to_target(row: &Row) -> rusqlite::Result<Target> {
        let created_at_str: String = row.get(5)?;
        Ok(Target {
            id: row.get(0)?,
            ip_host: row.get(1)?,
            ports_json: row.get(2)?,
            metadata_json: row.get(3)?,
            notes: row.get(4)?,
            created_at: parse_datetime(&created_at_str),
        })
    }

    // Finding operations
    pub fn add_finding(
        conn: &Connection,
        target_id: i64,
        title: &str,
        description: &str,
        severity: &str,
        source_cmd: Option<&str>,
        confidence: Option<i32>,
        evidence_path: Option<&str>,
    ) -> Result<i64> {
        let title_clean = title.trim();
        let description = truncate(description, 200);
        let confidence = confidence.unwrap_or(100).clamp(0, 100);

        // Deduplication: check if identical finding already exists for this target
        let existing_id: Option<i64> = if let Some(ep) = evidence_path {
            conn.query_row(
                "SELECT id FROM findings WHERE target_id = ?1 AND (LOWER(title) = ?2 OR (evidence_path IS NOT NULL AND evidence_path = ?3)) LIMIT 1",
                params![target_id, title_clean.to_lowercase(), ep],
                |row| row.get(0),
            ).optional().unwrap_or(None)
        } else {
            conn.query_row(
                "SELECT id FROM findings WHERE target_id = ?1 AND LOWER(title) = ?2 LIMIT 1",
                params![target_id, title_clean.to_lowercase()],
                |row| row.get(0),
            ).optional().unwrap_or(None)
        };

        if let Some(id) = existing_id {
            let _ = conn.execute(
                "UPDATE findings SET description = ?1, severity = ?2, confidence = MAX(confidence, ?3) WHERE id = ?4",
                params![description, severity, confidence, id],
            );
            return Ok(id);
        }

        conn.execute(
            "INSERT INTO findings (target_id, title, description, severity, source_cmd, confidence, evidence_path) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![target_id, title_clean, description, severity, source_cmd, confidence, evidence_path],
        )
        .context("Failed to insert finding")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn add_note(conn: &Connection, target_id: i64, text: &str) -> Result<i64> {
        Self::add_finding(conn, target_id, "note", text, "info", None, Some(100), None)
    }

    // Important facts operations
    pub fn add_important_fact(conn: &Connection, target_id: i64, fact: &str) -> Result<i64> {
        let fact_clean = fact.trim();
        if fact_clean.is_empty() {
            anyhow::bail!("Fact cannot be empty");
        }

        // Deduplication: skip identical fact for the same target
        let existing_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM important_facts WHERE target_id = ?1 AND LOWER(fact) = ?2 LIMIT 1",
                params![target_id, fact_clean.to_lowercase()],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None);

        if let Some(id) = existing_id {
            return Ok(id);
        }

        conn.execute(
            "INSERT INTO important_facts (target_id, fact) VALUES (?1, ?2)",
            params![target_id, fact_clean],
        )
        .context("Failed to insert important fact")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_important_facts(conn: &Connection, target_id: i64) -> Result<Vec<ImportantFact>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, target_id, fact, created_at FROM important_facts WHERE target_id = ?1 ORDER BY created_at ASC",
            )
            .context("Failed to prepare query")?;
        let facts = stmt
            .query_map(params![target_id], |row| {
                let created_at_str: String = row.get(3)?;
                Ok(ImportantFact {
                    id: row.get(0)?,
                    target_id: row.get(1)?,
                    fact: row.get(2)?,
                    created_at: parse_datetime(&created_at_str),
                })
            })
            .context("Failed to query important facts")?
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to collect important facts")?;
        Ok(facts)
    }

    pub fn delete_important_fact(conn: &Connection, fact_id: i64) -> Result<()> {
        conn.execute("DELETE FROM important_facts WHERE id = ?1", params![fact_id])
            .context("Failed to delete important fact")?;
        Ok(())
    }

    pub fn delete_finding(conn: &Connection, finding_id: i64) -> Result<()> {
        conn.execute("DELETE FROM findings WHERE id = ?1", params![finding_id])
            .context("Failed to delete finding")?;
        Ok(())
    }

    pub fn delete_findings_by_evidence(
        conn: &Connection,
        target_id: Option<i64>,
        filename: &str,
    ) -> Result<usize> {
        let stem = std::path::Path::new(filename)
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or(filename);
        let pattern = format!("%{}%", stem);
        let count = if let Some(tid) = target_id {
            conn.execute(
                "DELETE FROM findings WHERE target_id = ?1 AND (evidence_path LIKE ?2 OR title LIKE ?2)",
                params![tid, pattern],
            )?
        } else {
            conn.execute(
                "DELETE FROM findings WHERE evidence_path LIKE ?1 OR title LIKE ?1",
                params![pattern],
            )?
        };
        Ok(count)
    }

    pub fn get_findings_for_target(conn: &Connection, target_id: i64) -> Result<Vec<Finding>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, target_id, title, description, severity, source_cmd, confidence, evidence_path, created_at FROM findings WHERE target_id = ?1 ORDER BY created_at DESC",
            )
            .context("Failed to prepare query")?;
        let findings = stmt
            .query_map(params![target_id], |row| Self::row_to_finding(row))
            .context("Failed to query findings")?
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to collect findings")?;
        Ok(findings)
    }

    pub fn get_all_findings(conn: &Connection) -> Result<Vec<Finding>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, target_id, title, description, severity, source_cmd, confidence, evidence_path, created_at FROM findings ORDER BY created_at DESC",
            )
            .context("Failed to prepare query")?;
        let findings = stmt
            .query_map([], |row| Self::row_to_finding(row))
            .context("Failed to query all findings")?
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to collect all findings")?;
        Ok(findings)
    }

    fn row_to_finding(row: &Row) -> rusqlite::Result<Finding> {
        let created_at_str: String = row.get(8)?;
        Ok(Finding {
            id: row.get(0)?,
            target_id: row.get(1)?,
            title: row.get(2)?,
            description: row.get(3)?,
            severity: row.get(4)?,
            source_cmd: row.get(5)?,
            confidence: row.get(6)?,
            evidence_path: row.get(7)?,
            created_at: parse_datetime(&created_at_str),
        })
    }

    // Command operations
    pub fn add_command(
        conn: &Connection,
        session_path: &Path,
        target_id: Option<i64>,
        raw_cmd: &str,
        output: Option<&str>,
        exit_code: Option<i32>,
        duration_ms: Option<i64>,
    ) -> Result<i64> {
        let (output_preview, output_path, output_hash) = if let Some(out) = output {
            let preview = truncate_output(out);
            let (path, hash) = save_output_artifact(session_path, out)?;
            (Some(preview), Some(path), Some(hash))
        } else {
            (None, None, None)
        };

        conn.execute(
            "INSERT INTO commands (target_id, raw_cmd, output, output_path, output_hash, exit_code, duration_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![target_id, raw_cmd, output_preview, output_path, output_hash, exit_code, duration_ms],
        )
        .context("Failed to insert command")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn get_recent_commands(conn: &Connection, target_id: i64, limit: usize) -> Result<Vec<Command>> {
        let mut stmt = conn
            .prepare(
                "SELECT id, target_id, raw_cmd, output, output_path, output_hash, exit_code, duration_ms, executed_at FROM commands WHERE target_id = ?1 ORDER BY executed_at DESC LIMIT ?2",
            )
            .context("Failed to prepare query")?;
        let commands = stmt
            .query_map(params![target_id, limit as i64], |row| Self::row_to_command(row))
            .context("Failed to query commands")?
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to collect commands")?;
        Ok(commands)
    }

    pub fn get_command_count(conn: &Connection, target_id: i64) -> Result<i64> {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM commands WHERE target_id = ?1",
                params![target_id],
                |row| row.get(0),
            )
            .context("Failed to count commands")?;
        Ok(count)
    }

    fn row_to_command(row: &Row) -> rusqlite::Result<Command> {
        let executed_at_str: String = row.get(8)?;
        Ok(Command {
            id: row.get(0)?,
            target_id: row.get(1)?,
            raw_cmd: row.get(2)?,
            output: row.get(3)?,
            output_path: row.get(4)?,
            output_hash: row.get(5)?,
            exit_code: row.get(6)?,
            duration_ms: row.get(7)?,
            executed_at: parse_datetime(&executed_at_str),
        })
    }

    // Chat history
    pub fn add_chat_message(conn: &Connection, target_id: Option<i64>, role: &str, content: &str) -> Result<i64> {
        conn.execute(
            "INSERT INTO chat_history (target_id, role, content) VALUES (?1, ?2, ?3)",
            params![target_id, role, content],
        )
        .context("Failed to insert chat message")?;
        Ok(conn.last_insert_rowid())
    }

    // Session info
    pub fn get_targets_count(conn: &Connection) -> Result<i64> {
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM targets", [], |row| row.get(0))
            .context("Failed to count targets")?;
        Ok(count)
    }

    /// Deduplicates targets by normalized hostname/IP and deduplicates findings per target.
    /// Merges findings and ports to the canonical target record and removes orphaned duplicates.
    pub fn deduplicate_targets_and_findings(conn: &Connection) -> Result<()> {
        // 1. Group targets by normalized host
        let targets = Self::get_all_targets(conn)?;
        let mut grouped: std::collections::HashMap<String, Vec<Target>> = std::collections::HashMap::new();
        for t in targets {
            let key = normalize_target_host(&t.ip_host);
            if !key.is_empty() {
                grouped.entry(key).or_default().push(t);
            }
        }

        for (_key, group) in grouped {
            if group.len() > 1 {
                // Canonical target is the one with the lowest ID
                let canonical = match group.iter().min_by_key(|t| t.id) {
                    Some(c) => c,
                    None => continue,
                };
                let canonical_id = canonical.id;
                let duplicate_ids: Vec<i64> = group.iter()
                    .filter(|t| t.id != canonical_id)
                    .map(|t| t.id)
                    .collect();

                // Merge open ports
                let mut all_ports: Vec<crate::db::models::PortInfo> = Vec::new();
                for t in &group {
                    if let Some(ref pj) = t.ports_json {
                        if let Ok(ports) = serde_json::from_str::<Vec<crate::db::models::PortInfo>>(pj) {
                            for p in ports {
                                if !all_ports.iter().any(|ep| ep.port == p.port && ep.proto == p.proto) {
                                    all_ports.push(p);
                                }
                            }
                        }
                    }
                }
                all_ports.sort_by_key(|p| p.port);
                let ports_json_str = if !all_ports.is_empty() {
                    serde_json::to_string(&all_ports).ok()
                } else {
                    None
                };
                let _ = Self::update_target(conn, canonical_id, ports_json_str.as_deref(), None);

                // Re-point findings, commands, and chat history to canonical target
                for dup_id in duplicate_ids {
                    let _ = conn.execute("UPDATE findings SET target_id = ?1 WHERE target_id = ?2", params![canonical_id, dup_id]);
                    let _ = conn.execute("UPDATE commands SET target_id = ?1 WHERE target_id = ?2", params![canonical_id, dup_id]);
                    let _ = conn.execute("UPDATE chat_history SET target_id = ?1 WHERE target_id = ?2", params![canonical_id, dup_id]);
                    let _ = conn.execute("DELETE FROM targets WHERE id = ?1", params![dup_id]);
                }
            }
        }

        // 2. Deduplicate findings: for each target, remove duplicate findings with the same title or evidence_path
        let all_targets = Self::get_all_targets(conn)?;
        for target in all_targets {
            let findings = Self::get_findings_for_target(conn, target.id)?;
            let mut seen_titles = std::collections::HashSet::new();
            let mut seen_evidences = std::collections::HashSet::new();

            // Reverse order so we retain the newest finding ID
            for f in findings.into_iter().rev() {
                let norm_title = f.title.trim().to_lowercase();
                let is_dup_title = !seen_titles.insert(norm_title);
                
                let is_dup_evidence = if let Some(ref ep) = f.evidence_path {
                    let clean_ep = ep.trim().to_lowercase();
                    !clean_ep.is_empty() && !seen_evidences.insert(clean_ep)
                } else {
                    false
                };

                if is_dup_title || is_dup_evidence {
                    let _ = conn.execute("DELETE FROM findings WHERE id = ?1", params![f.id]);
                }
            }
        }

        Ok(())
    }

    /// Automatically parses scan outputs (such as nmap) to extract discovered open ports, services,
    /// versions, and operating system details, persisting them directly into the session database.
    pub fn parse_nmap_and_update_target(
        conn: &Connection,
        cmd: &str,
        output: &str,
        fallback_target_id: Option<i64>,
    ) -> Result<Option<i64>> {
        let is_scan = output.contains("Nmap") || cmd.contains("nmap") || output.contains("/tcp") || output.contains("/udp");
        if !is_scan {
            return Ok(None);
        }

        // 1. Extract target host or IP
        let mut target_ip = None;
        let mut target_hostname = None;
        for line in output.lines() {
            if line.contains("Nmap scan report for ") {
                let part = line.trim_start_matches("Nmap scan report for ").trim();
                if let Some(open) = part.find('(') {
                    if let Some(close) = part.find(')') {
                        if close > open {
                            target_hostname = Some(part[..open].trim().to_string());
                            target_ip = Some(part[open + 1..close].trim().to_string());
                        }
                    }
                }
                if target_ip.is_none() {
                    target_ip = Some(part.to_string());
                }
                break;
            }
        }

        // If not in output, inspect cmd line arguments for IPv4
        if target_ip.is_none() {
            for word in cmd.split_whitespace() {
                let clean = word.trim_matches(|c| c == '\'' || c == '"' || c == ',' || c == ';');
                let parts: Vec<&str> = clean.split('.').collect();
                if parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok()) {
                    target_ip = Some(clean.to_string());
                    break;
                }
            }
        }

        // 2. Extract open ports
        let mut ports: Vec<crate::db::models::PortInfo> = Vec::new();
        for line in output.lines() {
            let trimmed = line.trim();
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() >= 2 {
                let port_proto = parts[0];
                if let Some(slash_idx) = port_proto.find('/') {
                    let port_str = &port_proto[..slash_idx];
                    let proto = &port_proto[slash_idx + 1..];
                    if let Ok(port_num) = port_str.parse::<u16>() {
                        let state = parts[1];
                        if state == "open" {
                            let service = parts.get(2).unwrap_or(&"unknown").to_string();
                            let version = if parts.len() > 3 {
                                parts[3..].join(" ")
                            } else {
                                String::new()
                            };
                            ports.push(crate::db::models::PortInfo {
                                port: port_num,
                                proto: proto.to_string(),
                                service,
                                version,
                            });
                        }
                    }
                }
            }
        }

        // 3. Extract OS info if present
        let mut os_detected = None;
        for line in output.lines() {
            let lower = line.to_lowercase();
            if lower.contains("service info: os:") {
                if let Some(idx) = line.find("OS:") {
                    let after = &line[idx + 3..];
                    let os_str = after.split(';').next().unwrap_or(after).trim();
                    if !os_str.is_empty() {
                        os_detected = Some(os_str.to_string());
                        break;
                    }
                }
            } else if lower.contains("os details:") {
                let os_str = line.trim_start_matches("OS details:").trim();
                if !os_str.is_empty() {
                    os_detected = Some(os_str.to_string());
                    break;
                }
            } else if lower.contains("running:") {
                let os_str = line.trim_start_matches("Running:").trim();
                if !os_str.is_empty() {
                    os_detected = Some(os_str.to_string());
                    break;
                }
            }
        }

        if ports.is_empty() && os_detected.is_none() {
            return Ok(None);
        }

        // 4. Update or insert target
        let target_to_update = if let Some(host) = &target_hostname {
            if let Ok(Some(existing)) = Self::get_target_by_ip(conn, host) {
                Some(existing)
            } else if let Some(ip) = &target_ip {
                Self::get_target_by_ip(conn, ip).ok().flatten()
            } else {
                None
            }
        } else if let Some(ip) = &target_ip {
            Self::get_target_by_ip(conn, ip).ok().flatten()
        } else if let Some(fid) = fallback_target_id {
            Self::get_target(conn, fid).ok().flatten()
        } else {
            None
        };

        let target_to_update = match target_to_update {
            Some(t) => Some(t),
            None => {
                let host_to_add = target_hostname.as_deref().or(target_ip.as_deref());
                if let Some(h) = host_to_add {
                    let ports_json_str = if !ports.is_empty() {
                        Some(serde_json::to_string(&ports)?)
                    } else {
                        None
                    };
                    let id = Self::add_target(conn, h, ports_json_str.as_deref(), None)?;
                    Self::get_target(conn, id).ok().flatten()
                } else {
                    None
                }
            }
        };

        if let Some(target) = target_to_update {
            let mut merged_ports = if let Some(existing_json) = &target.ports_json {
                serde_json::from_str::<Vec<crate::db::models::PortInfo>>(existing_json).unwrap_or_default()
            } else {
                Vec::new()
            };

            for new_p in ports {
                if let Some(existing_p) = merged_ports.iter_mut().find(|p| p.port == new_p.port && p.proto == new_p.proto) {
                    if !new_p.service.is_empty() && new_p.service != "unknown" {
                        existing_p.service = new_p.service;
                    }
                    if !new_p.version.is_empty() {
                        existing_p.version = new_p.version;
                    }
                } else {
                    merged_ports.push(new_p);
                }
            }
            merged_ports.sort_by_key(|p| p.port);

            let ports_json = serde_json::to_string(&merged_ports)?;
            let mut meta: serde_json::Value = target.metadata_json
                .as_deref()
                .and_then(|m| serde_json::from_str(m).ok())
                .unwrap_or_else(|| serde_json::json!({}));

            if let Some(os) = os_detected {
                meta["os"] = serde_json::Value::String(os);
            }
            if let Some(hostname) = target_hostname {
                let mut hostnames_arr = meta.get("hostnames")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                if !hostnames_arr.iter().any(|v| v.as_str() == Some(&hostname)) {
                    hostnames_arr.push(serde_json::Value::String(hostname));
                }
                meta["hostnames"] = serde_json::Value::Array(hostnames_arr);
            }

            Self::update_target(conn, target.id, Some(&ports_json), Some(&meta.to_string()))?;
            Ok(Some(target.id))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_nmap_scan_output() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::init_database(&conn).unwrap();

        let cmd = "nmap -sS -sV -T4 10.129.90.124";
        let output = r#"
Starting Nmap 7.94 ( https://nmap.org ) at 2026-11-07 20:00 UTC
Nmap scan report for 10.129.90.124
Host is up (0.045s latency).
Not shown: 996 closed ports
PORT     STATE SERVICE       VERSION
135/tcp  open  msrpc         Microsoft Windows RPC
139/tcp  open  netbios-ssn   Microsoft Windows netbios-ssn
445/tcp  open  microsoft-ds?
5985/tcp open  http          Microsoft HTTPAPI httpd 2.0 (SSDP/UPnP)
Service Info: OS: Windows; CPE: cpe:/o:microsoft:windows

Service detection performed. Please report any incorrect results at https://nmap.org/submit/ .
Nmap done: 1 IP address (1 host up) scanned in 16.65 seconds
"#;

        let res = DatabaseQueries::parse_nmap_and_update_target(&conn, cmd, output, None).unwrap();
        assert!(res.is_some());
        let target_id = res.unwrap();

        let target = DatabaseQueries::get_target(&conn, target_id).unwrap().unwrap();
        assert_eq!(target.ip_host, "10.129.90.124");
        assert!(target.ports_json.is_some());

        let ports: Vec<crate::db::models::PortInfo> = serde_json::from_str(&target.ports_json.unwrap()).unwrap();
        assert_eq!(ports.len(), 4);
        assert_eq!(ports[0].port, 135);
        assert_eq!(ports[0].service, "msrpc");
        assert_eq!(ports[0].version, "Microsoft Windows RPC");
        assert_eq!(ports[3].port, 5985);
        assert_eq!(ports[3].service, "http");

        let meta: serde_json::Value = serde_json::from_str(&target.metadata_json.unwrap()).unwrap();
        assert_eq!(meta["os"], "Windows");
    }

    #[test]
    fn test_normalize_target_host() {
        assert_eq!(normalize_target_host("http://unclaimed.htb/"), "unclaimed.htb");
        assert_eq!(normalize_target_host("https://unclaimed.htb:80/ask"), "unclaimed.htb");
        assert_eq!(normalize_target_host("UNCLAIMED.HTB"), "unclaimed.htb");
        assert_eq!(normalize_target_host("10.129.90.124:8080"), "10.129.90.124");
    }

    #[test]
    fn test_deduplicate_targets_and_findings() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::schema::init_database(&conn).unwrap();

        // Simulate duplicates from multiple tools
        let id1 = DatabaseQueries::add_target(&conn, "unclaimed.htb", None, None).unwrap();
        conn.execute("INSERT INTO targets (ip_host) VALUES ('http://unclaimed.htb/')", []).unwrap();
        let id2 = conn.last_insert_rowid();

        DatabaseQueries::add_finding(&conn, id1, "SQLi in /ask", "SQL Injection", "critical", None, Some(90), None).unwrap();
        DatabaseQueries::add_finding(&conn, id2, "SQLi in /ask", "Duplicate finding", "critical", None, Some(95), None).unwrap();

        DatabaseQueries::deduplicate_targets_and_findings(&conn).unwrap();

        let targets = DatabaseQueries::get_all_targets(&conn).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].ip_host, "unclaimed.htb");

        let findings = DatabaseQueries::get_findings_for_target(&conn, targets[0].id).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].title, "SQLi in /ask");
    }
}