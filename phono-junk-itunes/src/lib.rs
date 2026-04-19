//! iTunes Search API — asset-only (album art) provider.
//!
//! Unauthenticated. Text search on `"<artist> <album>"`; each result carries
//! a `artworkUrl100` pointing at a 100-pixel JPEG. We rewrite the size token
//! from `100x100bb.jpg` to `1000x1000bb.jpg` to land a hi-res image.
//!
//! Fuzzy confidence — the hit is text-matched, not authoritative, so UX
//! should always present results as candidates rather than auto-selecting.
//! See TODO.md for the deferred scoring pass.
//!
//! Rate limit: Apple doesn't publish a firm number; CLAUDE.md's "~20 req/min
//! soft" is our self-imposed ceiling.

use governor::Quota;
use nonzero_ext::nonzero;
use phono_junk_identify::{
    AssetCandidate, AssetConfidence, AssetLookupCtx, AssetProvider, AssetType, HttpClient,
    HttpError, ProviderError,
};
use url::Url;

mod json;

const PROVIDER: &str = "itunes";
const SEARCH_ENDPOINT: &str = "https://itunes.apple.com/search";

pub struct ITunesProvider {
    http: HttpClient,
}

impl ITunesProvider {
    pub fn new(user_agent: impl Into<String>) -> Result<Self, HttpError> {
        let http = HttpClient::builder()
            .user_agent(user_agent)
            .host_quota("itunes.apple.com", Quota::per_minute(nonzero!(20u32)))
            .build()?;
        Ok(Self { http })
    }

    #[doc(hidden)]
    pub fn with_http_client(http: HttpClient) -> Self {
        Self { http }
    }
}

impl AssetProvider for ITunesProvider {
    fn name(&self) -> &'static str {
        PROVIDER
    }

    fn asset_types(&self) -> &[AssetType] {
        &[AssetType::FrontCover]
    }

    fn lookup_art(&self, ctx: &AssetLookupCtx<'_>) -> Result<Vec<AssetCandidate>, ProviderError> {
        // Text search needs album title + artist. The aggregator will usually
        // have populated these from an earlier MB hit; without both, skip.
        let Some(album) = ctx.album else {
            return Ok(Vec::new());
        };
        let (Some(title), Some(artist)) = (album.title.as_deref(), album.artist_credit.as_deref())
        else {
            return Ok(Vec::new());
        };

        let term = format!("{artist} {title}");
        let url = build_search_url(&term)?;
        let resp = self.http.get(url.as_str()).map_err(map_http_err)?;
        match resp.status {
            200 => parse_search_response(&resp.body),
            404 => Ok(Vec::new()),
            code => Err(ProviderError::Other(format!("itunes returned HTTP {code}"))),
        }
    }
}

fn build_search_url(term: &str) -> Result<Url, ProviderError> {
    let mut url = Url::parse(SEARCH_ENDPOINT)
        .map_err(|e| ProviderError::Other(format!("itunes url: {e}")))?;
    url.query_pairs_mut()
        .append_pair("term", term)
        .append_pair("entity", "album")
        .append_pair("limit", "5");
    Ok(url)
}

/// Parse an iTunes Search API response body into asset candidates. Rewrites
/// each hit's 100px artwork URL to 1000px so downloads land the high-res
/// asset rather than the thumbnail.
pub fn parse_search_response(bytes: &[u8]) -> Result<Vec<AssetCandidate>, ProviderError> {
    let resp: json::SearchResponse =
        serde_json::from_slice(bytes).map_err(|e| ProviderError::Parse(format!("itunes: {e}")))?;
    let mut out = Vec::with_capacity(resp.results.len());
    for hit in resp.results {
        let Some(art_url) = hit.artwork_url100 else {
            continue;
        };
        let rewritten = rewrite_artwork_size(&art_url);
        let source_url = match Url::parse(&rewritten) {
            Ok(u) => u,
            Err(e) => {
                log::warn!("itunes: skipping hit with invalid artwork URL {rewritten}: {e}");
                continue;
            }
        };
        out.push(AssetCandidate {
            provider: PROVIDER.to_string(),
            asset_type: AssetType::FrontCover,
            source_url,
            width: Some(1000),
            height: Some(1000),
            confidence: AssetConfidence::Fuzzy,
        });
    }
    Ok(out)
}

/// Rewrite iTunes' canonical `/source/100x100bb.jpg` URL segment to
/// `/source/1000x1000bb.jpg`. Covers jpg and png variants; other sizes pass
/// through unchanged. See TODO.md for the regex generalisation.
pub fn rewrite_artwork_size(url: &str) -> String {
    url.replace("100x100bb.jpg", "1000x1000bb.jpg")
        .replace("100x100bb.png", "1000x1000bb.png")
}

fn map_http_err(e: HttpError) -> ProviderError {
    match e {
        HttpError::ServerRateLimited => ProviderError::RateLimited,
        HttpError::MissingUserAgent => ProviderError::Other(e.to_string()),
        other => ProviderError::Network(other.to_string()),
    }
}

#[cfg(test)]
#[path = "tests/rewrite_tests.rs"]
mod rewrite_tests;
