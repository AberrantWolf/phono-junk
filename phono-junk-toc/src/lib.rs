//! TOC extraction and canonical DiscID computation.
//!
//! Reads a disc's Table of Contents from `.cue` or `.chd` (via
//! `junk-libs-disc`) and produces every ID that downstream providers need:
//!
//! - MusicBrainz DiscID (SHA-1 over formatted TOC → base64 with URL-safe chars)
//! - FreeDB/CDDB ID (8-hex-digit ID from offsets + length)
//! - AccurateRip discid1, discid2, cddb_id triple
//!
//! This crate is the single canonical implementation of each DiscID
//! algorithm. All providers consume [`DiscIds`] produced here — no
//! provider recomputes IDs.
//!
//! Generic disc-layout arithmetic (lead-in addition, single-vs-multi-BIN
//! CUE handling, CHD linear-sector translation) lives in
//! `junk-libs-disc::layout` and the adapter functions
//! `junk-libs-disc::{cue,chd}::{compute,read}_*_layout`. This crate only
//! owns the audio-CD-specific CD-Extra correction in `toc_from_layout`.

use std::path::Path;

use phono_junk_core::{AudioError, DiscIds, Toc};

pub mod discid;
mod toc_from_layout;

/// Extract the TOC from a CUE sheet path.
///
/// Resolves BIN file paths relative to the CUE's directory, handles
/// single-FILE and multi-FILE sheets, transparently normalises CDRWin
/// quirks, and applies the CD-Extra lead-out correction when a trailing
/// data track is present.
pub fn read_toc_from_cue(path: &Path) -> Result<Toc, AudioError> {
    let layout = junk_libs_disc::read_cue_layout(path)?;
    toc_from_layout::layout_to_toc(&layout)
}

/// Extract the TOC from a CHD file.
///
/// Opens the CHD, parses its track metadata, adds the lead-in, and applies
/// CD-Extra correction if the last track is a data track.
pub fn read_toc_from_chd(path: &Path) -> Result<Toc, AudioError> {
    let layout = junk_libs_disc::read_chd_layout(path)?;
    toc_from_layout::layout_to_toc(&layout)
}

/// Compute every canonical ID derivable from a [`Toc`].
///
/// Populates `mb_discid`, `cddb_id`, `ar_discid1`, and `ar_discid2`.
/// `barcode` and `catalog_number` stay `None` — those are provider-supplied.
pub fn compute_disc_ids(toc: &Toc) -> DiscIds {
    let (ar1, ar2, cddb) = discid::accuraterip_ids(toc);
    DiscIds {
        mb_discid: Some(discid::musicbrainz_discid(toc)),
        cddb_id: Some(cddb),
        ar_discid1: Some(ar1),
        ar_discid2: Some(ar2),
        barcode: None,
        catalog_number: None,
    }
}
