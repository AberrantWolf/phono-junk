//! dBAR URL construction.
//!
//! Format, per AccurateRip.md and ARver's
//! [`arver/disc/database.py`](https://github.com/arcctgx/ARver/blob/master/arver/disc/database.py):
//!
//! ```text
//! http://www.accuraterip.com/accuraterip/<id1[-1]>/<id1[-2]>/<id1[-3]>/dBAR-<NNN>-<id1>-<id2>-<cddb>.bin
//! ```
//!
//! `id1`, `id2`, and `cddb` are the lowercase 8-char hex strings produced by
//! `phono_junk_toc::compute_disc_ids`. `<NNN>` is the zero-padded 3-digit
//! track count.

use phono_junk_core::DiscIds;

use crate::error::AccurateRipError;

pub const ACCURATERIP_HOST: &str = "www.accuraterip.com";

/// Build the dBAR lookup URL for a disc. Returns [`AccurateRipError::MissingId`]
/// when any of the three required IDs is absent from `ids`.
pub fn dbar_url(ids: &DiscIds, track_count: u8) -> Result<String, AccurateRipError> {
    let id1 = ids
        .ar_discid1
        .as_deref()
        .ok_or(AccurateRipError::MissingId("ar_discid1"))?;
    let id2 = ids
        .ar_discid2
        .as_deref()
        .ok_or(AccurateRipError::MissingId("ar_discid2"))?;
    let cddb = ids
        .cddb_id
        .as_deref()
        .ok_or(AccurateRipError::MissingId("cddb_id"))?;

    // ARver takes id1 chars from the right: id1[-1], id1[-2], id1[-3].
    // We compute on the raw string so upstream formatting governs casing.
    let chars: Vec<char> = id1.chars().collect();
    let n = chars.len();
    if n < 3 {
        return Err(AccurateRipError::Parse(format!(
            "ar_discid1 too short for URL construction: {id1:?}"
        )));
    }
    let a = chars[n - 1];
    let b = chars[n - 2];
    let c = chars[n - 3];

    Ok(format!(
        "http://{ACCURATERIP_HOST}/accuraterip/{a}/{b}/{c}/dBAR-{track_count:03}-{id1}-{id2}-{cddb}.bin",
    ))
}
