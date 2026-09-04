//! Schema v7 tests: fresh-DB creation, idempotency, on-disk round-trip,
//! version-mismatch rejection, and foreign-key enforcement.

use std::collections::BTreeSet;

use phono_junk_db::{
    CURRENT_VERSION, SchemaError, create_schema, open_database, open_memory, reset_database,
};
use rusqlite::{Connection, params};

fn table_names(conn: &Connection) -> BTreeSet<String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .expect("query sqlite_master");
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("map rows");
    rows.map(|r| r.expect("row")).collect()
}

fn column_names(conn: &Connection, table: &str) -> BTreeSet<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .expect("prepare pragma");
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .expect("map rows");
    rows.map(|r| r.expect("row")).collect()
}

#[test]
fn open_memory_creates_all_tables() {
    let conn = open_memory().expect("open_memory");

    let tables = table_names(&conn);
    let expected = [
        "albums",
        "assets",
        "candidate_observations",
        "disagreements",
        "dbar_responses",
        "discs",
        "identification_attempts",
        "identification_candidates",
        "overrides",
        "provider_observations",
        "releases",
        "rip_files",
        "rip_file_provenance",
        "schema_version",
        "track_verifications",
        "tracks",
        "verification_runs",
    ];
    for name in expected {
        assert!(tables.contains(name), "missing table: {name}");
    }

    // MB-rhyming columns must be present — catches accidental drops in future
    // migrations. Each column maps directly to a MusicBrainz JSON field so
    // Sprint 11's aggregator can write it without translation.
    let album_cols = column_names(&conn, "albums");
    for col in ["primary_type", "secondary_types_json", "first_release_date"] {
        assert!(album_cols.contains(col), "albums missing column: {col}");
    }
    assert!(column_names(&conn, "releases").contains("status"));
    assert!(column_names(&conn, "discs").contains("format"));
    assert!(column_names(&conn, "discs").contains("stable_key"));
    assert!(column_names(&conn, "tracks").contains("recording_mbid"));
    assert!(column_names(&conn, "rip_files").contains("stable_key"));

    // schema_version row was written exactly once.
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let version: i32 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, CURRENT_VERSION);
}

#[test]
fn create_schema_is_idempotent() {
    let conn = open_memory().expect("open_memory");
    // open_memory already called create_schema once; call it again directly.
    create_schema(&conn).expect("second create_schema");
    create_schema(&conn).expect("third create_schema");

    // Row count is unchanged — repeat calls must not insert duplicate version
    // rows now that the schema is already at CURRENT_VERSION.
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn open_database_on_disk_roundtrip() {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    let path = tmp.path().to_path_buf();

    {
        let conn = open_database(&path).expect("open fresh");
        conn.execute(
            "INSERT INTO albums (title, primary_type) VALUES (?1, ?2)",
            params!["Kid A", "Album"],
        )
        .expect("insert");
    }

    let conn = open_database(&path).expect("reopen");
    let (title, primary_type): (String, String) = conn
        .query_row(
            "SELECT title, primary_type FROM albums WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("query");
    assert_eq!(title, "Kid A");
    assert_eq!(primary_type, "Album");

    // Reopen must not create a second schema_version row.
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn version_mismatch_rejects_future_db() {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    let path = tmp.path().to_path_buf();

    {
        let conn = open_database(&path).expect("open fresh");
        conn.execute(
            "INSERT INTO schema_version (version) VALUES (?1)",
            [CURRENT_VERSION + 100],
        )
        .expect("insert future version");
    }

    match open_database(&path) {
        Err(SchemaError::VersionMismatch { expected, found }) => {
            assert_eq!(expected, CURRENT_VERSION);
            assert_eq!(found, CURRENT_VERSION + 100);
        }
        other => panic!("expected VersionMismatch, got: {other:?}"),
    }
}

#[test]
fn v6_returns_typed_rebuild_required() {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    let path = tmp.path().to_path_buf();
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_version (version INTEGER NOT NULL);\
         INSERT INTO schema_version(version) VALUES (6);",
    )
    .unwrap();
    drop(conn);

    match open_database(&path) {
        Err(SchemaError::RebuildRequired { expected, found }) => {
            assert_eq!(expected, 7);
            assert_eq!(found, 6);
        }
        other => panic!("expected RebuildRequired, got: {other:?}"),
    }
}

#[test]
fn confirmed_reset_replaces_v6_with_v7() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("catalog.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE schema_version (version INTEGER NOT NULL);\
         INSERT INTO schema_version(version) VALUES (6);",
    )
    .unwrap();
    drop(conn);

    drop(reset_database(&path).expect("reset database"));
    let conn = open_database(&path).expect("open rebuilt database");
    let version: i32 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, CURRENT_VERSION);
    assert!(table_names(&conn).contains("verification_runs"));
}

#[test]
fn foreign_keys_enforced() {
    let conn = open_memory().expect("open_memory");
    // Attempt to insert a release pointing at a non-existent album. Requires
    // `PRAGMA foreign_keys=ON` to be active; this test fails loudly if the
    // pragma is ever dropped from open_memory.
    let err = conn
        .execute(
            "INSERT INTO releases (album_id, country) VALUES (?1, ?2)",
            params![999_999, "JP"],
        )
        .expect_err("expected FK violation");
    let msg = err.to_string();
    assert!(
        msg.contains("FOREIGN KEY") || msg.contains("foreign key"),
        "unexpected error: {msg}"
    );
}
