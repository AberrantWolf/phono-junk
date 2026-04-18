//! Re-export of [`phono_junk_identify::http`] for CLI/GUI callers that
//! already depend on `phono-junk-lib`. Provider crates depend on
//! `phono-junk-identify` directly (phono-junk-lib depends on every provider,
//! so placing the client here would be circular).
//!
//! Extracted to `junk-libs` in the follow-up migration pass once
//! retro-junk-scraper is ready to consume it.

pub use phono_junk_identify::http::*;
