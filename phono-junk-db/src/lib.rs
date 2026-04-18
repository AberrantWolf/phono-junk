//! SQLite persistence.
//!
//! Migration-version pattern copied from retro-junk-db's schema.rs idiom.
//! Library cache uses per-file mtime+size invalidation rather than
//! per-folder fingerprints so incremental changes don't force a full rescan.

use thiserror::Error;

pub mod schema;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Migration error: {0}")]
    Migration(String),
}
