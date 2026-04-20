//! Integration tests for redumper sidecar ingestion.
//!
//! Covers `collect_redumper_sidecars`, `enrich_disc_ids`, and
//! `apply_sidecar_to_catalog`. The full scan-through-identify path is
//! exercised by the smoke suite — here we validate the sidecar module's
//! own contract with fixture-grade synthetic inputs.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{TimeZone, Utc};
use junk_libs_disc::redumper::Ripper;
use phono_junk_catalog::{Album, Disc, Release, Track};
use phono_junk_core::DiscIds;
use phono_junk_db::{crud, open_memory};
use phono_junk_lib::sidecar::{
    apply_sidecar_to_catalog, collect_redumper_sidecars, enrich_disc_ids,
};

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

const SYNTHETIC_LOG: &str = "\
redumper v2024.03.01 build_20240301

2024-01-15 14:23:45

drive: ASUS - BW-16D1HT (fw 3.00)
read offset: +6

MCN: 0727361234567

ISRC:
  01 USRC17654321
  02 USRC17654322

done.
";

fn write_cue_with_sidecars(dir: &Path, name: &str, log: Option<&str>) -> PathBuf {
    let base = dir.join(name);
    fs::write(base.with_extension("cue"), b"FILE \"a.bin\" BINARY\n").unwrap();
    if let Some(log_text) = log {
        fs::write(base.with_extension("log"), log_text).unwrap();
    }
    base.with_extension("cue")
}

fn seed_catalog(conn: &rusqlite::Connection) -> phono_junk_catalog::Id {
    let album_id = crud::insert_album(
        conn,
        &Album {
            id: 0,
            title: "Test Album".into(),
            sort_title: None,
            artist_credit: None,
            year: None,
            mbid: None,
            primary_type: None,
            secondary_types: Vec::new(),
            first_release_date: None,
        },
    )
    .unwrap();
    let release_id = crud::insert_release(
        conn,
        &Release {
            id: 0,
            album_id,
            country: None,
            date: None,
            label: None,
            catalog_number: None,
            barcode: None,
            mbid: None,
            status: None,
            language: None,
            script: None,
        },
    )
    .unwrap();
    let disc_id = crud::insert_disc(
        conn,
        &Disc {
            id: 0,
            release_id,
            disc_number: 1,
            format: "CD".into(),
            toc: None,
            mb_discid: None,
            cddb_id: None,
            ar_discid1: None,
            ar_discid2: None,
            dbar_raw: None,
            mcn: None,
        },
    )
    .unwrap();
    for pos in 1..=3u8 {
        crud::insert_track(
            conn,
            &Track {
                id: 0,
                disc_id,
                position: pos,
                title: None,
                artist_credit: None,
                length_frames: None,
                isrc: None,
                mbid: None,
                recording_mbid: None,
            },
        )
        .unwrap();
    }
    disc_id
}

// ---------------------------------------------------------------------
// collect_redumper_sidecars
// ---------------------------------------------------------------------

#[test]
fn collect_returns_empty_when_no_sidecars() {
    let td = tempfile::tempdir().unwrap();
    let cue = write_cue_with_sidecars(td.path(), "album", None);
    let data = collect_redumper_sidecars(&cue);
    assert!(data.is_empty(), "CUE-only directory yields empty sidecar data");
}

#[test]
fn collect_parses_log_and_stamps_redumper_provenance() {
    let td = tempfile::tempdir().unwrap();
    let cue = write_cue_with_sidecars(td.path(), "album", Some(SYNTHETIC_LOG));
    let data = collect_redumper_sidecars(&cue);

    assert_eq!(data.mcn.as_deref(), Some("0727361234567"));
    assert_eq!(data.isrcs.len(), 2);
    assert_eq!(data.isrcs.get(&1).map(String::as_str), Some("USRC17654321"));
    let prov = data.provenance.as_ref().expect("provenance stamped");
    assert_eq!(prov.ripper, Ripper::Redumper);
    assert_eq!(prov.read_offset, Some(6));
    assert_eq!(
        prov.version.as_deref(),
        Some("v2024.03.01 build_20240301")
    );
    assert_eq!(
        prov.rip_date,
        Some(Utc.with_ymd_and_hms(2024, 1, 15, 14, 23, 45).unwrap())
    );
    assert_eq!(
        prov.drive.as_ref().map(|d| d.vendor.as_str()),
        Some("ASUS")
    );
}

#[test]
fn collect_on_eac_log_stamps_unknown_provenance() {
    let td = tempfile::tempdir().unwrap();
    let cue = write_cue_with_sidecars(
        td.path(),
        "album",
        Some("Exact Audio Copy V1.5\n\nlog contents...\n"),
    );
    let data = collect_redumper_sidecars(&cue);

    let prov = data.provenance.as_ref().expect("unknown provenance stamped");
    assert_eq!(prov.ripper, Ripper::Unknown);
    assert!(prov.version.is_none(), "no version on unrecognised log");
    // No MCN / ISRCs were extracted since the parser rejected the file.
    assert!(data.mcn.is_none());
    assert!(data.isrcs.is_empty());
}

// ---------------------------------------------------------------------
// enrich_disc_ids
// ---------------------------------------------------------------------

#[test]
fn enrich_mirrors_mcn_to_barcode_when_empty() {
    let mut ids = DiscIds::default();
    let data = collect_redumper_sidecars(&{
        let td = tempfile::tempdir().unwrap();
        let cue = write_cue_with_sidecars(td.path(), "album", Some(SYNTHETIC_LOG));
        // Leak the tempdir for the duration of this test: the path needs
        // to exist only long enough for find_sidecars() to stat it, which
        // happens during collect_redumper_sidecars above.
        std::mem::forget(td);
        cue
    });
    enrich_disc_ids(&mut ids, &data);
    assert_eq!(ids.barcode.as_deref(), Some("0727361234567"));
}

#[test]
fn enrich_preserves_existing_barcode() {
    let mut ids = DiscIds {
        barcode: Some("provider-barcode".into()),
        ..Default::default()
    };
    let td = tempfile::tempdir().unwrap();
    let cue = write_cue_with_sidecars(td.path(), "album", Some(SYNTHETIC_LOG));
    let data = collect_redumper_sidecars(&cue);
    enrich_disc_ids(&mut ids, &data);
    assert_eq!(
        ids.barcode.as_deref(),
        Some("provider-barcode"),
        "existing barcode wins over sidecar"
    );
}

// ---------------------------------------------------------------------
// apply_sidecar_to_catalog
// ---------------------------------------------------------------------

#[test]
fn apply_writes_mcn_and_isrcs() {
    let conn = open_memory().unwrap();
    let disc_id = seed_catalog(&conn);

    let td = tempfile::tempdir().unwrap();
    let cue = write_cue_with_sidecars(td.path(), "album", Some(SYNTHETIC_LOG));
    let data = collect_redumper_sidecars(&cue);

    apply_sidecar_to_catalog(&conn, disc_id, &data).unwrap();

    let disc = crud::get_disc(&conn, disc_id).unwrap().unwrap();
    assert_eq!(disc.mcn.as_deref(), Some("0727361234567"));

    let tracks = crud::list_tracks_for_disc(&conn, disc_id).unwrap();
    let t1 = tracks.iter().find(|t| t.position == 1).unwrap();
    let t2 = tracks.iter().find(|t| t.position == 2).unwrap();
    let t3 = tracks.iter().find(|t| t.position == 3).unwrap();
    assert_eq!(t1.isrc.as_deref(), Some("USRC17654321"));
    assert_eq!(t2.isrc.as_deref(), Some("USRC17654322"));
    assert!(t3.isrc.is_none(), "track 3 has no ISRC in the log");
}

#[test]
fn apply_preserves_existing_mcn_and_isrc() {
    let conn = open_memory().unwrap();
    let disc_id = seed_catalog(&conn);

    // Pre-stamp with different values from a notional earlier source.
    let mut disc = crud::get_disc(&conn, disc_id).unwrap().unwrap();
    disc.mcn = Some("pre-existing".into());
    crud::update_disc(&conn, &disc).unwrap();
    let mut t1 = crud::list_tracks_for_disc(&conn, disc_id)
        .unwrap()
        .into_iter()
        .find(|t| t.position == 1)
        .unwrap();
    t1.isrc = Some("HIGHER-TRUST".into());
    crud::update_track(&conn, &t1).unwrap();

    let td = tempfile::tempdir().unwrap();
    let cue = write_cue_with_sidecars(td.path(), "album", Some(SYNTHETIC_LOG));
    let data = collect_redumper_sidecars(&cue);
    apply_sidecar_to_catalog(&conn, disc_id, &data).unwrap();

    let disc = crud::get_disc(&conn, disc_id).unwrap().unwrap();
    assert_eq!(disc.mcn.as_deref(), Some("pre-existing"));
    let tracks = crud::list_tracks_for_disc(&conn, disc_id).unwrap();
    let t1 = tracks.iter().find(|t| t.position == 1).unwrap();
    assert_eq!(t1.isrc.as_deref(), Some("HIGHER-TRUST"));
    // Track 2 was empty so the sidecar write still applies.
    let t2 = tracks.iter().find(|t| t.position == 2).unwrap();
    assert_eq!(t2.isrc.as_deref(), Some("USRC17654322"));
}

#[test]
fn apply_raises_disagreement_on_mcn_upc_mismatch() {
    let conn = open_memory().unwrap();
    let disc_id = seed_catalog(&conn);

    // Hand-build a SidecarData with conflicting MCN vs CD-TEXT UPC.
    use phono_junk_lib::sidecar::SidecarData;
    let data = SidecarData {
        mcn: Some("0727361234567".into()),
        cdtext_upc: Some("9999999999999".into()),
        ..Default::default()
    };
    apply_sidecar_to_catalog(&conn, disc_id, &data).unwrap();

    let disagreements = crud::list_disagreements_for(&conn, "disc", disc_id).unwrap();
    let d = disagreements
        .iter()
        .find(|d| d.field == "mcn")
        .expect("MCN vs UPC disagreement written");
    assert_eq!(d.value_a, "0727361234567");
    assert_eq!(d.value_b, "9999999999999");
    assert!(d.source_a.starts_with("Redumper/"));
    assert!(d.source_b.starts_with("Redumper/"));
    assert!(!d.resolved);
}

// ---------------------------------------------------------------------
// refresh_for_cache_hit — Bug 2 / Sprint 26
// ---------------------------------------------------------------------

#[test]
fn refresh_writes_provenance_when_row_had_none() {
    use phono_junk_catalog::RipFile;
    use phono_junk_core::{IdentificationConfidence, IdentificationState};
    use phono_junk_lib::sidecar::refresh_for_cache_hit;

    let conn = open_memory().unwrap();
    let disc_id = seed_catalog(&conn);

    let td = tempfile::tempdir().unwrap();
    let cue = write_cue_with_sidecars(td.path(), "album", Some(SYNTHETIC_LOG));

    // Insert a rip row in Identified state, with no provenance — the
    // "missing redumper" false-positive shape.
    let rip_id = crud::insert_rip_file(
        &conn,
        &RipFile {
            id: 0,
            disc_id: Some(disc_id),
            cue_path: Some(cue.clone()),
            chd_path: None,
            bin_paths: Vec::new(),
            mtime: Some(1),
            size: Some(1),
            identification_confidence: IdentificationConfidence::Certain,
            identification_source: None,
            accuraterip_status: None,
            last_verified_at: None,
            last_identify_errors: None,
            last_identify_at: None,
            provenance: None,
            identification_state: IdentificationState::Identified,
            last_state_change_at: None,
        },
    )
    .unwrap();

    assert!(crud::load_rip_file_provenance(&conn, rip_id).unwrap().is_none());

    let changed = refresh_for_cache_hit(&conn, rip_id, Some(disc_id), &cue).unwrap();
    assert!(changed, "refresh should report that something changed");

    let prov = crud::load_rip_file_provenance(&conn, rip_id)
        .unwrap()
        .expect("provenance written on cache-hit refresh");
    assert_eq!(prov.ripper, junk_libs_disc::redumper::Ripper::Redumper);

    // Catalog-facing facts land too.
    let disc = crud::get_disc(&conn, disc_id).unwrap().unwrap();
    assert_eq!(disc.mcn.as_deref(), Some("0727361234567"));
}

#[test]
fn refresh_noop_when_no_sidecars_exist() {
    use phono_junk_lib::sidecar::refresh_for_cache_hit;

    let conn = open_memory().unwrap();
    let disc_id = seed_catalog(&conn);
    let td = tempfile::tempdir().unwrap();
    let cue = write_cue_with_sidecars(td.path(), "bare", None);

    let changed = refresh_for_cache_hit(&conn, 0, Some(disc_id), &cue).unwrap();
    assert!(!changed, "no sidecars next to cue → no-op");
}

#[test]
fn refresh_preserves_existing_provenance() {
    use junk_libs_disc::redumper::Ripper;
    use phono_junk_catalog::{RipFile, RipperProvenance};
    use phono_junk_core::{IdentificationConfidence, IdentificationState};
    use phono_junk_lib::sidecar::refresh_for_cache_hit;

    let conn = open_memory().unwrap();
    let disc_id = seed_catalog(&conn);
    let td = tempfile::tempdir().unwrap();
    let cue = write_cue_with_sidecars(td.path(), "album", Some(SYNTHETIC_LOG));

    // Pre-existing provenance with a different version should win.
    let earlier = RipperProvenance {
        ripper: Ripper::Redumper,
        version: Some("v1.0.0 build_earlier".into()),
        drive: None,
        read_offset: None,
        log_path: cue.with_extension("log"),
        rip_date: None,
    };
    let rip_id = crud::insert_rip_file(
        &conn,
        &RipFile {
            id: 0,
            disc_id: Some(disc_id),
            cue_path: Some(cue.clone()),
            chd_path: None,
            bin_paths: Vec::new(),
            mtime: Some(1),
            size: Some(1),
            identification_confidence: IdentificationConfidence::Certain,
            identification_source: None,
            accuraterip_status: None,
            last_verified_at: None,
            last_identify_errors: None,
            last_identify_at: None,
            provenance: Some(earlier.clone()),
            identification_state: IdentificationState::Identified,
            last_state_change_at: None,
        },
    )
    .unwrap();

    refresh_for_cache_hit(&conn, rip_id, Some(disc_id), &cue).unwrap();

    // Provenance version kept the earlier one.
    let prov = crud::load_rip_file_provenance(&conn, rip_id).unwrap().unwrap();
    assert_eq!(prov.version.as_deref(), Some("v1.0.0 build_earlier"));
}

#[test]
fn apply_on_matching_mcn_upc_writes_no_disagreement() {
    let conn = open_memory().unwrap();
    let disc_id = seed_catalog(&conn);

    use phono_junk_lib::sidecar::SidecarData;
    let data = SidecarData {
        mcn: Some("0727361234567".into()),
        cdtext_upc: Some("0727361234567".into()),
        ..Default::default()
    };
    apply_sidecar_to_catalog(&conn, disc_id, &data).unwrap();

    let disagreements = crud::list_disagreements_for(&conn, "disc", disc_id).unwrap();
    assert!(disagreements.iter().all(|d| d.field != "mcn"));
}
