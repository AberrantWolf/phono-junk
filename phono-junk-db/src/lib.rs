//! SQLite persistence.
//!
//! Migration-version pattern mirrors `retro-junk-db/src/schema.rs`. Library
//! cache uses per-file mtime+size invalidation rather than per-folder
//! fingerprints so incremental changes don't force a full rescan.

use thiserror::Error;

pub mod cache;
pub mod crud;
pub mod overrides;
pub mod schema;

pub use schema::{CURRENT_VERSION, SchemaError, create_schema, open_database, open_memory};

#[derive(Debug, Error)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("schema error: {0}")]
    Schema(#[from] SchemaError),
    #[error("migration error: {0}")]
    Migration(String),
}
