//! Schema creation and version guard.
//!
//! Schema v7 is the first durable baseline. Older alpha catalogs are rejected
//! with a typed rebuild error; every schema change after v7 must be expressed
//! as a forward migration.

use std::path::Path;

use rusqlite::Connection;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SchemaError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "catalog schema v{found} predates the durable v{expected} baseline; rebuild required (CLI: phono-junk reset --yes)"
    )]
    RebuildRequired { expected: i32, found: i32 },
    #[error("catalog schema v{found} is newer than this binary (v{expected})")]
    VersionMismatch { expected: i32, found: i32 },
    #[error("missing forward migration from catalog schema v{from}")]
    MissingMigration { from: i32 },
    #[error("invalid non-contiguous schema migration v{from} -> v{to}")]
    InvalidMigration { from: i32, to: i32 },
}

/// Current schema version. Version 7 is the migration-supported baseline;
/// every later bump must have a forward migration.
pub const CURRENT_VERSION: i32 = 7;

/// Open (or create) a catalog database at `path`. Sets `journal_mode=WAL`
/// and `foreign_keys=ON`. Pre-v7 alpha catalogs return `RebuildRequired`;
/// catalogs newer than this binary return `VersionMismatch`.
pub fn open_database(path: &Path) -> Result<Connection, SchemaError> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

    let version = get_schema_version(&conn)?;
    if version == 0 {
        create_schema(&conn)?;
    } else if version < 7 {
        return Err(SchemaError::RebuildRequired {
            expected: CURRENT_VERSION,
            found: version,
        });
    } else if version < CURRENT_VERSION {
        apply_migrations(&conn, version, CURRENT_VERSION, MIGRATIONS)?;
    } else if version > CURRENT_VERSION {
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

/// Remove a catalog and its WAL sidecars, then create an empty v7 catalog.
/// Callers must obtain explicit user confirmation before invoking this.
pub fn reset_database(path: &Path) -> Result<Connection, SchemaError> {
    for suffix in ["", "-wal", "-shm"] {
        let candidate = if suffix.is_empty() {
            path.to_path_buf()
        } else {
            let mut value = path.as_os_str().to_owned();
            value.push(suffix);
            value.into()
        };
        match std::fs::remove_file(&candidate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    open_database(path)
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

struct Migration {
    from: i32,
    to: i32,
    sql: &'static str,
}

// Append one contiguous migration per version bump. v7 itself is the fixture
// baseline and is never represented as a migration.
const MIGRATIONS: &[Migration] = &[];

fn apply_migrations(
    conn: &Connection,
    from: i32,
    target: i32,
    migrations: &[Migration],
) -> Result<(), SchemaError> {
    let transaction = conn.unchecked_transaction()?;
    let mut current = from;
    while current < target {
        let migration = migrations
            .iter()
            .find(|migration| migration.from == current)
            .ok_or(SchemaError::MissingMigration { from: current })?;
        if migration.to != current + 1 {
            return Err(SchemaError::InvalidMigration {
                from: migration.from,
                to: migration.to,
            });
        }
        transaction.execute_batch(migration.sql)?;
        set_schema_version(&transaction, migration.to)?;
        current = migration.to;
    }
    transaction.commit()?;
    Ok(())
}

// Schema v7. MusicBrainz-shaped column names map directly at the single
// projection boundary; provider observations remain provider-neutral JSON.
const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_version (
    version      INTEGER NOT NULL,
    applied_at   TEXT NOT NULL DEFAULT (datetime('now'))
);

-- MusicBrainz "release group" equivalent.
CREATE TABLE IF NOT EXISTS albums (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    stable_key              TEXT UNIQUE,
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
    stable_key      TEXT UNIQUE,
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
    stable_key      TEXT UNIQUE,
    release_id      INTEGER NOT NULL REFERENCES releases(id) ON DELETE CASCADE,
    disc_number     INTEGER NOT NULL,
    format          TEXT NOT NULL DEFAULT 'CD',
    toc_json        TEXT,
    mb_discid       TEXT,
    cddb_id         TEXT,
    ar_discid1      TEXT,
    ar_discid2      TEXT,
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
    stable_key      TEXT UNIQUE,
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
    stable_key                  TEXT UNIQUE,
    disc_id                     INTEGER REFERENCES discs(id) ON DELETE SET NULL,
    cue_path                    TEXT,
    chd_path                    TEXT,
    bin_paths_json              TEXT NOT NULL DEFAULT '[]',
    mtime                       INTEGER,
    size                        INTEGER,
    identification_confidence   TEXT NOT NULL,
    identification_source       TEXT,
    last_identify_errors        TEXT,
    last_identify_at            TEXT,
    -- Lifecycle state is separate from confidence. One of
    -- unscanned / queued / working / identified / unidentified / failed.
    identification_state        TEXT NOT NULL DEFAULT 'unscanned',
    last_state_change_at        TEXT
);
CREATE INDEX IF NOT EXISTS idx_rip_files_disc  ON rip_files(disc_id);
CREATE INDEX IF NOT EXISTS idx_rip_files_cue   ON rip_files(cue_path);
CREATE INDEX IF NOT EXISTS idx_rip_files_chd   ON rip_files(chd_path);
CREATE INDEX IF NOT EXISTS idx_rip_files_state ON rip_files(identification_state);

-- Ripper provenance as a 1:1 side-table: keeps the rip_files row lean for the
-- common pre-Redumper case, and leaves room for provenance to grow
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
    stable_key      TEXT UNIQUE,
    release_id      INTEGER NOT NULL REFERENCES releases(id) ON DELETE CASCADE,
    provider        TEXT NOT NULL DEFAULT 'unknown',
    asset_type      TEXT NOT NULL,
    group_id        INTEGER,
    sequence        INTEGER NOT NULL DEFAULT 0,
    source_url      TEXT,
    file_path       TEXT,
    width           INTEGER,
    height          INTEGER,
    confidence      TEXT,
    mime_type       TEXT,
    acquired_at     TEXT
);
CREATE INDEX IF NOT EXISTS idx_assets_release ON assets(release_id);
CREATE INDEX IF NOT EXISTS idx_assets_group   ON assets(release_id, group_id, sequence);

CREATE TABLE IF NOT EXISTS disagreements (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type     TEXT NOT NULL,
    entity_id       INTEGER NOT NULL,
    entity_key      TEXT NOT NULL,
    field           TEXT NOT NULL,
    source_a        TEXT NOT NULL,
    value_a         TEXT NOT NULL,
    source_b        TEXT NOT NULL,
    value_b         TEXT NOT NULL,
    resolved        INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_disagreements_entity     ON disagreements(entity_type, entity_id);
CREATE INDEX IF NOT EXISTS idx_disagreements_entity_key ON disagreements(entity_key);
CREATE INDEX IF NOT EXISTS idx_disagreements_unresolved ON disagreements(resolved) WHERE resolved = 0;

-- Library folders tracked for auto-rescan. "Adding a folder" via the GUI
-- registers it here so every future DB open can re-walk the tree — new rips
-- appear in the album list without requiring the user to click through a
-- dialog every time. `path` is the absolute on-disk path (UTF-8 only;
-- non-UTF-8 paths are rejected at insert time).
CREATE TABLE IF NOT EXISTS library_folders (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    path            TEXT NOT NULL UNIQUE,
    added_at        TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS overrides (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type     TEXT NOT NULL,
    entity_id       INTEGER NOT NULL,
    entity_key      TEXT NOT NULL,
    sub_path        TEXT,
    field           TEXT NOT NULL,
    override_value  TEXT NOT NULL,
    reason          TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_overrides_entity ON overrides(entity_type, entity_id);
CREATE INDEX IF NOT EXISTS idx_overrides_entity_key ON overrides(entity_key);

-- Provider evidence is append-only. Projection updates happen only after a
-- deterministic resolution has been recorded.
CREATE TABLE IF NOT EXISTS identification_attempts (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    rip_file_id         INTEGER REFERENCES rip_files(id) ON DELETE SET NULL,
    disc_stable_key     TEXT NOT NULL,
    started_at          TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at         TEXT,
    status              TEXT NOT NULL DEFAULT 'working',
    selected_candidate  TEXT,
    confidence          TEXT,
    ambiguity_json      TEXT
);

CREATE TABLE IF NOT EXISTS provider_observations (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    attempt_id          INTEGER NOT NULL REFERENCES identification_attempts(id) ON DELETE CASCADE,
    provider            TEXT NOT NULL,
    input_kind          TEXT NOT NULL,
    input_value         TEXT NOT NULL,
    stage               INTEGER NOT NULL,
    observed_at         TEXT NOT NULL DEFAULT (datetime('now')),
    raw_response_json   TEXT,
    error_text          TEXT,
    UNIQUE(attempt_id, provider, input_kind, input_value)
);

CREATE TABLE IF NOT EXISTS identification_candidates (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    attempt_id          INTEGER NOT NULL REFERENCES identification_attempts(id) ON DELETE CASCADE,
    candidate_key       TEXT NOT NULL,
    candidate_json      TEXT NOT NULL,
    score_json          TEXT NOT NULL,
    selected            INTEGER NOT NULL DEFAULT 0,
    UNIQUE(attempt_id, candidate_key)
);

CREATE TABLE IF NOT EXISTS candidate_observations (
    candidate_id        INTEGER NOT NULL REFERENCES identification_candidates(id) ON DELETE CASCADE,
    observation_id      INTEGER NOT NULL REFERENCES provider_observations(id) ON DELETE CASCADE,
    PRIMARY KEY(candidate_id, observation_id)
);

-- A fetched dBAR body is immutable evidence identified by its content hash.
CREATE TABLE IF NOT EXISTS dbar_responses (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    disc_stable_key     TEXT NOT NULL,
    body_hash           TEXT NOT NULL,
    body                BLOB NOT NULL,
    acquired_at         TEXT NOT NULL,
    UNIQUE(disc_stable_key, body_hash)
);

CREATE TABLE IF NOT EXISTS verification_runs (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    rip_file_id         INTEGER NOT NULL REFERENCES rip_files(id) ON DELETE CASCADE,
    dbar_response_id    INTEGER REFERENCES dbar_responses(id) ON DELETE SET NULL,
    started_at          TEXT NOT NULL DEFAULT (datetime('now')),
    finished_at         TEXT,
    max_sample_shift    INTEGER NOT NULL,
    chosen_sample_shift INTEGER,
    ripper_read_offset  INTEGER,
    status              TEXT NOT NULL,
    ambiguous_offsets_json TEXT
);

CREATE TABLE IF NOT EXISTS track_verifications (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id              INTEGER NOT NULL REFERENCES verification_runs(id) ON DELETE CASCADE,
    track_position      INTEGER NOT NULL,
    computed_v1         INTEGER NOT NULL,
    computed_v2         INTEGER NOT NULL,
    matched_checksum    INTEGER,
    checksum_version    TEXT,
    sample_shift        INTEGER,
    confidence          INTEGER,
    response_index      INTEGER,
    frame_450_support   INTEGER NOT NULL DEFAULT 0,
    status              TEXT NOT NULL,
    UNIQUE(run_id, track_position)
);
"#;

#[cfg(test)]
mod migration_tests {
    use super::*;

    #[test]
    fn v7_fixture_runs_a_transactional_sample_forward_migration() {
        let conn = open_memory().unwrap();
        let sample = [Migration {
            from: 7,
            to: 8,
            sql: "CREATE TABLE migration_probe (id INTEGER PRIMARY KEY);",
        }];
        apply_migrations(&conn, 7, 8, &sample).unwrap();
        assert_eq!(get_schema_version(&conn).unwrap(), 8);
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='migration_probe')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists);
    }
}
