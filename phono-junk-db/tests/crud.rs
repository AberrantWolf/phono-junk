use std::path::PathBuf;

use phono_junk_catalog::{
    Album, Asset, AssetType, Disagreement, Disc, Override, Release, RipFile, Track,
};
use phono_junk_core::{IdentificationConfidence, IdentificationSource, Toc};
use phono_junk_db::{cache, crud, open_memory};

fn sample_album() -> Album {
    Album {
        id: 0,
        title: "Kid A".into(),
        sort_title: Some("Kid A".into()),
        artist_credit: Some("Radiohead".into()),
        year: Some(2000),
        mbid: Some("album-mbid".into()),
        primary_type: Some("Album".into()),
        secondary_types: vec!["Live".into()],
        first_release_date: Some("2000-10-02".into()),
    }
}

fn sample_release(album_id: i64) -> Release {
    Release {
        id: 0,
        album_id,
        country: Some("GB".into()),
        date: Some("2000-10-02".into()),
        label: Some("Parlophone".into()),
        catalog_number: Some("7243 527753 1 5".into()),
        barcode: Some("724352775316".into()),
        mbid: Some("release-mbid".into()),
        status: Some("Official".into()),
    }
}

fn sample_disc(release_id: i64) -> Disc {
    Disc {
        id: 0,
        release_id,
        disc_number: 1,
        format: "CD".into(),
        toc: Some(Toc {
            first_track: 1,
            last_track: 3,
            leadout_sector: 200_000,
            track_offsets: vec![0, 50_000, 120_000],
        }),
        mb_discid: Some("disc-mb".into()),
        cddb_id: Some("1a2b3c4d".into()),
        ar_discid1: Some("00112233".into()),
        ar_discid2: Some("44556677".into()),
        dbar_raw: None,
    }
}

fn sample_track(disc_id: i64, position: u8) -> Track {
    Track {
        id: 0,
        disc_id,
        position,
        title: Some(format!("Track {position}")),
        artist_credit: Some("Radiohead".into()),
        length_frames: Some(1000 * position as u64),
        isrc: None,
        mbid: Some(format!("recording-{position}")),
        recording_mbid: Some(format!("recording-{position}")),
    }
}

fn sample_rip_file(disc_id: Option<i64>) -> RipFile {
    RipFile {
        id: 0,
        disc_id,
        cue_path: Some(PathBuf::from("/rips/album.cue")),
        chd_path: None,
        bin_paths: vec![PathBuf::from("/rips/album.bin")],
        mtime: Some(1_700_000_000),
        size: Some(700_000_000),
        identification_confidence: IdentificationConfidence::Certain,
        identification_source: Some(IdentificationSource::MusicBrainz),
        accuraterip_status: Some("v2 confidence 8".into()),
        last_verified_at: Some("2026-04-19T00:00:00Z".into()),
    }
}

#[test]
fn album_round_trip() {
    let conn = open_memory().unwrap();
    let album = sample_album();
    let id = crud::insert_album(&conn, &album).unwrap();
    let got = crud::get_album(&conn, id).unwrap().unwrap();
    assert_eq!(got.title, album.title);
    assert_eq!(got.secondary_types, vec!["Live".to_string()]);
    assert_eq!(got.first_release_date.as_deref(), Some("2000-10-02"));
}

#[test]
fn album_update_and_list() {
    let conn = open_memory().unwrap();
    let id = crud::insert_album(&conn, &sample_album()).unwrap();
    let mut album = crud::get_album(&conn, id).unwrap().unwrap();
    album.title = "OK Computer".into();
    crud::update_album(&conn, &album).unwrap();
    let all = crud::list_albums(&conn).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].title, "OK Computer");
}

#[test]
fn release_cascade_on_album_delete() {
    let conn = open_memory().unwrap();
    let album_id = crud::insert_album(&conn, &sample_album()).unwrap();
    let release_id = crud::insert_release(&conn, &sample_release(album_id)).unwrap();
    crud::delete_album(&conn, album_id).unwrap();
    assert!(crud::get_release(&conn, release_id).unwrap().is_none());
}

#[test]
fn disc_round_trip_with_toc_and_dbar() {
    let conn = open_memory().unwrap();
    let album_id = crud::insert_album(&conn, &sample_album()).unwrap();
    let release_id = crud::insert_release(&conn, &sample_release(album_id)).unwrap();
    let id = crud::insert_disc(&conn, &sample_disc(release_id)).unwrap();

    let bytes = vec![0xDEu8, 0xAD, 0xBE, 0xEF];
    crud::set_disc_dbar_raw(&conn, id, &bytes).unwrap();

    let disc = crud::get_disc(&conn, id).unwrap().unwrap();
    assert_eq!(disc.toc.as_ref().unwrap().last_track, 3);
    assert_eq!(disc.dbar_raw.as_deref(), Some(bytes.as_slice()));
    assert_eq!(
        crud::get_disc_dbar_raw(&conn, id).unwrap().as_deref(),
        Some(bytes.as_slice())
    );
}

#[test]
fn find_disc_by_mb_and_ar() {
    let conn = open_memory().unwrap();
    let album_id = crud::insert_album(&conn, &sample_album()).unwrap();
    let release_id = crud::insert_release(&conn, &sample_release(album_id)).unwrap();
    let disc_id = crud::insert_disc(&conn, &sample_disc(release_id)).unwrap();

    let by_mb = crud::find_disc_by_mb_discid(&conn, "disc-mb")
        .unwrap()
        .unwrap();
    assert_eq!(by_mb.id, disc_id);

    let by_ar = crud::find_disc_by_ar_triple(&conn, "00112233", "44556677", "1a2b3c4d")
        .unwrap()
        .unwrap();
    assert_eq!(by_ar.id, disc_id);

    assert!(
        crud::find_disc_by_mb_discid(&conn, "nope")
            .unwrap()
            .is_none()
    );
}

#[test]
fn track_round_trip_and_cascade() {
    let conn = open_memory().unwrap();
    let album_id = crud::insert_album(&conn, &sample_album()).unwrap();
    let release_id = crud::insert_release(&conn, &sample_release(album_id)).unwrap();
    let disc_id = crud::insert_disc(&conn, &sample_disc(release_id)).unwrap();

    for pos in 1..=3 {
        crud::insert_track(&conn, &sample_track(disc_id, pos)).unwrap();
    }

    let tracks = crud::list_tracks_for_disc(&conn, disc_id).unwrap();
    assert_eq!(tracks.len(), 3);
    assert_eq!(tracks[1].position, 2);

    crud::delete_disc(&conn, disc_id).unwrap();
    assert!(crud::list_tracks_for_disc(&conn, disc_id).unwrap().is_empty());
}

#[test]
fn rip_file_round_trip() {
    let conn = open_memory().unwrap();
    let file = sample_rip_file(None);
    let id = crud::insert_rip_file(&conn, &file).unwrap();

    let got = crud::get_rip_file(&conn, id).unwrap().unwrap();
    assert_eq!(got.bin_paths.len(), 1);
    assert_eq!(got.identification_confidence, IdentificationConfidence::Certain);
    assert!(matches!(
        got.identification_source,
        Some(IdentificationSource::MusicBrainz)
    ));

    let unidentified = crud::list_unidentified_rip_files(&conn).unwrap();
    assert_eq!(unidentified.len(), 1);
}

#[test]
fn find_rip_file_for_disc_picks_earliest() {
    let conn = open_memory().unwrap();
    let album_id = crud::insert_album(&conn, &sample_album()).unwrap();
    let release_id = crud::insert_release(&conn, &sample_release(album_id)).unwrap();
    let disc_id = crud::insert_disc(&conn, &sample_disc(release_id)).unwrap();

    // Two rip files linked to the same disc (e.g. a re-rip); the earliest by
    // id should win so behaviour is stable across runs.
    let first_id = crud::insert_rip_file(&conn, &sample_rip_file(Some(disc_id))).unwrap();
    let mut second = sample_rip_file(Some(disc_id));
    second.cue_path = Some(PathBuf::from("/rips/rerip.cue"));
    let second_id = crud::insert_rip_file(&conn, &second).unwrap();
    assert!(second_id > first_id);

    let got = crud::find_rip_file_for_disc(&conn, disc_id).unwrap().unwrap();
    assert_eq!(got.id, first_id);

    // No match → None.
    assert!(crud::find_rip_file_for_disc(&conn, 9_999).unwrap().is_none());
}

#[test]
fn rip_file_disc_set_null_on_disc_delete() {
    let conn = open_memory().unwrap();
    let album_id = crud::insert_album(&conn, &sample_album()).unwrap();
    let release_id = crud::insert_release(&conn, &sample_release(album_id)).unwrap();
    let disc_id = crud::insert_disc(&conn, &sample_disc(release_id)).unwrap();
    let rip_id = crud::insert_rip_file(&conn, &sample_rip_file(Some(disc_id))).unwrap();

    crud::delete_disc(&conn, disc_id).unwrap();
    let got = crud::get_rip_file(&conn, rip_id).unwrap().unwrap();
    assert_eq!(got.disc_id, None);
}

#[test]
fn asset_list_ordered_by_group_and_sequence() {
    let conn = open_memory().unwrap();
    let album_id = crud::insert_album(&conn, &sample_album()).unwrap();
    let release_id = crud::insert_release(&conn, &sample_release(album_id)).unwrap();

    // Two booklet pages in group 1, plus a front cover.
    crud::insert_asset(
        &conn,
        &Asset {
            id: 0,
            release_id,
            asset_type: AssetType::Booklet,
            group_id: Some(1),
            sequence: 2,
            source_url: None,
            file_path: Some(PathBuf::from("/art/booklet-2.jpg")),
            scraped_at: None,
        },
    )
    .unwrap();
    crud::insert_asset(
        &conn,
        &Asset {
            id: 0,
            release_id,
            asset_type: AssetType::Booklet,
            group_id: Some(1),
            sequence: 1,
            source_url: None,
            file_path: Some(PathBuf::from("/art/booklet-1.jpg")),
            scraped_at: None,
        },
    )
    .unwrap();
    crud::insert_asset(
        &conn,
        &Asset {
            id: 0,
            release_id,
            asset_type: AssetType::FrontCover,
            group_id: None,
            sequence: 0,
            source_url: Some("https://example/cover.jpg".into()),
            file_path: None,
            scraped_at: None,
        },
    )
    .unwrap();

    let assets = crud::list_assets_for_release(&conn, release_id).unwrap();
    assert_eq!(assets.len(), 3);
    // Front cover (no group) sorts first via COALESCE(group_id, -1).
    assert_eq!(assets[0].asset_type, AssetType::FrontCover);
    assert_eq!(assets[1].sequence, 1);
    assert_eq!(assets[2].sequence, 2);
}

#[test]
fn asset_type_other_round_trips() {
    let conn = open_memory().unwrap();
    let album_id = crud::insert_album(&conn, &sample_album()).unwrap();
    let release_id = crud::insert_release(&conn, &sample_release(album_id)).unwrap();
    let id = crud::insert_asset(
        &conn,
        &Asset {
            id: 0,
            release_id,
            asset_type: AssetType::Other("poster".into()),
            group_id: None,
            sequence: 0,
            source_url: None,
            file_path: None,
            scraped_at: None,
        },
    )
    .unwrap();
    let got = crud::get_asset(&conn, id).unwrap().unwrap();
    assert_eq!(got.asset_type, AssetType::Other("poster".into()));
}

#[test]
fn disagreement_and_override_round_trip() {
    let conn = open_memory().unwrap();
    let d = Disagreement {
        id: 0,
        entity_type: "Album".into(),
        entity_id: 1,
        field: "title".into(),
        source_a: "MusicBrainz".into(),
        value_a: "Kid A".into(),
        source_b: "Discogs".into(),
        value_b: "KID A".into(),
        resolved: false,
        created_at: None,
    };
    let d_id = crud::insert_disagreement(&conn, &d).unwrap();
    let got = crud::get_disagreement(&conn, d_id).unwrap().unwrap();
    assert_eq!(got.field, "title");
    assert!(got.created_at.is_some());

    let o = Override {
        id: 0,
        entity_type: "Album".into(),
        entity_id: 1,
        sub_path: Some("track[6].title".into()),
        field: "title".into(),
        override_value: "National Anthem".into(),
        reason: Some("typo in upstream".into()),
        created_at: None,
    };
    let o_id = crud::insert_override(&conn, &o).unwrap();
    let list = crud::list_overrides_for(&conn, "Album", 1).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, o_id);
}

#[test]
fn end_to_end_identify_flow() {
    // The plan's closing verification sequence, executed as a test.
    let conn = open_memory().unwrap();

    let album_id = crud::insert_album(&conn, &sample_album()).unwrap();
    let release_id = crud::insert_release(&conn, &sample_release(album_id)).unwrap();
    let disc_id = crud::insert_disc(&conn, &sample_disc(release_id)).unwrap();

    for pos in 1..=3 {
        crud::insert_track(&conn, &sample_track(disc_id, pos)).unwrap();
    }

    let rip = sample_rip_file(Some(disc_id));
    let rip_id = cache::upsert_rip_file(&conn, &rip).unwrap();

    let cue_path = rip.cue_path.as_deref().unwrap();
    let hit = cache::lookup_cached(
        &conn,
        cue_path,
        rip.mtime.unwrap(),
        rip.size.unwrap(),
    )
    .unwrap();
    assert!(hit.is_some());
    assert_eq!(hit.unwrap().id, rip_id);
}
