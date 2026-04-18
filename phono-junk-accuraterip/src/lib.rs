//! AccurateRip v1/v2 CRC computation and lookup.
//!
//! CRC v1/v2: offset-compensated streaming sum of stereo PCM samples.
//! Lookup: `accuraterip.com/accuraterip/<a>/<b>/<c>/dBAR-<ntracks>-<discid1>-<discid2>-<cddbid>.bin`
//! Returns per-track submitter counts as confidence scores.

use phono_junk_core::{AudioError, DiscIds, Toc};

/// Per-track verification status.
#[derive(Debug, Clone)]
pub struct TrackVerification {
    pub position: u8,
    pub v1_confidence: Option<u32>,
    pub v2_confidence: Option<u32>,
}

/// Compute AccurateRip CRC v1 and v2 for a track of stereo 16-bit PCM.
pub fn track_crc(_pcm: &[u8], _track_index: u8, _is_last: bool) -> (u32, u32) {
    // TODO: implement offset-compensated CRC v1/v2.
    (0, 0)
}

/// Look up a disc in the AccurateRip database.
pub fn lookup(_toc: &Toc, _ids: &DiscIds) -> Result<Vec<TrackVerification>, AudioError> {
    // TODO: fetch dBAR file, parse, return per-track confidences.
    Ok(Vec::new())
}
