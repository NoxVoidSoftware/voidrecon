//! Database queries for voidrecon

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use uuid::Uuid;

use crate::db::models::{Command, Finding, Target};

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

impl DatabaseQueries {
    // Target operations
    pub fn add_target(conn: &Connection, ip_host: &str, ports_json: Option<&str>, notes: Option<&str>) -> Result<i64> {
        conn.execute(
            "INSERT INTO targets (ip_host, ports_json, notes) VALUES (?1, ?2, ?3)",
            params![ip_host, ports_json, notes],
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
        conn.query_row(
            "SELECT id, ip_host, ports_json, metadata_json, notes, created_at FROM targets WHERE ip_host = ?1",
            params![ip_host],
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
        let description = truncate(description, 200);
        let confidence = confidence.unwrap_or(100).clamp(0, 100);
        conn.execute(
            "INSERT INTO findings (target_id, title, description, severity, source_cmd, confidence, evidence_path) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![target_id, title, description, severity, source_cmd, confidence, evidence_path],
        )
        .context("Failed to insert finding")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn add_note(conn: &Connection, target_id: i64, text: &str) -> Result<i64> {
        Self::add_finding(conn, target_id, "note", text, "info", None, Some(100), None)
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
}