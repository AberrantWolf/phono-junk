//! Canonical DiscID algorithms.
//!
//! - [`musicbrainz_discid`] — SHA-1 over formatted TOC string, base64 with
//!   `+/=` replaced by `.-_` (MB's URL-safe variant).
//! - [`cddb_discid`] — 8-hex-digit FreeDB/CDDB ID from track offsets + length.
//! - [`accuraterip_ids`] — the discid1/discid2/cddb triple used by dBAR lookup.

use phono_junk_core::Toc;

/// Compute the MusicBrainz DiscID for a [`Toc`].
pub fn musicbrainz_discid(_toc: &Toc) -> String {
    // TODO: implement and test against canonical MB DiscID fixtures.
    String::new()
}

/// Compute the FreeDB/CDDB 8-hex-digit disc ID.
pub fn cddb_discid(_toc: &Toc) -> String {
    // TODO: implement.
    String::new()
}

/// Compute the AccurateRip disc ID triple (discid1, discid2, cddb_id).
pub fn accuraterip_ids(_toc: &Toc) -> (String, String, String) {
    // TODO: implement.
    (String::new(), String::new(), String::new())
}
