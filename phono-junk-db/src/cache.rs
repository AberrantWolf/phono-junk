//! Library-level rip-file cache — a thin policy layer over `rip_files` CRUD.
//!
//! Invalidation is per-file: a cached row is valid iff the current on-disk
//! `(mtime, size)` pair matches the stored pair. Folder fingerprinting (as in
//! retro-junk) is deliberately not used — audio rips are small in count and
//! large in size, so per-file checks are cheap and avoid a full rescan when
//! one file changes.
//!
//! Caller contract:
//! - On `scan`: [`lookup_cached`] by path. `Some` → skip identification.
//!   `None` → re-identify, then [`upsert_rip_file`].
//! - Stale (`disc_id` points at a now-deleted disc) is handled by the
//!   schema's `ON DELETE SET NULL` — callers see the row with `disc_id=None`
//!   and treat it as unidentified.

use std::path::Path;

use phono_junk_catalog::{Id, RipFile};
use rusqlite::Connection;

use crate::{DbError, crud};

/// Look up a cached rip for `path`. Returns `Some(file)` only if both `mtime`
/// and `size` match the stored values — mismatch or missing row both return
/// `None`. Callers choose CUE-vs-CHD lookup based on the path extension or
/// the source they're scanning.
pub fn lookup_cached(
    conn: &Connection,
    path: &Path,
    current_mtime: i64,
    current_size: u64,
) -> Result<Option<RipFile>, DbError> {
    let cached = find_by_path(conn, path)?;
    Ok(cached.filter(|f| {
        f.mtime == Some(current_mtime) && f.size == Some(current_size)
    }))
}

/// Insert or update a rip-file row keyed on `cue_path` (preferred) or
/// `chd_path`. If an existing row is found, its `id` is preserved and all
/// other fields are overwritten from `file`. Returns the final row id.
pub fn upsert_rip_file(conn: &Connection, file: &RipFile) -> Result<Id, DbError> {
    let lookup_path = file
        .cue_path
        .as_deref()
        .or(file.chd_path.as_deref());

    if let Some(path) = lookup_path
        && let Some(existing) = find_by_path(conn, path)?
    {
        let mut merged = file.clone();
        merged.id = existing.id;
        crud::update_rip_file(conn, &merged)?;
        return Ok(existing.id);
    }

    crud::insert_rip_file(conn, file)
}

fn find_by_path(conn: &Connection, path: &Path) -> Result<Option<RipFile>, DbError> {
    if let Some(file) = crud::find_rip_file_by_cue_path(conn, path)? {
        return Ok(Some(file));
    }
    crud::find_rip_file_by_chd_path(conn, path)
}
