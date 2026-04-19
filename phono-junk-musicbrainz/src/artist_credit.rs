//! Format a MusicBrainz `artist-credit` array into a display string.
//!
//! MB splits multi-artist credits into entries with a `name` plus an optional
//! `joinphrase` (e.g. `" & "`, `" feat. "`, `" vs "`). Concatenating name +
//! joinphrase across entries reproduces the rendered credit exactly — no
//! special-case handling, just what MB tells us.

use crate::json::ArtistCredit;

pub(crate) fn format(credits: &[ArtistCredit]) -> String {
    let mut out = String::new();
    for c in credits {
        out.push_str(&c.name);
        if let Some(jp) = &c.joinphrase {
            out.push_str(jp);
        }
    }
    out
}

#[cfg(test)]
#[path = "tests/artist_credit_tests.rs"]
mod tests;
