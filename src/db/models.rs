//! Database models for voidrecon

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub id: i64,
    pub ip_host: String,
    pub ports_json: Option<String>,
    pub metadata_json: Option<String>,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: i64,
    pub target_id: i64,
    pub title: String,
    pub description: String,
    pub severity: String,
    pub source_cmd: Option<String>,
    pub confidence: i32,
    pub evidence_path: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub id: i64,
    pub target_id: Option<i64>,
    pub raw_cmd: String,
    pub output: Option<String>,
    pub output_path: Option<String>,
    pub output_hash: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
    pub executed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: i64,
    pub target_id: Option<i64>,
    pub role: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortInfo {
    pub port: u16,
    pub proto: String,
    pub service: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetMetadata {
    pub os: Option<String>,
    pub hostnames: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub shell_user: Option<String>,
    pub privesc_done: Option<bool>,
}

impl Default for TargetMetadata {
    fn default() -> Self {
        Self {
            os: None,
            hostnames: None,
            tags: None,
            shell_user: None,
            privesc_done: Some(false),
        }
    }
}