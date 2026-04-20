//! Discogs identification + image asset provider.
//!
//! Keyed on `barcode` or `catalog_number`. Requires a user token
//! (60 req/min authenticated). Credential name: `"discogs"`.
//!
//! Two trait impls live on one struct. The aggregator registers each
//! half in its own slot (`register_identifier` + `register_asset_provider`);
//! the shared [`phono_junk_identify::http::HttpClient`] keeps per-host rate
//! limits coordinated.
//!
//! ## Token handling
//!
//! The token is injected as `Authorization: Discogs token=<t>` via
//! [`HttpClient::get_with_headers`]. It is never logged, and never
//! stringified into error messages — auth failures surface as
//! [`ProviderError::Auth`] with a constant description.
//!
//! ## Parse surface
//!
//! [`parse_search_response`] operates on raw bytes so unit tests can
//! exercise it with recorded JSON fixtures. Network / parse split
//! matches the MB / iTunes idiom.

use phono_junk_core::{DiscIds, Toc};
use phono_junk_identify::header::{AUTHORIZATION, HeaderName, HeaderValue};
use phono_junk_identify::{
    AlbumMeta, AssetCandidate, AssetConfidence, AssetLookupCtx, AssetProvider, AssetType,
    Credentials, DiscIdKind, HttpClient, HttpError, IdentificationProvider, ProviderError,
    ProviderResult, ReleaseMeta,
};
use serde::Deserialize;
use url::Url;

const PROVIDER: &str = "discogs";
const CRED_KEY: &str = "discogs";

pub struct DiscogsProvider {
    http: Option<HttpClient>,
}

impl DiscogsProvider {
    /// Construct without HTTP. Lookups silently return `Ok(None)`.
    /// Useful in tests that only exercise the asset half or the aggregator
    /// wiring; production registers via [`Self::with_client`].
    pub fn new() -> Self {
        Self { http: None }
    }

    /// Canonical constructor — accepts the shared [`HttpClient`] with
    /// Discogs's host quota registered, so per-host rate limits
    /// coordinate across providers.
    pub fn with_client(http: HttpClient) -> Self {
        Self { http: Some(http) }
    }

    fn http(&self) -> Result<&HttpClient, ProviderError> {
        self.http
            .as_ref()
            .ok_or_else(|| ProviderError::Other("discogs: HTTP client not configured".into()))
    }
}

impl Default for DiscogsProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Headers for Discogs-authenticated requests. Factored so callers never
/// build the `Authorization: Discogs token=<t>` string more than once.
fn auth_headers(token: &str) -> Result<[(HeaderName, HeaderValue); 1], ProviderError> {
    let value = HeaderValue::from_str(&format!("Discogs token={token}"))
        .map_err(|_| ProviderError::Auth("discogs token contains invalid header bytes".into()))?;
    Ok([(AUTHORIZATION, value)])
}

/// Build the search URL. Prefers `barcode`; falls back to `catno` when
/// barcode is empty but a catalog number is present. Returns `None`
/// when neither is set (fanout should already have filtered this case,
/// but we defend).
fn build_search_url(ids: &DiscIds) -> Option<Url> {
    let base = Url::parse("https://api.discogs.com/database/search").ok()?;
    let (key, value) = if let Some(bc) = ids.barcode.as_deref().filter(|s| !s.is_empty()) {
        ("barcode", bc)
    } else if let Some(cn) = ids.catalog_number.as_deref().filter(|s| !s.is_empty()) {
        ("catno", cn)
    } else {
        return None;
    };
    let mut url = base;
    url.query_pairs_mut()
        .append_pair("type", "release")
        .append_pair(key, value);
    Some(url)
}

impl IdentificationProvider for DiscogsProvider {
    fn name(&self) -> &'static str {
        PROVIDER
    }

    fn supported_ids(&self) -> &[DiscIdKind] {
        &[DiscIdKind::Barcode, DiscIdKind::CatalogNumber]
    }

    fn lookup(
        &self,
        _toc: &Toc,
        ids: &DiscIds,
        creds: &Credentials,
    ) -> Result<Option<ProviderResult>, ProviderError> {
        let Some(token) = creds.get(CRED_KEY) else {
            // Fan-out collects this as a per-provider error so the GUI's
            // detail panel can show a "no token" row instead of a silent
            // nothing. Does not fail the identify call.
            return Err(ProviderError::MissingCredential("discogs"));
        };
        let Some(url) = build_search_url(ids) else {
            return Ok(None);
        };
        let headers = auth_headers(token)?;
        let resp = self.http()?.get_with_headers(url.as_str(), &headers).map_err(map_http_err)?;
        match resp.status {
            200 => parse_search_response(&resp.body),
            401 | 403 => Err(ProviderError::Auth("discogs token rejected".into())),
            429 => Err(ProviderError::RateLimited),
            404 => Ok(None),
            code => Err(ProviderError::Network(format!(
                "discogs search HTTP {code}"
            ))),
        }
    }
}

impl AssetProvider for DiscogsProvider {
    fn name(&self) -> &'static str {
        PROVIDER
    }

    fn asset_types(&self) -> &[AssetType] {
        &[AssetType::FrontCover]
    }

    fn lookup_art(&self, ctx: &AssetLookupCtx<'_>) -> Result<Vec<AssetCandidate>, ProviderError> {
        let Some(token) = ctx.creds.get(CRED_KEY) else {
            // Missing token on the asset half is NOT an error — callers
            // are already surfacing identification errors. Silent skip.
            return Ok(Vec::new());
        };
        let Some(url) = build_search_url(ctx.ids) else {
            return Ok(Vec::new());
        };
        let headers = auth_headers(token)?;
        let resp = self.http()?.get_with_headers(url.as_str(), &headers).map_err(map_http_err)?;
        if resp.status != 200 {
            // Silent on asset-side failures — identify already logged it.
            return Ok(Vec::new());
        }
        parse_search_assets(&resp.body)
    }
}

fn map_http_err(e: HttpError) -> ProviderError {
    match e {
        HttpError::ServerRateLimited => ProviderError::RateLimited,
        HttpError::Timeout => ProviderError::Network("request timed out".into()),
        other => ProviderError::Network(other.to_string()),
    }
}

// ---------------------------------------------------------------------------
// JSON shapes (Discogs search API subset)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchHit>,
}

#[derive(Debug, Deserialize)]
struct SearchHit {
    #[serde(default)]
    id: Option<u64>,
    /// "Artist - Album" on search; we best-effort split.
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    year: Option<serde_json::Value>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    label: Vec<String>,
    #[serde(default)]
    catno: Option<String>,
    #[serde(default)]
    barcode: Vec<String>,
    #[serde(default)]
    cover_image: Option<String>,
}

/// Parse a `/database/search` response body into a [`ProviderResult`].
///
/// Empty `results` → `Ok(None)`. Multiple hits → picks the first and
/// logs a warning (same behaviour as MusicBrainz's multi-release path).
///
/// The `title` field on search hits is `"<Artist> - <Album>"`; we split
/// on the first ` - ` delimiter. Classical / multi-artist releases with
/// embedded dashes in the artist name will route the tail into the
/// album title — good enough for MVP; revisit if misclassifications
/// surface. The detail endpoint (`/releases/<id>`) returns split fields
/// but costs a second round-trip.
pub fn parse_search_response(bytes: &[u8]) -> Result<Option<ProviderResult>, ProviderError> {
    let resp: SearchResponse = serde_json::from_slice(bytes)
        .map_err(|e| ProviderError::Parse(format!("discogs search: {e}")))?;

    if resp.results.is_empty() {
        return Ok(None);
    }
    if resp.results.len() > 1 {
        log::warn!(
            "discogs search returned {} hits; picking first ({})",
            resp.results.len(),
            resp.results[0].id.unwrap_or(0),
        );
    }

    let raw_response = serde_json::from_slice::<serde_json::Value>(bytes).ok();
    let hit = &resp.results[0];
    let (artist, title) = split_title(hit.title.as_deref());
    let year = hit.year.as_ref().and_then(parse_year);

    let album = Some(AlbumMeta {
        title,
        artist_credit: artist,
        year,
        mbid: None,
    });

    let release = Some(ReleaseMeta {
        country: hit.country.clone(),
        date: year.map(|y| y.to_string()),
        label: hit.label.first().cloned(),
        catalog_number: hit.catno.clone(),
        barcode: hit.barcode.first().cloned(),
        mbid: None,
        language: None,
        script: None,
    });

    Ok(Some(ProviderResult {
        album,
        release,
        tracks: Vec::new(),
        cover_art_urls: hit.cover_image.clone().into_iter().collect(),
        provider: PROVIDER.to_string(),
        raw_response,
    }))
}

/// Parse a `/database/search` response into a list of cover-image asset
/// candidates. Returns an empty vec when no hit has a usable cover URL.
pub fn parse_search_assets(bytes: &[u8]) -> Result<Vec<AssetCandidate>, ProviderError> {
    let resp: SearchResponse = serde_json::from_slice(bytes)
        .map_err(|e| ProviderError::Parse(format!("discogs search: {e}")))?;
    let mut out = Vec::new();
    for hit in resp.results.iter().take(1) {
        let Some(url_str) = hit.cover_image.as_deref() else {
            continue;
        };
        let Ok(url) = Url::parse(url_str) else {
            log::warn!("discogs: invalid cover_image URL: {url_str}");
            continue;
        };
        out.push(AssetCandidate {
            provider: PROVIDER.to_string(),
            asset_type: AssetType::FrontCover,
            source_url: url,
            width: None,
            height: None,
            confidence: AssetConfidence::Identifier,
        });
    }
    Ok(out)
}

fn split_title(combined: Option<&str>) -> (Option<String>, Option<String>) {
    let t = match combined {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => return (None, None),
    };
    match t.split_once(" - ") {
        Some((artist, title)) if !artist.is_empty() && !title.is_empty() => {
            (Some(artist.trim().to_string()), Some(title.trim().to_string()))
        }
        _ => (None, Some(t.to_string())),
    }
}

fn parse_year(v: &serde_json::Value) -> Option<u16> {
    match v {
        serde_json::Value::Number(n) => n.as_u64().and_then(|n| u16::try_from(n).ok()),
        serde_json::Value::String(s) => {
            // Year field can be "1996", "1996-05-03", or empty.
            let head = s.split(['-', '/', ' ']).next().unwrap_or("");
            head.parse().ok()
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_search_response_barcode_hit() {
        let bytes = include_bytes!("../tests/fixtures/search_barcode_hit.json");
        let r = parse_search_response(bytes).unwrap().unwrap();
        assert_eq!(r.provider, "discogs");
        let album = r.album.as_ref().unwrap();
        assert_eq!(album.title.as_deref(), Some("Test Album"));
        assert_eq!(album.artist_credit.as_deref(), Some("Test Artist"));
        assert_eq!(album.year, Some(1996));
        let release = r.release.as_ref().unwrap();
        assert_eq!(release.country.as_deref(), Some("US"));
        assert_eq!(release.label.as_deref(), Some("Test Label"));
        assert_eq!(release.catalog_number.as_deref(), Some("TEST-001"));
        assert_eq!(release.barcode.as_deref(), Some("0123456789012"));
        assert_eq!(r.cover_art_urls.len(), 1);
    }

    #[test]
    fn parse_search_response_empty_results() {
        let bytes = br#"{"results":[]}"#;
        let r = parse_search_response(bytes).unwrap();
        assert!(r.is_none());
    }

    #[test]
    fn parse_search_assets_emits_front_cover() {
        let bytes = include_bytes!("../tests/fixtures/search_barcode_hit.json");
        let assets = parse_search_assets(bytes).unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].asset_type, AssetType::FrontCover);
        assert_eq!(assets[0].confidence, AssetConfidence::Identifier);
    }

    #[test]
    fn split_title_handles_plain_and_missing() {
        assert_eq!(
            split_title(Some("Foo - Bar")),
            (Some("Foo".into()), Some("Bar".into()))
        );
        assert_eq!(split_title(Some("")), (None, None));
        assert_eq!(split_title(Some("JustAlbum")), (None, Some("JustAlbum".into())));
        assert_eq!(split_title(None), (None, None));
    }

    #[test]
    fn build_search_url_prefers_barcode_over_catno() {
        let ids = DiscIds {
            barcode: Some("123".into()),
            catalog_number: Some("ABC".into()),
            ..DiscIds::default()
        };
        let u = build_search_url(&ids).unwrap();
        let q: std::collections::HashMap<_, _> = u
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(q.get("barcode").map(String::as_str), Some("123"));
        assert!(q.get("catno").is_none());
    }

    #[test]
    fn build_search_url_falls_back_to_catno() {
        let ids = DiscIds {
            catalog_number: Some("ABC".into()),
            ..DiscIds::default()
        };
        let u = build_search_url(&ids).unwrap();
        let q: std::collections::HashMap<_, _> = u
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(q.get("catno").map(String::as_str), Some("ABC"));
    }

    #[test]
    fn build_search_url_returns_none_without_ids() {
        let ids = DiscIds::default();
        assert!(build_search_url(&ids).is_none());
    }

    #[test]
    fn missing_token_is_missing_credential_error() {
        let ids = DiscIds {
            barcode: Some("123".into()),
            ..DiscIds::default()
        };
        let toc = Toc {
            first_track: 1,
            last_track: 1,
            leadout_sector: 0,
            track_offsets: vec![0],
        };
        let p = DiscogsProvider::new();
        let creds = Credentials::new();
        let err = p.lookup(&toc, &ids, &creds).unwrap_err();
        assert!(matches!(err, ProviderError::MissingCredential("discogs")));
    }
}
