//! AccurateRip CRC computation and dBAR database lookup.
//!
//! Two independent halves:
//!
//! - **CRC v1/v2** in [`crc`] — stream PCM samples through
//!   [`track_crc_streaming`] (or the `track_crc_from_cue` / `track_crc_from_chd`
//!   conveniences below) to produce a [`TrackCrc`].
//! - **dBAR lookup** in [`client`] + [`dbar`] + [`verify`] —
//!   [`AccurateRipClient::fetch_dbar`] retrieves the binary response file
//!   for a disc's `(id1, id2, cddb)` triple, [`DbarFile::parse`] decodes it,
//!   and [`verify_disc`] / [`verify_track`] compare your computed CRCs to
//!   every submitter pressing in the file.
//!
//! Identification is *not* this crate's job — see `phono-junk-identify`
//! and its providers. AccurateRip answers "is this rip bit-perfect?",
//! not "what is this disc?". Combining the two is orchestrated by
//! `phono-junk-lib` once the aggregator (Sprint 11) lands.

use std::path::Path;

use junk_libs_disc::TrackPcmReader;
use phono_junk_core::AudioError;

pub mod client;
pub mod crc;
pub mod dbar;
pub mod error;
pub mod url;
pub mod verify;

pub use client::AccurateRipClient;
pub use crc::{SKIP_SAMPLES, TrackCrc, TrackPosition, skip_bounds, track_crc_streaming};
pub use dbar::{DbarFile, DbarResponse, ExpectedCrc};
pub use error::AccurateRipError;
pub use url::{ACCURATERIP_HOST, dbar_url};
pub use verify::{CrcMatch, TrackVerification, verify_disc, verify_track};

/// Compute AccurateRip CRC v1 and v2 for an audio track in a CUE image.
/// Handles both single-BIN whole-disc and multi-BIN per-track rips
/// transparently (see `TrackPcmReader::from_cue`).
pub fn track_crc_from_cue(
    cue_path: &Path,
    track_number: u8,
    position: TrackPosition,
) -> Result<TrackCrc, AudioError> {
    let reader = TrackPcmReader::from_cue(cue_path, track_number)?;
    let total_samples = reader.total_samples() as u32;
    track_crc_streaming(reader, total_samples, position)
}

/// Compute AccurateRip CRC v1 and v2 for an audio track in a CHD image.
pub fn track_crc_from_chd(
    chd_path: &Path,
    track_number: u8,
    position: TrackPosition,
) -> Result<TrackCrc, AudioError> {
    let reader = TrackPcmReader::from_chd(chd_path, track_number)?;
    let total_samples = reader.total_samples() as u32;
    track_crc_streaming(reader, total_samples, position)
}
