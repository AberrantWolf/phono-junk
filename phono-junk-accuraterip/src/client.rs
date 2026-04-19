//! Thin HTTP fetcher for dBAR files.
//!
//! Wraps `phono_junk_identify::HttpClient` — the shared rate-limited
//! client — so accuraterip.com has its own per-host token bucket and
//! this crate doesn't re-implement HTTP plumbing. Parsing lives in
//! [`crate::dbar`]; this module is network glue only.

use governor::Quota;
use nonzero_ext::nonzero;
use phono_junk_core::DiscIds;
use phono_junk_identify::{HttpClient, HttpError};

use crate::dbar::DbarFile;
use crate::error::AccurateRipError;
use crate::url::{ACCURATERIP_HOST, dbar_url};

/// AccurateRip.com has no published rate limit, but the server hosts
/// millions of small static files and courtesy rates well below any
/// realistic concurrency ceiling are cheap. One req/sec per host
/// matches the MusicBrainz provider's pattern.
const DEFAULT_QUOTA: Quota = Quota::per_second(nonzero!(1u32));

pub struct AccurateRipClient {
    http: HttpClient,
}

impl AccurateRipClient {
    pub fn new(user_agent: impl Into<String>) -> Result<Self, HttpError> {
        let http = HttpClient::builder()
            .user_agent(user_agent)
            .host_quota(ACCURATERIP_HOST, DEFAULT_QUOTA)
            .build()?;
        Ok(Self { http })
    }

    /// Inject a preconfigured client, sharing rate-limit state across
    /// providers (see `PhonoContext::with_default_providers`). Also used
    /// by tests to point the fetcher at an httpmock server without touching
    /// the real internet.
    pub fn with_client(http: HttpClient) -> Self {
        Self { http }
    }

    /// Fetch and parse the dBAR for a disc. Returns `Ok(None)` when the
    /// server responds 404 (no submissions for this TOC triple — a normal
    /// state, not an error). Any other non-200 is mapped to
    /// [`AccurateRipError::Parse`] with the status for diagnostics.
    pub fn fetch_dbar(
        &self,
        ids: &DiscIds,
        track_count: u8,
    ) -> Result<Option<DbarFile>, AccurateRipError> {
        let url = dbar_url(ids, track_count)?;
        self.fetch_at_url(&url)
    }

    /// Fetch and parse a dBAR from a caller-supplied URL.
    ///
    /// Used internally by [`fetch_dbar`] after URL construction, and
    /// exposed so tests can drive the response-branch logic against a
    /// mock HTTP server (the real fetch_dbar always hits
    /// `www.accuraterip.com`).
    pub fn fetch_at_url(&self, url: &str) -> Result<Option<DbarFile>, AccurateRipError> {
        let resp = self.http.get(url)?;
        match resp.status {
            200 => Ok(Some(DbarFile::parse(&resp.body)?)),
            404 => Ok(None),
            code => Err(AccurateRipError::Parse(format!(
                "accuraterip.com returned HTTP {code}"
            ))),
        }
    }
}
