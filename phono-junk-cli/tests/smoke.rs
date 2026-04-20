//! CLI smoke tests.
//!
//! These exercise the subcommand boundary: clap argument parsing, the
//! env resolution chain, DB lifecycle, and happy-path output shape. The
//! deeper logic (TOC parsing, CRC, FLAC encode, provider HTTP) is covered
//! by the individual crate tests and reached transitively — no need to
//! duplicate here.
//!
//! The network-backed `identify` and `verify` subcommands cannot be
//! smoke-tested here because the providers hardcode production URLs
//! today; upgrading [`phono_junk_identify::HttpClient`] with per-host
//! URL rewriting is a future item (see TODO.md).

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use phono_junk_catalog::{Album, Disc, Id, Release, RipFile, Track};
use phono_junk_core::{IdentificationConfidence, Toc};
use phono_junk_db::{crud, open_database};
use predicates::prelude::*;
use tempfile::TempDir;

fn toc_fixtures_dir() -> PathBuf {
    // Reach across to the sibling crate's fixtures rather than copying them.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("phono-junk-toc")
        .join("tests")
        .join("fixtures")
}

fn ensure_sparse(path: &Path, len: u64) {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() == len {
            return;
        }
    }
    let f: File = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .unwrap();
    f.set_len(len).unwrap();
}

fn stage_arver_fixture(into: &Path) -> PathBuf {
    let fixtures = toc_fixtures_dir();
    ensure_sparse(&fixtures.join("arver_3track.bin"), 335_953 * 2352);
    let cue_src = fixtures.join("arver_3track.cue");
    let bin_src = fixtures.join("arver_3track.bin");
    let cue_dst = into.join("arver_3track.cue");
    let bin_dst = into.join("arver_3track.bin");
    std::fs::copy(&cue_src, &cue_dst).unwrap();
    // Re-create the BIN as sparse in the target tempdir (fs::copy would
    // materialise the full 790 MB).
    ensure_sparse(&bin_dst, 335_953 * 2352);
    let _ = bin_src; // keep silently present for symmetry
    cue_dst
}

fn phono() -> Command {
    Command::cargo_bin("phono-junk").unwrap()
}

#[test]
fn help_lists_every_subcommand() {
    phono()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("scan").and(predicate::str::contains("identify")))
        .stdout(predicate::str::contains("verify").and(predicate::str::contains("export")))
        .stdout(predicate::str::contains("list"));
}

#[test]
fn scan_no_identify_populates_rip_files_and_list_shows_them() {
    let tmp = TempDir::new().unwrap();
    let rips_dir = tmp.path().join("rips");
    std::fs::create_dir_all(&rips_dir).unwrap();
    stage_arver_fixture(&rips_dir);

    let db_path = tmp.path().join("library.db");

    phono()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "scan",
            rips_dir.to_str().unwrap(),
            "--no-identify",
        ])
        .assert()
        .success();

    let list = phono()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "--format",
            "json",
            "list",
            "--unidentified",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&list).unwrap();
    let rows = parsed
        .get("Unidentified")
        .and_then(|v| v.as_array())
        .expect("expected Unidentified variant with rows");
    assert_eq!(rows.len(), 1, "expected exactly one unidentified rip: {parsed}");
    let path = rows[0]["cue_path"].as_str().unwrap();
    assert!(
        path.ends_with("arver_3track.cue"),
        "unexpected cue_path: {path}"
    );
}

#[test]
fn list_filters_albums_by_artist() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("library.db");
    seed_two_albums(&db_path);

    let out = phono()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "--format",
            "json",
            "list",
            "--artist",
            "weez",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let rows = parsed
        .get("Albums")
        .and_then(|v| v.as_array())
        .expect("expected Albums variant with rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["artist"].as_str(), Some("Weezer"));
    assert_eq!(rows[0]["title"].as_str(), Some("Pinkerton"));
}

#[test]
fn list_year_range_filter_excludes_outside() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("library.db");
    seed_two_albums(&db_path);

    let out = phono()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "--format",
            "json",
            "list",
            "--year",
            "1990-1999",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let rows = parsed["Albums"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["year"].as_i64(), Some(1996));
}

#[test]
fn export_dry_run_prints_planned_paths() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("library.db");
    let (_album_id, _release_id, disc_id) = seed_one_disc(&db_path);
    let out_dir = tmp.path().join("out");
    std::fs::create_dir_all(&out_dir).unwrap();

    let out = phono()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "export",
            "--disc-ids",
            &disc_id.to_string(),
            "--out",
            out_dir.to_str().unwrap(),
            "--dry-run",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("would write"), "dry-run marker missing: {s}");
    assert!(s.contains("Pinkerton"), "album dir missing: {s}");
    assert!(
        s.contains("01 - Tired of Sex.flac"),
        "track layout missing: {s}"
    );
}

// ---------------------------------------------------------------------------
// Seed helpers
// ---------------------------------------------------------------------------

fn seed_two_albums(db_path: &Path) {
    let conn = open_database(db_path).unwrap();
    insert_album_with_release(
        &conn,
        "Pinkerton",
        Some("Weezer"),
        Some(1996),
        Some("US"),
        Some("DGC"),
    );
    insert_album_with_release(
        &conn,
        "Parachutes",
        Some("Coldplay"),
        Some(2000),
        Some("GB"),
        Some("Parlophone"),
    );
}

fn seed_one_disc(db_path: &Path) -> (Id, Id, Id) {
    let conn = open_database(db_path).unwrap();
    let album_id = crud::insert_album(
        &conn,
        &Album {
            id: 0,
            title: "Pinkerton".into(),
            sort_title: None,
            artist_credit: Some("Weezer".into()),
            year: Some(1996),
            mbid: Some("11111111-1111-1111-1111-111111111111".into()),
            primary_type: None,
            secondary_types: Vec::new(),
            first_release_date: None,
        },
    )
    .unwrap();
    let release_id = crud::insert_release(
        &conn,
        &Release {
            id: 0,
            album_id,
            country: Some("US".into()),
            date: Some("1996-09-24".into()),
            label: Some("DGC".into()),
            catalog_number: None,
            barcode: None,
            mbid: Some("22222222-2222-2222-2222-222222222222".into()),
            status: None,
            language: None,
            script: None,
        },
    )
    .unwrap();
    let toc = Toc {
        first_track: 1,
        last_track: 1,
        leadout_sector: 300_000,
        track_offsets: vec![150],
    };
    let disc_id = crud::insert_disc(
        &conn,
        &Disc {
            id: 0,
            release_id,
            disc_number: 1,
            format: "CD".into(),
            toc: Some(toc),
            mb_discid: None,
            cddb_id: None,
            ar_discid1: None,
            ar_discid2: None,
            dbar_raw: None,
        },
    )
    .unwrap();
    crud::insert_track(
        &conn,
        &Track {
            id: 0,
            disc_id,
            position: 1,
            title: Some("Tired of Sex".into()),
            artist_credit: Some("Weezer".into()),
            length_frames: Some(215 * 75),
            isrc: None,
            mbid: None,
            recording_mbid: None,
        },
    )
    .unwrap();
    let rip_id = crud::insert_rip_file(
        &conn,
        &RipFile {
            id: 0,
            disc_id: Some(disc_id),
            cue_path: Some(PathBuf::from("/tmp/pink.cue")),
            chd_path: None,
            bin_paths: vec![PathBuf::from("/tmp/pink.bin")],
            mtime: Some(0),
            size: Some(0),
            identification_confidence: IdentificationConfidence::Certain,
            identification_source: None,
            accuraterip_status: None,
            last_verified_at: None,
            last_identify_errors: None,
            last_identify_at: None,
        },
    )
    .unwrap();
    let _ = rip_id;
    (album_id, release_id, disc_id)
}

fn insert_album_with_release(
    conn: &rusqlite::Connection,
    title: &str,
    artist: Option<&str>,
    year: Option<u16>,
    country: Option<&str>,
    label: Option<&str>,
) -> Id {
    let album_id = crud::insert_album(
        conn,
        &Album {
            id: 0,
            title: title.into(),
            sort_title: None,
            artist_credit: artist.map(Into::into),
            year,
            mbid: None,
            primary_type: None,
            secondary_types: Vec::new(),
            first_release_date: None,
        },
    )
    .unwrap();
    crud::insert_release(
        conn,
        &Release {
            id: 0,
            album_id,
            country: country.map(Into::into),
            date: None,
            label: label.map(Into::into),
            catalog_number: None,
            barcode: None,
            mbid: None,
            status: None,
            language: None,
            script: None,
        },
    )
    .unwrap();
    album_id
}
