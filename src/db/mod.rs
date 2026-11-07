//! Database module for voidrecon
//! Handles SQLite schema initialization and all queries.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub mod models;
pub mod queries;
pub mod schema;

pub use models::*;
pub use queries::*;
pub use schema::*;

/// Database connection wrapper with initialized schema
pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path).context("Failed to open database")?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA busy_timeout = 5000;",
        )
        .context("Failed to set PRAGMA options")?;
        let db = Self { conn };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(SCHEMA).context("Failed to create schema")?;
        Ok(())
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS targets (
    id INTEGER PRIMARY KEY,
    ip_host TEXT NOT NULL,
    ports_json TEXT,
    metadata_json TEXT,
    notes TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS findings (
    id INTEGER PRIMARY KEY,
    target_id INTEGER NOT NULL REFERENCES targets(id),
    title TEXT NOT NULL,
    description TEXT,
    severity TEXT CHECK(severity IN ('info','low','medium','high','critical')),
    source_cmd TEXT,
    confidence INTEGER CHECK(confidence BETWEEN 0 AND 100) DEFAULT 100,
    evidence_path TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS commands (
    id INTEGER PRIMARY KEY,
    target_id INTEGER REFERENCES targets(id),
    raw_cmd TEXT NOT NULL,
    output TEXT,
    output_path TEXT,
    output_hash TEXT,
    exit_code INTEGER,
    duration_ms INTEGER,
    executed_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS chat_history (
    id INTEGER PRIMARY KEY,
    target_id INTEGER REFERENCES targets(id),
    role TEXT CHECK(role IN ('user','assistant','system')),
    content TEXT NOT NULL,
    timestamp DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_findings_target ON findings(target_id);
CREATE INDEX IF NOT EXISTS idx_findings_severity ON findings(severity);
CREATE INDEX IF NOT EXISTS idx_commands_target ON commands(target_id);
CREATE INDEX IF NOT EXISTS idx_commands_executed ON commands(executed_at);
CREATE INDEX IF NOT EXISTS idx_chat_target ON chat_history(target_id);
";