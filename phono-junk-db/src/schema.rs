//! Schema and migrations.
//!
//! Migration table tracks applied schema version. `migrate()` applies any
//! missing migrations in order. Copy of retro-junk-db's idiom —
//! eventually extracted to junk-libs once both products are ready.

use rusqlite::Connection;

use crate::DbError;

pub fn migrate(_conn: &Connection) -> Result<(), DbError> {
    // TODO: create schema_version table, apply migrations idempotently
    Ok(())
}
