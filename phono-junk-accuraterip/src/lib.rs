//! AccurateRip v1/v2 CRC computation and (future) dBAR lookup.
//!
//! CRC v1/v2: single-pass streaming sum of stereo PCM samples with
//! first/last-track 2940-sample skips; see [`crc`].
//!
//! Lookup (Sprint 8, stub): dBAR fetch + parse from
//! `accuraterip.com/accuraterip/<a>/<b>/<c>/dBAR-<ntracks>-<discid1>-<discid2>-<cddbid>.bin`
//! → per-track submitter counts as confidence scores.

use std::path::Path;

use junk_libs_disc::{TrackLayout, TrackPcmReader};
use phono_junk_core::{AudioError, DiscIds, Toc};

pub mod crc;

pub use crc::{SKIP_SAMPLES, TrackCrc, TrackPosition, skip_bounds, track_crc_streaming};

/// Per-track verification status.
#[derive(Debug, Clone)]
pub struct TrackVerification {
    pub position: u8,
    pub v1_confidence: Option<u32>,
    pub v2_confidence: Option<u32>,
}

/// Compute AccurateRip CRC v1 and v2 for an audio track in a CUE/BIN image.
///
/// Opens the BIN, streams the track's PCM via [`TrackPcmReader::from_bin`],
/// and feeds it through [`track_crc_streaming`]. The caller supplies
/// `position` because it depends on the track's place in the whole disc,
/// which a single `TrackLayout` does not know.
pub fn track_crc_from_cue(
    bin_path: &Path,
    layout: &TrackLayout,
    position: TrackPosition,
) -> Result<TrackCrc, AudioError> {
    let reader = TrackPcmReader::from_bin(bin_path, layout)?;
    let total_samples = layout.length_sectors * 588;
    track_crc_streaming(reader, total_samples, position)
}

/// Compute AccurateRip CRC v1 and v2 for an audio track in a CHD image.
pub fn track_crc_from_chd(
    chd_path: &Path,
    layout: &TrackLayout,
    position: TrackPosition,
) -> Result<TrackCrc, AudioError> {
    let reader = TrackPcmReader::from_chd(chd_path, layout)?;
    let total_samples = layout.length_sectors * 588;
    track_crc_streaming(reader, total_samples, position)
}

/// Look up a disc in the AccurateRip database.
///
/// Sprint 8 scope: dBAR fetch + parse. Currently a stub.
pub fn lookup(_toc: &Toc, _ids: &DiscIds) -> Result<Vec<TrackVerification>, AudioError> {
    // TODO(sprint-8): fetch dBAR file, parse, return per-track confidences.
    Ok(Vec::new())
}
