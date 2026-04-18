//! TOC extraction and canonical DiscID computation.
//!
//! Reads a disc's Table of Contents from `.cue` or `.chd` (via `junk-libs-disc`)
//! and produces every ID that downstream providers need:
//!
//! - MusicBrainz DiscID (SHA-1 over formatted TOC → base64 with URL-safe chars)
//! - FreeDB/CDDB ID (8-hex-digit ID from offsets + length)
//! - AccurateRip discid1, discid2, cddb_id triple
//!
//! This crate is the single canonical implementation of each algorithm. All
//! providers consume [`DiscIds`] produced here — no provider recomputes IDs.

use phono_junk_core::{AudioError, DiscIds, Toc};

pub mod discid;

/// Extract the TOC from a CUE sheet path.
///
/// Returns a fully-populated [`Toc`] with track offsets in sectors.
pub fn read_toc_from_cue(_path: &std::path::Path) -> Result<Toc, AudioError> {
    // TODO: implement using junk_libs_disc::cue
    Err(AudioError::Unsupported(
        "read_toc_from_cue not implemented".into(),
    ))
}

/// Extract the TOC from a CHD file.
pub fn read_toc_from_chd(_path: &std::path::Path) -> Result<Toc, AudioError> {
    // TODO: implement using junk_libs_disc::chd
    Err(AudioError::Unsupported(
        "read_toc_from_chd not implemented".into(),
    ))
}

/// Compute every canonical ID derivable from a [`Toc`].
pub fn compute_disc_ids(_toc: &Toc) -> DiscIds {
    // TODO: implement. Placeholder returns empty IDs.
    DiscIds::default()
}
