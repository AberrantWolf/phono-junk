//! Core types for phono-junk.
//!
//! No I/O. Types only — `Toc`, `DiscIds`, `AlbumIdentification`, `AudioError`,
//! and the identification confidence/source enums consumed by every other
//! crate in the workspace.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use junk_libs_core::ReadSeek;

/// Errors produced anywhere in phono-junk's analysis and I/O layers.
#[derive(Debug, Error)]
pub enum AudioError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Junk(#[from] junk_libs_core::AnalysisError),

    #[error("Invalid TOC: {0}")]
    InvalidToc(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Unsupported: {0}")]
    Unsupported(String),

    #[error("{0}")]
    Other(String),
}

/// A CD's Table of Contents: the per-track offset layout that every
/// identification ID is derived from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toc {
    /// First track number on the disc (usually 1).
    pub first_track: u8,
    /// Last track number on the disc.
    pub last_track: u8,
    /// Lead-out offset in sectors (start of the gap after the last track).
    pub leadout_sector: u32,
    /// Per-track start offsets in sectors, indexed by track number.
    /// `track_offsets[n]` is the start of track `first_track + n`.
    pub track_offsets: Vec<u32>,
}

/// One row's worth of "which track starts where, and how long is it" —
/// the primitive shared between TOC rendering and provider-less track stubs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackSpan {
    pub position: u8,
    pub start_sector: u32,
    pub length_frames: u64,
}

impl Toc {
    /// Number of tracks recorded in this TOC.
    pub fn track_count(&self) -> usize {
        self.track_offsets.len()
    }

    /// Length of the `idx`-th track in CD frames (75 per second), derived
    /// from the next track's start offset or the leadout for the final
    /// track. Returns `None` when `idx` is out of range.
    pub fn track_length_frames(&self, idx: usize) -> Option<u64> {
        let start = *self.track_offsets.get(idx)?;
        let next = self
            .track_offsets
            .get(idx + 1)
            .copied()
            .unwrap_or(self.leadout_sector);
        Some(next.saturating_sub(start) as u64)
    }

    /// Total disc length in CD frames — leadout minus first track start.
    pub fn total_length_frames(&self) -> u64 {
        self.track_offsets
            .first()
            .map(|first| self.leadout_sector.saturating_sub(*first) as u64)
            .unwrap_or(0)
    }

    /// Iterate per-track spans in TOC order, yielding position (1-indexed
    /// from [`Toc::first_track`]), start sector, and length in CD frames.
    pub fn iter_track_spans(&self) -> impl Iterator<Item = TrackSpan> + '_ {
        self.track_offsets.iter().enumerate().map(|(i, &start)| {
            let length_frames = self
                .track_offsets
                .get(i + 1)
                .copied()
                .unwrap_or(self.leadout_sector)
                .saturating_sub(start) as u64;
            TrackSpan {
                position: self.first_track + i as u8,
                start_sector: start,
                length_frames,
            }
        })
    }
}

#[cfg(test)]
#[path = "tests/toc_tests.rs"]
mod toc_tests;

/// All externally-resolvable identifiers derived from a disc's TOC and metadata.
///
/// Different providers key on different IDs: MusicBrainz uses `mb_discid`,
/// AccurateRip uses the triple, Discogs uses `barcode`/`catalog_number`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscIds {
    pub mb_discid: Option<String>,
    pub cddb_id: Option<String>,
    pub ar_discid1: Option<String>,
    pub ar_discid2: Option<String>,
    pub barcode: Option<String>,
    pub catalog_number: Option<String>,
}

/// Confidence in an identification result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IdentificationConfidence {
    /// Exact match on a canonical ID (DiscID, barcode).
    Certain,
    /// Match with some fuzzy component (text search, best-of candidates).
    Likely,
    /// User manually tagged — treat as authoritative but note the source.
    Manual,
    /// No match found; TOC preserved for later retry.
    Unidentified,
}

/// Where a rip file sits in the scan/identify lifecycle.
///
/// Distinct from [`IdentificationConfidence`]: confidence answers "how
/// trustworthy is the match?", state answers "has identification run yet?".
/// `Unscanned` is only seen transiently during ingest; persisted rows are
/// always one of the other four.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum IdentificationState {
    /// Row exists but no identify attempt has been run. Used briefly during
    /// metadata-only ingest before the queue picks the row up.
    #[default]
    Unscanned,
    /// Sitting in the identify queue, waiting for a worker.
    Queued,
    /// An identify worker is currently running providers for this rip.
    Working,
    /// Identification succeeded — `disc_id` is set.
    Identified,
    /// Providers ran but none returned a match. Distinct from `Queued`:
    /// "tried and failed to match" vs "hasn't tried yet".
    Unidentified,
    /// Identify attempt aborted with a hard error (bad TOC, DB error, not
    /// a provider no-match). Retrying clears the state back to `Queued`.
    Failed,
}

impl IdentificationState {
    pub fn as_str(self) -> &'static str {
        match self {
            IdentificationState::Unscanned => "unscanned",
            IdentificationState::Queued => "queued",
            IdentificationState::Working => "working",
            IdentificationState::Identified => "identified",
            IdentificationState::Unidentified => "unidentified",
            IdentificationState::Failed => "failed",
        }
    }

    pub fn from_str_db(s: &str) -> Option<Self> {
        Some(match s {
            "unscanned" => IdentificationState::Unscanned,
            "queued" => IdentificationState::Queued,
            "working" => IdentificationState::Working,
            "identified" => IdentificationState::Identified,
            "unidentified" => IdentificationState::Unidentified,
            "failed" => IdentificationState::Failed,
            _ => return None,
        })
    }
}

/// Where an identification came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IdentificationSource {
    MusicBrainz,
    Discogs,
    ITunes,
    Amazon,
    Tower,
    UserTagged,
    Import,
    /// A redumper sidecar (log or CD-TEXT) read off the local filesystem.
    /// Used for physical-disc facts like MCN and per-track ISRCs mirrored
    /// out of the rip's `.log` / `.cdtext`.
    Redumper,
    /// Another provider, named by the provider's `name()`.
    Other(String),
}

/// Builder-style identification output — the audio analog of retro-junk's
/// `RomIdentification`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlbumIdentification {
    pub album_title: Option<String>,
    pub album_artist: Option<String>,
    pub year: Option<u16>,
    pub mbid: Option<String>,
    pub confidence: Option<IdentificationConfidence>,
    pub sources: Vec<IdentificationSource>,
    pub tracks: Vec<TrackIdentification>,
}

/// Per-track metadata that may or may not be populated.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrackIdentification {
    pub position: u8,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub length_frames: Option<u64>,
    pub isrc: Option<String>,
    pub mbid: Option<String>,
}

impl AlbumIdentification {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.album_title = Some(title.into());
        self
    }

    pub fn with_artist(mut self, artist: impl Into<String>) -> Self {
        self.album_artist = Some(artist.into());
        self
    }

    pub fn with_year(mut self, year: u16) -> Self {
        self.year = Some(year);
        self
    }

    pub fn with_mbid(mut self, mbid: impl Into<String>) -> Self {
        self.mbid = Some(mbid.into());
        self
    }

    pub fn with_confidence(mut self, confidence: IdentificationConfidence) -> Self {
        self.confidence = Some(confidence);
        self
    }

    pub fn with_source(mut self, source: IdentificationSource) -> Self {
        self.sources.push(source);
        self
    }
}
