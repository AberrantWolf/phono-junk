use std::path::PathBuf;

use phono_junk_catalog::RipFile;
use phono_junk_core::{IdentificationConfidence, IdentificationSource};
use phono_junk_db::{cache, crud, open_memory};

fn rip(mtime: i64, size: u64) -> RipFile {
    RipFile {
        id: 0,
        disc_id: None,
        cue_path: Some(PathBuf::from("/rips/album.cue")),
        chd_path: None,
        bin_paths: vec![PathBuf::from("/rips/album.bin")],
        mtime: Some(mtime),
        size: Some(size),
        identification_confidence: IdentificationConfidence::Unidentified,
        identification_source: None,
        accuraterip_status: None,
        last_verified_at: None,
    }
}

#[test]
fn lookup_miss_on_unknown_path() {
    let conn = open_memory().unwrap();
    let got = cache::lookup_cached(&conn, &PathBuf::from("/rips/other.cue"), 1, 1).unwrap();
    assert!(got.is_none());
}

#[test]
fn lookup_hits_when_mtime_and_size_match() {
    let conn = open_memory().unwrap();
    let id = cache::upsert_rip_file(&conn, &rip(100, 500)).unwrap();
    let got = cache::lookup_cached(&conn, &PathBuf::from("/rips/album.cue"), 100, 500)
        .unwrap()
        .unwrap();
    assert_eq!(got.id, id);
}

#[test]
fn lookup_miss_on_mtime_change() {
    let conn = open_memory().unwrap();
    cache::upsert_rip_file(&conn, &rip(100, 500)).unwrap();
    let got = cache::lookup_cached(&conn, &PathBuf::from("/rips/album.cue"), 101, 500).unwrap();
    assert!(got.is_none());
}

#[test]
fn lookup_miss_on_size_change() {
    let conn = open_memory().unwrap();
    cache::upsert_rip_file(&conn, &rip(100, 500)).unwrap();
    let got = cache::lookup_cached(&conn, &PathBuf::from("/rips/album.cue"), 100, 999).unwrap();
    assert!(got.is_none());
}

#[test]
fn upsert_preserves_id_when_path_already_known() {
    let conn = open_memory().unwrap();
    let first_id = cache::upsert_rip_file(&conn, &rip(100, 500)).unwrap();

    // Re-scan with a new (mtime, size) pair: same path should update, not insert.
    let updated = rip(200, 600);
    let second_id = cache::upsert_rip_file(&conn, &updated).unwrap();
    assert_eq!(first_id, second_id);

    let all_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM rip_files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(all_count, 1);

    let got = crud::get_rip_file(&conn, first_id).unwrap().unwrap();
    assert_eq!(got.mtime, Some(200));
    assert_eq!(got.size, Some(600));
}

#[test]
fn upsert_inserts_when_path_unknown() {
    let conn = open_memory().unwrap();
    let mut other = rip(100, 500);
    other.cue_path = Some(PathBuf::from("/rips/other.cue"));
    cache::upsert_rip_file(&conn, &other).unwrap();

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM rip_files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn chd_rip_round_trips_via_chd_path() {
    let conn = open_memory().unwrap();
    let chd = RipFile {
        id: 0,
        disc_id: None,
        cue_path: None,
        chd_path: Some(PathBuf::from("/rips/album.chd")),
        bin_paths: Vec::new(),
        mtime: Some(500),
        size: Some(800),
        identification_confidence: IdentificationConfidence::Likely,
        identification_source: Some(IdentificationSource::Import),
        accuraterip_status: None,
        last_verified_at: None,
    };
    let id = cache::upsert_rip_file(&conn, &chd).unwrap();

    let got = cache::lookup_cached(&conn, &PathBuf::from("/rips/album.chd"), 500, 800)
        .unwrap()
        .unwrap();
    assert_eq!(got.id, id);
}
