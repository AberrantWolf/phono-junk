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

#[derive(Debug)]
pub struct FetchedDbar {
    pub dbar: DbarFile,
    pub body: Vec<u8>,
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
        Ok(self
            .fetch_dbar_evidence(ids, track_count)?
            .map(|fetched| fetched.dbar))
    }

    /// Fetch a validated dBAR together with its immutable raw body for the
    /// catalog evidence store.
    pub fn fetch_dbar_evidence(
        &self,
        ids: &DiscIds,
        track_count: u8,
    ) -> Result<Option<FetchedDbar>, AccurateRipError> {
        let url = dbar_url(ids, track_count)?;
        let result = self.fetch_evidence_at_url(&url)?;
        let Some(fetched) = result else {
            return Ok(None);
        };
        fetched.dbar.validate_request(
            track_count,
            parse_id(ids.ar_discid1.as_deref(), "ar_discid1")?,
            parse_id(ids.ar_discid2.as_deref(), "ar_discid2")?,
            parse_id(ids.cddb_id.as_deref(), "cddb_id")?,
        )?;
        Ok(Some(fetched))
    }

    /// Fetch and parse a dBAR from a caller-supplied URL.
    ///
    /// Used internally by [`fetch_dbar`] after URL construction, and
    /// exposed so tests can drive the response-branch logic against a
    /// mock HTTP server (the real fetch_dbar always hits
    /// `www.accuraterip.com`).
    pub fn fetch_at_url(&self, url: &url::Url) -> Result<Option<DbarFile>, AccurateRipError> {
        Ok(self.fetch_evidence_at_url(url)?.map(|fetched| fetched.dbar))
    }

    fn fetch_evidence_at_url(
        &self,
        url: &url::Url,
    ) -> Result<Option<FetchedDbar>, AccurateRipError> {
        let resp = self.http.get(url)?;
        match resp.status {
            200 => Ok(Some(FetchedDbar {
                dbar: DbarFile::parse(&resp.body)?,
                body: resp.body,
            })),
            404 => Ok(None),
            code => Err(AccurateRipError::Parse(format!(
                "accuraterip.com returned HTTP {code}"
            ))),
        }
    }
}

fn parse_id(value: Option<&str>, name: &'static str) -> Result<u32, AccurateRipError> {
    let value = value.ok_or(AccurateRipError::MissingId(name))?;
    u32::from_str_radix(value, 16)
        .map_err(|_| AccurateRipError::Parse(format!("invalid hexadecimal {name}: {value}")))
}
