//! Binary parser for AccurateRip's dBAR response files.
//!
//! A dBAR `.bin` is a concatenation of one or more "Responses", each
//! representing a single submitter's rip of a disc whose TOC happened to
//! hash to the same triple `(id1, id2, cddb)`. Wire format, all
//! little-endian:
//!
//! ```text
//! Response {
//!     u8   track_count
//!     u32  ar_id1
//!     u32  ar_id2
//!     u32  cddb_id
//!     TrackEntry[track_count] {
//!         u8   confidence
//!         u32  crc_v1
//!         u32  crc_v2        // 0 for legacy submissions (pre-v2)
//!     }
//! }
//! ```
//!
//! Format documented in `.claude/skills/phono-archive/formats/AccurateRip.md`
//! and mirrors ARver's
//! [`arver/disc/database.py`](https://github.com/arcctgx/ARver/blob/master/arver/disc/database.py)
//! response parser.

use crate::error::AccurateRipError;

/// Header size in bytes: `u8 + 3 * u32`.
pub const HEADER_LEN: usize = 1 + 4 + 4 + 4;
/// Track entry size in bytes: `u8 + u32 + u32`.
pub const ENTRY_LEN: usize = 1 + 4 + 4;

/// One expected-CRC entry: a submitter's v1/v2 pair with its agreement count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpectedCrc {
    /// Number of submitters whose rips produced this pair. Saturates
    /// around 200+; interpret per the AccurateRip.md rubric.
    pub confidence: u8,
    pub v1: u32,
    /// v2 is 0 for pre-v2 submissions — treat "0" as "no v2 value" when
    /// matching, not as a real checksum of silence.
    pub v2: u32,
}

/// One Response block — a single pressing's worth of expected CRCs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbarResponse {
    pub track_count: u8,
    pub ar_id1: u32,
    pub ar_id2: u32,
    pub cddb_id: u32,
    /// Per-track entries. `tracks.len() == track_count as usize`.
    pub tracks: Vec<ExpectedCrc>,
}

/// A parsed dBAR file — all Responses stacked in submission order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DbarFile {
    pub responses: Vec<DbarResponse>,
}

impl DbarFile {
    /// Parse the raw `.bin` bytes into its Responses. The file contains
    /// no length prefix; parsing runs to end-of-buffer and errors on any
    /// truncation.
    pub fn parse(bytes: &[u8]) -> Result<Self, AccurateRipError> {
        let mut responses = Vec::new();
        let mut cur = 0usize;

        while cur < bytes.len() {
            if bytes.len() - cur < HEADER_LEN {
                return Err(AccurateRipError::Parse(format!(
                    "truncated header at offset {cur}: {} bytes remaining, need {HEADER_LEN}",
                    bytes.len() - cur
                )));
            }
            let track_count = bytes[cur];
            let ar_id1 = read_u32_le(&bytes[cur + 1..cur + 5]);
            let ar_id2 = read_u32_le(&bytes[cur + 5..cur + 9]);
            let cddb_id = read_u32_le(&bytes[cur + 9..cur + 13]);
            cur += HEADER_LEN;

            let tc = track_count as usize;
            let need = tc * ENTRY_LEN;
            if bytes.len() - cur < need {
                return Err(AccurateRipError::Parse(format!(
                    "truncated entries at offset {cur}: {} bytes remaining, need {need} for {tc} tracks",
                    bytes.len() - cur
                )));
            }
            let mut tracks = Vec::with_capacity(tc);
            for _ in 0..tc {
                let confidence = bytes[cur];
                let v1 = read_u32_le(&bytes[cur + 1..cur + 5]);
                let v2 = read_u32_le(&bytes[cur + 5..cur + 9]);
                cur += ENTRY_LEN;
                tracks.push(ExpectedCrc { confidence, v1, v2 });
            }

            responses.push(DbarResponse {
                track_count,
                ar_id1,
                ar_id2,
                cddb_id,
                tracks,
            });
        }

        Ok(DbarFile { responses })
    }

    /// Iterate every expected CRC for a given 1-indexed track position,
    /// across all pressings in the file. Yields `(pressing_index, entry)`
    /// pairs. Positions beyond a pressing's `track_count` are skipped
    /// rather than erroring — heterogeneous track counts within one file
    /// are rare but not illegal.
    pub fn entries_for_track(
        &self,
        position: u8,
    ) -> impl Iterator<Item = (usize, &ExpectedCrc)> + '_ {
        self.responses.iter().enumerate().filter_map(move |(i, r)| {
            if position == 0 || position > r.track_count {
                None
            } else {
                Some((i, &r.tracks[(position - 1) as usize]))
            }
        })
    }
}

fn read_u32_le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
