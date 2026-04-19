//! End-to-end extract-pipeline tests with an in-memory SQLite catalog and
//! a synthetic BIN+CUE on a temp directory.
//!
//! Verifies: output tree layout, claxon round-trip of silence, Vorbis tag
//! presence, embedded METADATA_BLOCK_PICTURE, and the locally-cached
//! `cover.jpg` sidecar. No HTTP is exercised — the test seeds the Asset
//! row with a pre-populated `file_path` so the cover-fetch path is skipped.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use claxon::FlacReader;
use junk_libs_disc::layout::LEAD_IN_FRAMES;
use metaflac::{Tag, block::Block, block::PictureType};
use phono_junk_catalog::{Album, Asset, AssetType, Disc, Release, RipFile, Track};
use phono_junk_core::{IdentificationConfidence, IdentificationSource, Toc};
use phono_junk_db::{crud, open_memory};
use phono_junk_lib::PhonoContext;
use rusqlite::Connection;

const RAW_SECTOR_SIZE: u64 = 2352;
const TRACK_SECTORS: u32 = 5;
const TRACK_COUNT: u32 = 2;

/// Build a minimal CUE + matching whole-disc BIN under `dir`. Returns
/// `(cue_path, bin_path)`. The BIN is a plain on-disk file of zeros —
/// exercising the encoder on silence is enough to validate the layout.
fn write_fixture_cue_and_bin(dir: &Path) -> (PathBuf, PathBuf) {
    let cue = dir.join("album.cue");
    let bin = dir.join("album.bin");
    let total_sectors = TRACK_SECTORS * TRACK_COUNT;
    let bin_len = total_sectors as u64 * RAW_SECTOR_SIZE;
    {
        let f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&bin)
            .unwrap();
        f.set_len(bin_len).unwrap();
    }

    // Two-track CUE. Track 1 at sector 0 (absolute 150), track 2 at
    // sector TRACK_SECTORS (absolute 150 + TRACK_SECTORS). MSF encoding
    // mm:ss:ff where 75 frames = 1 second; at 5 sectors that's 00:00:05.
    let t2_msf = format!("00:00:{:02}", TRACK_SECTORS);
    let cue_body = format!(
        "FILE \"album.bin\" BINARY\n  \
         TRACK 01 AUDIO\n    INDEX 01 00:00:00\n  \
         TRACK 02 AUDIO\n    INDEX 01 {t2_msf}\n"
    );
    let mut f = File::create(&cue).unwrap();
    f.write_all(cue_body.as_bytes()).unwrap();

    (cue, bin)
}

fn sample_album() -> Album {
    Album {
        id: 0,
        title: "Test Album".into(),
        sort_title: None,
        artist_credit: Some("Test Artist".into()),
        year: Some(2025),
        mbid: Some("album-mbid-xyz".into()),
        primary_type: Some("Album".into()),
        secondary_types: Vec::new(),
        first_release_date: Some("2025-01-15".into()),
    }
}

fn sample_release(album_id: i64) -> Release {
    Release {
        id: 0,
        album_id,
        country: Some("US".into()),
        date: Some("2025-01-15".into()),
        label: None,
        catalog_number: None,
        barcode: None,
        mbid: Some("release-mbid".into()),
        status: Some("Official".into()),
    }
}

fn sample_disc(release_id: i64, toc: Toc) -> Disc {
    Disc {
        id: 0,
        release_id,
        disc_number: 1,
        format: "CD".into(),
        toc: Some(toc),
        mb_discid: Some("discid-xyz".into()),
        cddb_id: Some("12345678".into()),
        ar_discid1: Some("00000001".into()),
        ar_discid2: Some("00000002".into()),
        dbar_raw: None,
    }
}

fn sample_track(disc_id: i64, position: u8, title: &str) -> Track {
    Track {
        id: 0,
        disc_id,
        position,
        title: Some(title.into()),
        artist_credit: Some("Test Artist".into()),
        length_frames: Some(TRACK_SECTORS as u64),
        isrc: None,
        mbid: Some(format!("recording-{position}")),
        recording_mbid: Some(format!("recording-{position}")),
    }
}

fn sample_rip_file(disc_id: i64, cue: &Path, bin: &Path) -> RipFile {
    RipFile {
        id: 0,
        disc_id: Some(disc_id),
        cue_path: Some(cue.to_path_buf()),
        chd_path: None,
        bin_paths: vec![bin.to_path_buf()],
        mtime: None,
        size: None,
        identification_confidence: IdentificationConfidence::Certain,
        identification_source: Some(IdentificationSource::MusicBrainz),
        accuraterip_status: None,
        last_verified_at: None,
    }
}

fn seed_catalog(conn: &Connection, cue: &Path, bin: &Path) -> i64 {
    let album_id = crud::insert_album(conn, &sample_album()).unwrap();
    let release_id = crud::insert_release(conn, &sample_release(album_id)).unwrap();
    let toc = Toc {
        first_track: 1,
        last_track: TRACK_COUNT as u8,
        leadout_sector: LEAD_IN_FRAMES + TRACK_SECTORS * TRACK_COUNT,
        track_offsets: vec![LEAD_IN_FRAMES, LEAD_IN_FRAMES + TRACK_SECTORS],
    };
    let disc_id = crud::insert_disc(conn, &sample_disc(release_id, toc)).unwrap();
    crud::insert_track(conn, &sample_track(disc_id, 1, "First Track")).unwrap();
    crud::insert_track(conn, &sample_track(disc_id, 2, "Second / Track")).unwrap();
    crud::insert_rip_file(conn, &sample_rip_file(disc_id, cue, bin)).unwrap();
    disc_id
}

fn seed_cover(conn: &Connection, release_id: i64, library_root: &Path) -> Vec<u8> {
    // Minimal JPEG SOI+APP0 prefix so downstream tooling doesn't balk.
    let cover: Vec<u8> = b"\xFF\xD8\xFF\xE0\x00\x10JFIFfakedata".to_vec();
    // Pre-cache the bytes and point the Asset row at the cached path, so
    // the extract path skips HTTP entirely.
    let rel = PathBuf::from(".cache").join("assets").join("1.jpg");
    let abs = library_root.join(&rel);
    fs::create_dir_all(abs.parent().unwrap()).unwrap();
    fs::write(&abs, &cover).unwrap();
    let asset = Asset {
        id: 0,
        release_id,
        asset_type: AssetType::FrontCover,
        group_id: None,
        sequence: 0,
        source_url: Some("https://example.test/cover.jpg".into()),
        file_path: Some(rel),
        scraped_at: None,
    };
    crud::insert_asset(conn, &asset).unwrap();
    cover
}

#[test]
fn export_writes_tree_with_tagged_flacs_and_cover() {
    let tmp = tempfile::tempdir().unwrap();
    let rips_dir = tmp.path().join("rips");
    fs::create_dir_all(&rips_dir).unwrap();
    let (cue, bin) = write_fixture_cue_and_bin(&rips_dir);

    let library = tmp.path().join("library");
    fs::create_dir_all(&library).unwrap();

    let conn = open_memory().unwrap();
    let disc_id = seed_catalog(&conn, &cue, &bin);
    let release_id = crud::get_disc(&conn, disc_id).unwrap().unwrap().release_id;
    let cover_bytes = seed_cover(&conn, release_id, &library);

    let ctx = PhonoContext::new();
    let outcome = ctx.export_disc(&conn, disc_id, &library).unwrap();

    assert_eq!(outcome.disc_id, disc_id);
    assert!(outcome.cover_written, "cover.jpg should be written");

    let album_dir = library.join("Test Artist").join("Test Album (2025)");
    let track1 = album_dir.join("01 - First Track.flac");
    let track2 = album_dir.join("02 - Second _ Track.flac");
    let cover = album_dir.join("cover.jpg");

    assert!(track1.exists(), "track 1 flac written: {}", track1.display());
    assert!(track2.exists(), "track 2 flac written");
    assert!(cover.exists(), "cover.jpg written");
    assert_eq!(fs::read(&cover).unwrap(), cover_bytes, "cover bytes match");
    assert_eq!(
        outcome.written.len(),
        3,
        "written list includes both tracks + cover"
    );

    // claxon decodes silence (sparse BIN = zeros).
    let mut reader = FlacReader::open(&track1).unwrap();
    let info = reader.streaminfo();
    assert_eq!(info.sample_rate, 44_100);
    assert_eq!(info.channels, 2);
    assert_eq!(info.samples, Some(TRACK_SECTORS as u64 * 588));
    for s in reader.samples() {
        assert_eq!(s.unwrap(), 0, "silence");
    }

    // Tags + embedded picture block.
    let tag = Tag::read_from_path(&track1).unwrap();
    let vc = tag.vorbis_comments().expect("vorbis comments present");
    assert_eq!(vc.get("ALBUM").unwrap(), &vec!["Test Album".to_string()]);
    assert_eq!(vc.get("ALBUMARTIST").unwrap(), &vec!["Test Artist".to_string()]);
    assert_eq!(vc.get("ARTIST").unwrap(), &vec!["Test Artist".to_string()]);
    assert_eq!(vc.get("TITLE").unwrap(), &vec!["First Track".to_string()]);
    assert_eq!(vc.get("TRACKNUMBER").unwrap(), &vec!["1".to_string()]);
    assert_eq!(vc.get("TOTALTRACKS").unwrap(), &vec!["2".to_string()]);
    assert_eq!(vc.get("DISCNUMBER").unwrap(), &vec!["1".to_string()]);
    assert_eq!(vc.get("TOTALDISCS").unwrap(), &vec!["1".to_string()]);
    assert_eq!(vc.get("DATE").unwrap(), &vec!["2025-01-15".to_string()]);
    assert_eq!(
        vc.get("MUSICBRAINZ_ALBUMID").unwrap(),
        &vec!["album-mbid-xyz".to_string()]
    );
    assert_eq!(
        vc.get("MUSICBRAINZ_RELEASETRACKID").unwrap(),
        &vec!["recording-1".to_string()]
    );

    let picture = tag
        .blocks()
        .find_map(|b| if let Block::Picture(p) = b { Some(p) } else { None })
        .expect("picture block");
    assert_eq!(picture.picture_type, PictureType::CoverFront);
    assert_eq!(picture.mime_type, "image/jpeg");
    assert_eq!(picture.data, cover_bytes);
}

#[test]
fn export_without_front_cover_skips_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let rips_dir = tmp.path().join("rips");
    fs::create_dir_all(&rips_dir).unwrap();
    let (cue, bin) = write_fixture_cue_and_bin(&rips_dir);

    let library = tmp.path().join("library");
    fs::create_dir_all(&library).unwrap();

    let conn = open_memory().unwrap();
    let disc_id = seed_catalog(&conn, &cue, &bin);
    // Deliberately skip seeding the cover asset row.

    let ctx = PhonoContext::new();
    let outcome = ctx.export_disc(&conn, disc_id, &library).unwrap();

    assert!(!outcome.cover_written);
    let album_dir = library.join("Test Artist").join("Test Album (2025)");
    assert!(!album_dir.join("cover.jpg").exists());

    let track1 = album_dir.join("01 - First Track.flac");
    assert!(track1.exists());
    // No embedded picture block in the FLAC either.
    let tag = Tag::read_from_path(&track1).unwrap();
    assert!(!tag.blocks().any(|b| matches!(b, Block::Picture(_))));
}
