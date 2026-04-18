//! Rate-limited, retrying HTTP client shared by all providers.
//!
//! Per-host rate limits honor each service's published quotas:
//! - musicbrainz.org: 1 req/sec
//! - api.discogs.com: 60 req/min authenticated, 25 req/min anonymous
//! - itunes.apple.com: ~20 req/min soft
//!
//! Extracted to `junk-libs` in the follow-up migration pass once retro-junk
//! is ready to consume it.

// TODO: implement rate-limited client wrapper around reqwest::blocking::Client
