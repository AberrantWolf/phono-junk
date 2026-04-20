//! Schema creation and version guard.
//!
//! Pre-release policy: the DB is treated as ephemeral. A `schema_version`
//! row is written so a stale DB is rejected with a clear error rather than
//! silently corrupting data, but there is no stepwise `migrate()` — if the
//! version doesn't match, the caller deletes the file and re-scans. Real
//! migration logic lands once a released binary has user state in the
//! wild; until then, bump `CURRENT_VERSION` freely.

use std::path::Path;

use rusqlite::Connection;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SchemaError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(
        "schema version mismatch: binary expects v{expected}, database is at v{found}. \
         Pre-release builds have no migration path — delete the database file and re-scan."
    )]
    VersionMismatch { expected: i32, found: i32 },
}

/// Current schema version. Bump freely during development; there is no
/// migration path yet, so bumping means existing dev DBs must be deleted.
pub const CURRENT_VERSION: i32 = 6;

/// Open (or create) a catalog database at `path`. Sets `journal_mode=WAL`
/// and `foreign_keys=ON`. Returns `VersionMismatch` if the DB was created
/// by a different schema version in either direction.
pub fn open_database(path: &Path) -> Result<Connection, SchemaError> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

    let version = get_schema_version(&conn)?;
    if version == 0 {
        create_schema(&conn)?;
    } else if version != CURRENT_VERSION {
        return Err(SchemaError::VersionMismatch {
            expected: CURRENT_VERSION,
            found: version,
        });
    }
    Ok(conn)
}

/// Open an in-memory database with the full current schema. Convenience for
/// tests; WAL is pointless on `:memory:` so only `foreign_keys=ON` is set.
pub fn open_memory() -> Result<Connection, SchemaError> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    create_schema(&conn)?;
    Ok(conn)
}

/// Create every table and index. Idempotent (all DDL uses `IF NOT EXISTS`).
/// Records `CURRENT_VERSION` in `schema_version` only once; repeat calls
/// leave the row count unchanged.
pub fn create_schema(conn: &Connection) -> Result<(), SchemaError> {
    conn.execute_batch(SCHEMA_SQL)?;
    let current = get_schema_version(conn)?;
    if current < CURRENT_VERSION {
        set_schema_version(conn, CURRENT_VERSION)?;
    }
    Ok(())
}

fn get_schema_version(conn: &Connection) -> Result<i32, SchemaError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_version')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(0);
    }
    let version: i32 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |row| row.get(0),
    )?;
    Ok(version)
}

fn set_schema_version(conn: &Connection, version: i32) -> Result<(), SchemaError> {
    conn.execute(
        "INSERT INTO schema_version (version) VALUES (?1)",
        [version],
    )?;
    Ok(())
}

// Schema v1. Column names that use MusicBrainz vocabulary (`primary_type`,
// `status`, `format`, `recording_mbid`, `first_release_date`) map 1:1 to
// MB JSON response fields so Sprint 11's aggregator can write rows without
// translation. Table names (`albums`, `discs`) match the Rust catalog types;
// rename to `release_groups`/`mediums` when the Rust types are renamed.
const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_version (
    version      INTEGER NOT NULL,
    applied_at   TEXT NOT NULL DEFAULT (datetime('now'))
);

-- MusicBrainz "release group" equivalent.
CREATE TABLE IF NOT EXISTS albums (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    title                   TEXT NOT NULL,
    sort_title              TEXT,
    artist_credit           TEXT,
    year                    INTEGER,
    mbid                    TEXT,
    primary_type            TEXT,
    secondary_types_json    TEXT,
    first_release_date      TEXT
);
CREATE INDEX IF NOT EXISTS idx_albums_mbid ON albums(mbid);

-- `language` and `script` mirror MB `release.text-representation.{language,script}`:
-- ISO 639-3 language code (e.g. `jpn`, `kor`, `zho`, `eng`) and ISO 15924
-- script code (e.g. `Jpan`, `Hans`, `Hant`, `Hang`, `Latn`). Drives
-- region-aware CJK font selection in the GUI.
CREATE TABLE IF NOT EXISTS releases (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    album_id        INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    country         TEXT,
    date            TEXT,
    label           TEXT,
    catalog_number  TEXT,
    barcode         TEXT,
    mbid            TEXT,
    status          TEXT,
    language        TEXT,
    script          TEXT
);
CREATE INDEX IF NOT EXISTS idx_releases_album   ON releases(album_id);
CREATE INDEX IF NOT EXISTS idx_releases_mbid    ON releases(mbid);
CREATE INDEX IF NOT EXISTS idx_releases_barcode ON releases(barcode);

CREATE TABLE IF NOT EXISTS discs (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    release_id      INTEGER NOT NULL REFERENCES releases(id) ON DELETE CASCADE,
    disc_number     INTEGER NOT NULL,
    format          TEXT NOT NULL DEFAULT 'CD',
    toc_json        TEXT,
    mb_discid       TEXT,
    cddb_id         TEXT,
    ar_discid1      TEXT,
    ar_discid2      TEXT,
    dbar_raw        BLOB,
    -- Media Catalog Number from the disc's subchannel Q data
    -- (a physical-disc fact; releases.barcode is the metadata-DB fact).
    mcn             TEXT
);
CREATE INDEX IF NOT EXISTS idx_discs_release   ON discs(release_id);
CREATE INDEX IF NOT EXISTS idx_discs_mb_discid ON discs(mb_discid);
-- Partial unique index: unidentified discs (NULL ar_discid1) are allowed to
-- coexist; identified discs collide on their AccurateRip triple.
CREATE UNIQUE INDEX IF NOT EXISTS idx_discs_ar_triple
    ON discs(ar_discid1, ar_discid2, cddb_id)
    WHERE ar_discid1 IS NOT NULL;

CREATE TABLE IF NOT EXISTS tracks (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    disc_id         INTEGER NOT NULL REFERENCES discs(id) ON DELETE CASCADE,
    position        INTEGER NOT NULL,
    title           TEXT,
    artist_credit   TEXT,
    length_frames   INTEGER,
    isrc            TEXT,
    mbid            TEXT,
    recording_mbid  TEXT,
    UNIQUE(disc_id, position)
);
CREATE INDEX IF NOT EXISTS idx_tracks_disc           ON tracks(disc_id);
CREATE INDEX IF NOT EXISTS idx_tracks_mbid           ON tracks(mbid);
CREATE INDEX IF NOT EXISTS idx_tracks_recording_mbid ON tracks(recording_mbid);

CREATE TABLE IF NOT EXISTS rip_files (
    id                          INTEGER PRIMARY KEY AUTOINCREMENT,
    disc_id                     INTEGER REFERENCES discs(id) ON DELETE SET NULL,
    cue_path                    TEXT,
    chd_path                    TEXT,
    bin_paths_json              TEXT NOT NULL DEFAULT '[]',
    mtime                       INTEGER,
    size                        INTEGER,
    identification_confidence   TEXT NOT NULL,
    identification_source       TEXT,
    accuraterip_status          TEXT,
    last_verified_at            TEXT,
    last_identify_errors        TEXT,
    last_identify_at            TEXT,
    -- Sprint 26: lifecycle state separate from confidence. One of
    -- unscanned / queued / working / identified / unidentified / failed.
    identification_state        TEXT NOT NULL DEFAULT 'unscanned',
    last_state_change_at        TEXT
);
CREATE INDEX IF NOT EXISTS idx_rip_files_disc  ON rip_files(disc_id);
CREATE INDEX IF NOT EXISTS idx_rip_files_cue   ON rip_files(cue_path);
CREATE INDEX IF NOT EXISTS idx_rip_files_chd   ON rip_files(chd_path);
CREATE INDEX IF NOT EXISTS idx_rip_files_state ON rip_files(identification_state);

-- Ripper provenance as a 1:1 side-table: keeps the rip_files row lean for the
-- common "pre-redumper rip" case, and leaves room for provenance to grow
-- (C2 error counts, secure-mode details, per-track stats) without another
-- ALTER. `ripper` is Ripper::as_str() (e.g. 'redumper', 'eac', 'unknown').
CREATE TABLE IF NOT EXISTS rip_file_provenance (
    rip_file_id     INTEGER PRIMARY KEY REFERENCES rip_files(id) ON DELETE CASCADE,
    ripper          TEXT NOT NULL,
    version         TEXT,
    drive_json      TEXT,
    read_offset     INTEGER,
    log_path        TEXT NOT NULL,
    rip_date        TEXT
);
CREATE INDEX IF NOT EXISTS idx_rip_file_provenance_ripper ON rip_file_provenance(ripper);

CREATE TABLE IF NOT EXISTS assets (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    release_id      INTEGER NOT NULL REFERENCES releases(id) ON DELETE CASCADE,
    asset_type      TEXT NOT NULL,
    group_id        INTEGER,
    sequence        INTEGER NOT NULL DEFAULT 0,
    source_url      TEXT,
    file_path       TEXT,
    scraped_at      TEXT
);
CREATE INDEX IF NOT EXISTS idx_assets_release ON assets(release_id);
CREATE INDEX IF NOT EXISTS idx_assets_group   ON assets(release_id, group_id, sequence);

CREATE TABLE IF NOT EXISTS disagreements (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type     TEXT NOT NULL,
    entity_id       INTEGER NOT NULL,
    field           TEXT NOT NULL,
    source_a        TEXT NOT NULL,
    value_a         TEXT NOT NULL,
    source_b        TEXT NOT NULL,
    value_b         TEXT NOT NULL,
    resolved        INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_disagreements_entity     ON disagreements(entity_type, entity_id);
CREATE INDEX IF NOT EXISTS idx_disagreements_unresolved ON disagreements(resolved) WHERE resolved = 0;

-- Library folders tracked for auto-rescan. "Adding a folder" via the GUI
-- registers it here so every future DB open re-walks the tree — new rips
-- appear in the album list without requiring the user to click through a
-- dialog every time. `path` is the absolute on-disk path (UTF-8 only;
-- non-UTF-8 paths are rejected at insert time). Sprint 27.
CREATE TABLE IF NOT EXISTS library_folders (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    path            TEXT NOT NULL UNIQUE,
    added_at        TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS overrides (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type     TEXT NOT NULL,
    entity_id       INTEGER NOT NULL,
    sub_path        TEXT,
    field           TEXT NOT NULL,
    override_value  TEXT NOT NULL,
    reason          TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_overrides_entity ON overrides(entity_type, entity_id);
"#;
