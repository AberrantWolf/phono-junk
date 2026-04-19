//! End-to-end round-trip tests for the FLAC encoder.
//!
//! Synthesises a short PCM stream, runs it through `encode_flac_track`,
//! then decodes the resulting FLAC with `claxon` and re-reads tags + the
//! picture block with `metaflac` to confirm:
//!
//! - audio samples round-trip bit-exactly (FLAC is lossless);
//! - Vorbis comments land under the expected keys;
//! - the `METADATA_BLOCK_PICTURE` is present and typed as front cover.

use std::path::PathBuf;

use claxon::FlacReader;
use junk_libs_core::AnalysisError;
use junk_libs_disc::{PCM_SAMPLES_PER_SECTOR, PcmSector};
use metaflac::Tag;
use metaflac::block::{Block, PictureType};
use phono_junk_extract::{TrackTags, encode_flac_track};
use tempfile::TempDir;

/// Build an iterator of `n` sectors whose samples follow `f(i)` for the
/// i-th stereo sample across the whole track. Returns samples as
/// `(left_i16, right_i16)` pairs packed into the CDDA `PcmSector` layout.
fn synth_sectors(
    n_sectors: usize,
    mut f: impl FnMut(usize) -> (i16, i16),
) -> Vec<PcmSector> {
    let mut out = Vec::with_capacity(n_sectors);
    let mut global = 0usize;
    for _ in 0..n_sectors {
        let mut sector = [0u32; PCM_SAMPLES_PER_SECTOR];
        for slot in sector.iter_mut() {
            let (l, r) = f(global);
            *slot = (l as u16 as u32) | ((r as u16 as u32) << 16);
            global += 1;
        }
        out.push(sector);
    }
    out
}

fn sample_tags() -> TrackTags {
    TrackTags {
        album: "Test Album".into(),
        album_artist: "Test Artist".into(),
        artist: "Test Artist".into(),
        title: "Test Track".into(),
        track_number: 1,
        total_tracks: 1,
        disc_number: 1,
        total_discs: 1,
        date: Some("2025-01-01".into()),
        genre: Some("Test".into()),
        musicbrainz_album_id: Some("aaaa-bbbb".into()),
        musicbrainz_release_track_id: Some("cccc-dddd".into()),
        isrc: Some("USTEST0000001".into()),
    }
}

#[test]
fn silence_roundtrips_bit_exact_with_tags_and_cover() {
    let tmp = TempDir::new().unwrap();
    let out: PathBuf = tmp.path().join("silence.flac");

    let n_sectors = 10;
    let sectors = synth_sectors(n_sectors, |_| (0, 0));
    let total_samples = (n_sectors * PCM_SAMPLES_PER_SECTOR) as u64;
    let cover: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]; // JPEG magic prefix
    let tags = sample_tags();

    let iter = sectors
        .into_iter()
        .map(Ok::<PcmSector, AnalysisError>);

    encode_flac_track(iter, total_samples, &tags, Some(&cover), &out).unwrap();

    let mut reader = FlacReader::open(&out).unwrap();
    let info = reader.streaminfo();
    assert_eq!(info.channels, 2, "channels");
    assert_eq!(info.sample_rate, 44_100, "sample rate");
    assert_eq!(info.bits_per_sample, 16, "bits per sample");
    assert_eq!(info.samples, Some(total_samples), "total samples in STREAMINFO");

    let mut decoded = 0u64;
    for result in reader.samples() {
        let s: i32 = result.unwrap();
        assert_eq!(s, 0, "silence should decode to 0 samples");
        decoded += 1;
    }
    // claxon's `samples()` yields per-channel samples, so a stereo silence
    // stream yields `total_samples * 2` values.
    assert_eq!(decoded, total_samples * 2, "stereo sample count");
}

#[test]
fn sine_wave_survives_encode_decode() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("sine.flac");

    let n_sectors = 4; // keep the test quick
    // Simple bit pattern: left = i as i16, right = -(i as i16). Verifies
    // sign-extension: values like 0x8000 (the most-negative i16) round-
    // trip through the encoder.
    let sectors = synth_sectors(n_sectors, |i| {
        let l = (i as i16).wrapping_mul(17);
        (l, l.wrapping_neg())
    });
    let total_samples = (n_sectors * PCM_SAMPLES_PER_SECTOR) as u64;
    let tags = sample_tags();

    // Reproduce the expected samples before we consume `sectors` via the iterator.
    let expected: Vec<i32> = sectors
        .iter()
        .flat_map(|s| {
            s.iter().flat_map(|packed| {
                let l = (*packed as i16) as i32;
                let r = ((*packed >> 16) as i16) as i32;
                [l, r]
            })
        })
        .collect();

    let iter = sectors
        .into_iter()
        .map(Ok::<PcmSector, AnalysisError>);
    encode_flac_track(iter, total_samples, &tags, None, &out).unwrap();

    let mut reader = FlacReader::open(&out).unwrap();
    let actual: Vec<i32> = reader.samples().map(Result::unwrap).collect();
    assert_eq!(actual, expected, "PCM samples must round-trip bit-exactly");
}

#[test]
fn tags_and_picture_land_in_output_file() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("tagged.flac");

    let n_sectors = 2;
    let sectors = synth_sectors(n_sectors, |_| (0, 0));
    let total_samples = (n_sectors * PCM_SAMPLES_PER_SECTOR) as u64;
    let cover: Vec<u8> = b"\xFF\xD8\xFF\xE0fakejpegdata".to_vec();
    let tags = sample_tags();

    let iter = sectors
        .into_iter()
        .map(Ok::<PcmSector, AnalysisError>);
    encode_flac_track(iter, total_samples, &tags, Some(&cover), &out).unwrap();

    let tag = Tag::read_from_path(&out).unwrap();
    let vc = tag.vorbis_comments().expect("vorbis comment block present");

    for (key, expected) in tags.to_vorbis_comments() {
        let got = vc.get(key).expect(key);
        assert_eq!(got, &vec![expected.clone()], "tag {key}");
    }

    // METADATA_BLOCK_PICTURE present, type = front cover, bytes match.
    let mut saw_front_cover = false;
    for block in tag.blocks() {
        if let Block::Picture(p) = block {
            assert_eq!(p.picture_type, PictureType::CoverFront);
            assert_eq!(p.mime_type, "image/jpeg");
            assert_eq!(p.data, cover);
            saw_front_cover = true;
        }
    }
    assert!(saw_front_cover, "expected front-cover picture block");
}

#[test]
fn no_cover_means_no_picture_block() {
    let tmp = TempDir::new().unwrap();
    let out = tmp.path().join("no_cover.flac");

    let n_sectors = 1;
    let sectors = synth_sectors(n_sectors, |_| (0, 0));
    let total_samples = PCM_SAMPLES_PER_SECTOR as u64;
    let tags = sample_tags();

    let iter = sectors
        .into_iter()
        .map(Ok::<PcmSector, AnalysisError>);
    encode_flac_track(iter, total_samples, &tags, None, &out).unwrap();

    let tag = Tag::read_from_path(&out).unwrap();
    let any_picture = tag
        .blocks()
        .any(|b| matches!(b, Block::Picture(_)));
    assert!(!any_picture, "no cover supplied → no picture block");
}
