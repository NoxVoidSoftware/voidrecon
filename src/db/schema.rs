//! Database schema definitions and migrations for voidrecon

/// Current schema version
pub const SCHEMA_VERSION: i32 = 1;

/// SQL for creating the initial schema
pub const INITIAL_SCHEMA: &str = "
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

CREATE TABLE IF NOT EXISTS important_facts (
    id INTEGER PRIMARY KEY,
    target_id INTEGER NOT NULL REFERENCES targets(id),
    fact TEXT NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_findings_target ON findings(target_id);
CREATE INDEX IF NOT EXISTS idx_findings_severity ON findings(severity);
CREATE INDEX IF NOT EXISTS idx_commands_target ON commands(target_id);
CREATE INDEX IF NOT EXISTS idx_commands_executed ON commands(executed_at);
CREATE INDEX IF NOT EXISTS idx_chat_target ON chat_history(target_id);
CREATE INDEX IF NOT EXISTS idx_facts_target ON important_facts(target_id);
";

/// Migration SQL for future schema versions
pub const MIGRATIONS: &[&str] = &[
    // v2: important facts
    "CREATE TABLE IF NOT EXISTS important_facts (
        id INTEGER PRIMARY KEY,
        target_id INTEGER NOT NULL REFERENCES targets(id),
        fact TEXT NOT NULL,
        created_at DATETIME DEFAULT CURRENT_TIMESTAMP
    );
    CREATE INDEX IF NOT EXISTS idx_facts_target ON important_facts(target_id);",
];

/// Initialize database with schema and run migrations
pub fn init_database(conn: &rusqlite::Connection) -> anyhow::Result<()> {
    // Create schema_version table if not exists
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            applied_at DATETIME DEFAULT CURRENT_TIMESTAMP
        );",
    )?;

    // Get current version
    let current_version: i32 = conn
        .query_row("SELECT COALESCE(MAX(version), 0) FROM schema_version", [], |row| row.get(0))
        .unwrap_or(0);

    // Apply initial schema if this is a new database
    if current_version == 0 {
        conn.execute_batch(INITIAL_SCHEMA)?;
        conn.execute("INSERT INTO schema_version (version) VALUES (?)", [SCHEMA_VERSION])?;
    }

    // Apply migrations
    for (i, migration) in MIGRATIONS.iter().enumerate() {
        let version = i as i32 + 2; // Start from version 2
        if version > current_version {
            conn.execute_batch(migration)?;
            conn.execute("INSERT INTO schema_version (version) VALUES (?)", [version])?;
        }
    }

    // Enable PRAGMAs
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA busy_timeout = 5000;",
    )?;

    Ok(())
}