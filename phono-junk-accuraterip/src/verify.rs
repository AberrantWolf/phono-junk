//! Compare computed [`TrackCrc`] values against a parsed [`DbarFile`].
//!
//! The verification loop is deliberately stateless: given a dBAR and one
//! or more computed CRCs, it reports every matching pressing for v1 and
//! v2 independently. Callers (CLI `verify`, library cache writer) decide
//! how to present the outcome.

use crate::crc::TrackCrc;
use crate::dbar::{DbarFile, ExpectedCrc};

/// One hit: the pressing index within the `DbarFile` and its submitter count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrcMatch {
    pub pressing: usize,
    pub confidence: u8,
}

/// Outcome of checking one track's computed CRC against a dBAR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackVerification {
    pub position: u8,
    pub computed: TrackCrc,
    pub v1_matches: Vec<CrcMatch>,
    pub v2_matches: Vec<CrcMatch>,
}

impl TrackVerification {
    /// Highest submitter count across all matches (either version).
    pub fn best_confidence(&self) -> Option<u8> {
        self.v1_matches
            .iter()
            .chain(self.v2_matches.iter())
            .map(|m| m.confidence)
            .max()
    }

    pub fn is_verified(&self) -> bool {
        !self.v1_matches.is_empty() || !self.v2_matches.is_empty()
    }

    /// Short human-readable summary suitable for persisting in
    /// `RipFile.accuraterip_status` or printing in CLI output. The
    /// format is stable and grep-friendly:
    ///
    /// - `"v2 confidence 8"` — v2 matched, best confidence 8
    /// - `"v1 confidence 3 (v2 no match)"` — only v1 matched
    /// - `"no match"` — dBAR loaded, neither version matched
    pub fn status_string(&self) -> String {
        let v1_best = self.v1_matches.iter().map(|m| m.confidence).max();
        let v2_best = self.v2_matches.iter().map(|m| m.confidence).max();
        match (v1_best, v2_best) {
            (_, Some(c)) => format!("v2 confidence {c}"),
            (Some(c), None) => format!("v1 confidence {c} (v2 no match)"),
            (None, None) => "no match".to_string(),
        }
    }
}

/// Check one track's computed CRC against every pressing in a dBAR.
///
/// `v2 == 0` entries are treated as legacy "no v2 submitted" sentinels
/// and never match — otherwise every all-zero CRC would spuriously
/// match silent or zero-init bugs.
pub fn verify_track(dbar: &DbarFile, position: u8, computed: TrackCrc) -> TrackVerification {
    let mut v1_matches = Vec::new();
    let mut v2_matches = Vec::new();
    for (pressing, entry) in dbar.entries_for_track(position) {
        if matches_v1(entry, computed.v1) {
            v1_matches.push(CrcMatch {
                pressing,
                confidence: entry.confidence,
            });
        }
        if matches_v2(entry, computed.v2) {
            v2_matches.push(CrcMatch {
                pressing,
                confidence: entry.confidence,
            });
        }
    }
    TrackVerification {
        position,
        computed,
        v1_matches,
        v2_matches,
    }
}

/// Verify every computed track CRC on a disc against the dBAR.
/// `crcs` is a slice of `(position, crc)` tuples in any order.
pub fn verify_disc(dbar: &DbarFile, crcs: &[(u8, TrackCrc)]) -> Vec<TrackVerification> {
    crcs.iter()
        .map(|&(pos, crc)| verify_track(dbar, pos, crc))
        .collect()
}

fn matches_v1(entry: &ExpectedCrc, computed: u32) -> bool {
    // v1 is always populated — even 0 is a real value (rare in practice).
    entry.v1 == computed
}

fn matches_v2(entry: &ExpectedCrc, computed: u32) -> bool {
    // v2 == 0 is the legacy "no v2 submitted" sentinel. Treat it as
    // non-matching regardless of the computed value — otherwise every
    // computed CRC would trivially match legacy entries with matching v1.
    entry.v2 != 0 && entry.v2 == computed
}
