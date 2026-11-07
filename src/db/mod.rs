//! Database module for voidrecon
//! Handles SQLite schema initialization and all queries.

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

pub mod models;
pub mod queries;
pub mod schema;

pub use models::*;
pub use queries::*;

/// Database connection wrapper with initialized schema
pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path).context("Failed to open database")?;
        schema::init_database(&conn)?;
        Ok(Self { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}