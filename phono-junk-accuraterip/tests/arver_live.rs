//! Cross-verification of AccurateRip CRC v1/v2 against ARver's published
//! expected values.
//!
//! Network-gated: these tests fetch small WAV fixtures from the ARver
//! repository on each run. Invoke with `cargo test -p phono-junk-accuraterip
//! -- --ignored`. Offline CI should rely on `crc_algorithm.rs` instead.
//!
//! Source of truth — ARver's `tests/checksums_test.py`:
//! <https://github.com/arcctgx/ARver/blob/master/tests/checksums_test.py>.
//! Published CRC values and sample WAVs are redistributed solely for the
//! purpose of cross-verifying this crate's implementation against ARver.

#![cfg(test)]

use junk_libs_core::AnalysisError;
use junk_libs_disc::{PCM_SAMPLES_PER_SECTOR, PcmSector, RAW_SECTOR_SIZE, sector_to_samples};
use phono_junk_accuraterip::{TrackCrc, TrackPosition, track_crc_streaming};

const ARVER_COMMIT: &str = "master";
const SAMPLE_WAV_URL: &str =
    "https://raw.githubusercontent.com/arcctgx/ARver/master/tests/data/samples/sample.wav";
const SILENCE_WAV_URL: &str =
    "https://raw.githubusercontent.com/arcctgx/ARver/master/tests/data/samples/silence.wav";

// Expected CRCs from ARver's tests/checksums_test.py as of the reference
// commit. (v1, v2) tuples.
const EXPECTED_ONLY: (u32, u32) = (0xf43f_9174, 0x0ae7_c6f9);
const EXPECTED_FIRST: (u32, u32) = (0x9d6c_90ec, 0xb775_893e);
const EXPECTED_MIDDLE: (u32, u32) = (0x3c8d_d1d2, 0x56bb_a272);
const EXPECTED_LAST: (u32, u32) = (0x9360_d25a, 0xaa2d_e02d);
const EXPECTED_SILENCE: (u32, u32) = (0, 0);

fn fetch(url: &str) -> Vec<u8> {
    let resp = reqwest::blocking::get(url).unwrap_or_else(|e| panic!("fetch {}: {}", url, e));
    assert!(
        resp.status().is_success(),
        "GET {} → {}",
        url,
        resp.status()
    );
    resp.bytes()
        .unwrap_or_else(|e| panic!("read body {}: {}", url, e))
        .to_vec()
}

/// Parse a canonical CDDA-compatible WAV (PCM 16-bit stereo 44.1 kHz) and
/// return the PCM payload (interleaved L,R 16-bit little-endian bytes).
/// Skips the RIFF/fmt header and locates the `data` chunk.
fn wav_pcm_bytes(wav: &[u8]) -> Vec<u8> {
    assert_eq!(&wav[0..4], b"RIFF", "expected RIFF header");
    assert_eq!(&wav[8..12], b"WAVE", "expected WAVE format");

    // Walk chunks after the 12-byte RIFF header until we find `data`.
    let mut pos = 12usize;
    while pos + 8 <= wav.len() {
        let chunk_id = &wav[pos..pos + 4];
        let chunk_size =
            u32::from_le_bytes([wav[pos + 4], wav[pos + 5], wav[pos + 6], wav[pos + 7]]) as usize;
        let body_start = pos + 8;
        let body_end = body_start + chunk_size;
        if chunk_id == b"data" {
            return wav[body_start..body_end].to_vec();
        }
        // Chunks are word-aligned.
        pos = body_end + (body_end & 1);
    }
    panic!("no `data` chunk in WAV");
}

/// Chunk PCM bytes into 2352-byte CDDA sectors and yield `PcmSector`s.
/// Panics if the byte count isn't a multiple of 2352.
fn sectors_from_pcm(pcm: &[u8]) -> Vec<Result<PcmSector, AnalysisError>> {
    assert_eq!(
        pcm.len() % RAW_SECTOR_SIZE as usize,
        0,
        "PCM length {} is not a multiple of CDDA sector size {}",
        pcm.len(),
        RAW_SECTOR_SIZE
    );
    pcm.chunks_exact(RAW_SECTOR_SIZE as usize)
        .map(|sector_bytes| {
            let arr: [u8; RAW_SECTOR_SIZE as usize] = sector_bytes
                .try_into()
                .expect("chunks_exact yielded wrong size");
            Ok(sector_to_samples(&arr))
        })
        .collect()
}

fn run_crc(wav_bytes: &[u8], position: TrackPosition) -> TrackCrc {
    let pcm = wav_pcm_bytes(wav_bytes);
    let sectors = sectors_from_pcm(&pcm);
    let total_samples = (pcm.len() / 4) as u32;
    let expected_sector_samples = sectors.len() as u32 * PCM_SAMPLES_PER_SECTOR as u32;
    assert_eq!(
        total_samples, expected_sector_samples,
        "sample-count sanity"
    );
    track_crc_streaming(sectors, total_samples, position).expect("CRC streaming failed")
}

#[test]
#[ignore = "network: fetches WAV fixtures from github.com"]
fn sample_wav_only_track() {
    let wav = fetch(SAMPLE_WAV_URL);
    let got = run_crc(&wav, TrackPosition::Only);
    assert_eq!(
        (got.v1, got.v2),
        EXPECTED_ONLY,
        "ARver sample.wav @ position=Only (ARver ref {})",
        ARVER_COMMIT
    );
}

#[test]
#[ignore = "network: fetches WAV fixtures from github.com"]
fn sample_wav_first_track() {
    let wav = fetch(SAMPLE_WAV_URL);
    let got = run_crc(&wav, TrackPosition::First);
    assert_eq!((got.v1, got.v2), EXPECTED_FIRST);
}

#[test]
#[ignore = "network: fetches WAV fixtures from github.com"]
fn sample_wav_middle_track() {
    let wav = fetch(SAMPLE_WAV_URL);
    let got = run_crc(&wav, TrackPosition::Middle);
    assert_eq!((got.v1, got.v2), EXPECTED_MIDDLE);
}

#[test]
#[ignore = "network: fetches WAV fixtures from github.com"]
fn sample_wav_last_track() {
    let wav = fetch(SAMPLE_WAV_URL);
    let got = run_crc(&wav, TrackPosition::Last);
    assert_eq!((got.v1, got.v2), EXPECTED_LAST);
}

#[test]
#[ignore = "network: fetches WAV fixtures from github.com"]
fn silence_wav_is_zero_at_all_positions() {
    let wav = fetch(SILENCE_WAV_URL);
    for pos in [
        TrackPosition::Only,
        TrackPosition::First,
        TrackPosition::Middle,
        TrackPosition::Last,
    ] {
        let got = run_crc(&wav, pos);
        assert_eq!((got.v1, got.v2), EXPECTED_SILENCE, "silence at {:?}", pos);
    }
}
