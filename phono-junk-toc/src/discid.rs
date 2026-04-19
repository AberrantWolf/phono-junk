//! Canonical DiscID algorithms.
//!
//! Every audio CD's TOC yields three externally-resolvable identifiers,
//! each used by a different database. All are derived from the same inputs:
//! first/last track numbers, per-track sector offsets, and the lead-out
//! sector. See `.claude/skills/phono-archive/formats/DiscID.md` for the full
//! spec and upstream sources.
//!
//! - [`musicbrainz_discid`] — SHA-1 over formatted TOC string, base64 with
//!   MusicBrainz's URL-safe substitutions (`+/=` → `.-_`). Spec:
//!   <https://musicbrainz.org/doc/Disc_ID_Calculation>.
//! - [`cddb_discid`] — 8-hex-digit FreeDB/CDDB ID. Matches
//!   `libdiscid::discid_get_freedb_id()`. Reference:
//!   <https://github.com/metabrainz/libdiscid>.
//! - [`accuraterip_ids`] — the `(id1, id2, cddb)` triple used by dBAR
//!   lookup. Formulas cross-verified against ARver:
//!   <https://github.com/arcctgx/ARver>.

use junk_libs_disc::LEAD_IN_FRAMES;
use phono_junk_core::Toc;
use sha1::{Digest, Sha1};

/// Convert an absolute sector offset to an LSN (Logical Sector Number —
/// sectors relative to the start of audio data). Used by AccurateRip's
/// id1 / id2 formulas. FreeDB does NOT use LSN; it uses the raw absolute
/// offset via `offset / 75`.
fn lsn(sector: u32) -> u32 {
    sector.saturating_sub(LEAD_IN_FRAMES)
}

/// Compute the MusicBrainz DiscID for a [`Toc`].
///
/// Algorithm: concatenate uppercase-hex `first_track` (2 chars), `last_track`
/// (2 chars), `leadout_sector` (8 chars), and 99 track offsets (8 chars each,
/// zero-padded for tracks that don't exist). SHA-1 the 804-char ASCII string,
/// then URL-safe base64 the 20-byte digest with `+/=` → `.-_`.
pub fn musicbrainz_discid(toc: &Toc) -> String {
    use base64::Engine;

    let mut s = String::with_capacity(804);
    s.push_str(&format!("{:02X}", toc.first_track));
    s.push_str(&format!("{:02X}", toc.last_track));
    s.push_str(&format!("{:08X}", toc.leadout_sector));
    for i in 1..=99u8 {
        let idx = i.wrapping_sub(toc.first_track) as usize;
        let offset = if i >= toc.first_track && i <= toc.last_track && idx < toc.track_offsets.len()
        {
            toc.track_offsets[idx]
        } else {
            0
        };
        s.push_str(&format!("{:08X}", offset));
    }

    let digest = Sha1::new().chain_update(s.as_bytes()).finalize();
    let b64 = base64::engine::general_purpose::STANDARD.encode(digest);
    b64.replace('+', ".").replace('/', "_").replace('=', "-")
}

/// Sum of the decimal digits of `n`, single-pass (not iterative).
/// Matches `cddb_sum()` in `cd-discid` and libdiscid's internal helper.
fn digit_sum(mut n: u32) -> u32 {
    let mut sum = 0;
    while n > 0 {
        sum += n % 10;
        n /= 10;
    }
    sum
}

/// Compute the FreeDB/CDDB 8-hex-digit disc ID.
///
/// `cddb_id = ((N mod 0xFF) << 24) | (T << 8) | num_tracks`, where `N` is
/// the sum of `digit_sum(offset / 75)` over every audio track (absolute MSF
/// seconds — **lead-in is included**, i.e. the divisor is the raw sector
/// offset, not the LSN), and `T = (leadout - first_track_offset) / 75`.
///
/// Cross-verified against libdiscid's `discid_get_freedb_id()` and the
/// ARver test suite. Output is lowercase 8-hex to match libdiscid.
pub fn cddb_discid(toc: &Toc) -> String {
    let mut n = 0u32;
    for offset in &toc.track_offsets {
        let seconds = offset / 75;
        n = n.wrapping_add(digit_sum(seconds));
    }
    let first_offset = toc.track_offsets.first().copied().unwrap_or(0);
    let t = (toc.leadout_sector - first_offset) / 75;
    let num_tracks = u32::from(toc.last_track - toc.first_track + 1);
    let id = ((n % 0xFF) << 24) | (t << 8) | (num_tracks & 0xFF);
    format!("{:08x}", id)
}

/// Compute the AccurateRip disc ID triple `(id1, id2, cddb_id)`.
///
/// All three are lowercase 8-hex-digit strings ready for dBAR URL
/// construction: `dBAR-<NNN>-<id1>-<id2>-<cddb_id>.bin`.
///
/// Formulas:
/// - `id1 = (sum(lsn_offsets) + lsn_leadout) & 0xFFFFFFFF`
/// - `id2 = sum((lsn_or_1) * track_number) + lsn_leadout * (num_tracks + 1)`,
///   where `lsn_or_1` is the LSN offset or `1` if that offset is `0` (keeps
///   track 1's contribution non-zero when it starts exactly at LSN 0).
///
/// Cross-verified against ARver (`arver/disc/fingerprint.py`).
pub fn accuraterip_ids(toc: &Toc) -> (String, String, String) {
    let lsn_leadout = lsn(toc.leadout_sector);

    let mut id1 = lsn_leadout;
    let mut id2 = lsn_leadout.wrapping_mul(u32::from(toc.last_track - toc.first_track + 1) + 1);
    for (i, &offset) in toc.track_offsets.iter().enumerate() {
        let l = lsn(offset);
        id1 = id1.wrapping_add(l);
        let l_or_1 = if l == 0 { 1 } else { l };
        let track_num = u32::from(toc.first_track) + i as u32;
        id2 = id2.wrapping_add(l_or_1.wrapping_mul(track_num));
    }

    (
        format!("{:08x}", id1),
        format!("{:08x}", id2),
        cddb_discid(toc),
    )
}

#[cfg(test)]
#[path = "tests/discid_tests.rs"]
mod tests;
