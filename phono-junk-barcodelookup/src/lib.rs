//! Barcode Lookup (barcodelookup.com) identification + image asset provider.
//!
//! Keyed on `barcode` (UPC-A / UPC-E / EAN-13). Intended as a **final
//! fallback** after MusicBrainz and Discogs have declined: the generic
//! product database tends to have some record of long-tail commercial
//! pressings (regional imports, indie self-releases) that the
//! music-specific databases miss. It only returns album-level metadata
//! — no track listings — so the consensus merge relies on MB/Discogs
//! for track data when those providers do hit.
//!
//! One provider struct, both traits. Aggregator boxes it into each
//! slot; the shared [`phono_junk_identify::http::HttpClient`] keeps
//! per-host rate limits coordinated.
//!
//! ## API key handling
//!
//! Barcode Lookup authenticates via query-string `key=` parameter
//! (no auth header). The key is sourced from [`Credentials`] under
//! the name `"barcodelookup"` and is only ever attached to the URL
//! immediately before the GET — never logged, never embedded in
//! error messages. Auth failures surface as
//! [`ProviderError::Auth`] with a constant description.
//!
//! ## Parse surface
//!
//! [`parse_search_response`] and [`parse_search_assets`] operate on
//! raw bytes so unit tests can exercise them with recorded JSON
//! fixtures, matching the MB / Discogs / iTunes idiom.

use phono_junk_core::{DiscIds, Toc};
use phono_junk_identify::{
    AlbumMeta, AssetCandidate, AssetConfidence, AssetLookupCtx, AssetProvider, AssetType,
    Credentials, DiscIdKind, HttpClient, HttpError, IdentificationProvider, ProviderError,
    ProviderResult, ReleaseMeta,
};
use serde::Deserialize;
use url::Url;

const PROVIDER: &str = "barcodelookup";
const CRED_KEY: &str = "barcodelookup";
const API_BASE: &str = "https://api.barcodelookup.com/v3/products";

pub struct BarcodelookupProvider {
    http: Option<HttpClient>,
}

impl BarcodelookupProvider {
    /// Construct without HTTP. Lookups silently return `Ok(None)`.
    /// Useful in tests that only exercise the asset half or the
    /// aggregator wiring; production registers via [`Self::with_client`].
    pub fn new() -> Self {
        Self { http: None }
    }

    /// Canonical constructor — accepts the shared [`HttpClient`] with
    /// Barcode Lookup's host quota registered, so per-host rate limits
    /// coordinate across providers.
    pub fn with_client(http: HttpClient) -> Self {
        Self { http: Some(http) }
    }

    fn http(&self) -> Result<&HttpClient, ProviderError> {
        self.http
            .as_ref()
            .ok_or_else(|| ProviderError::Other("barcodelookup: HTTP client not configured".into()))
    }
}

impl Default for BarcodelookupProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the lookup URL. Returns `None` when no barcode is set.
///
/// Kept separate from any particular host so integration tests can
/// substitute a mock server base via [`build_search_url_with_base`].
fn build_search_url(ids: &DiscIds, key: &str) -> Option<Url> {
    build_search_url_with_base(API_BASE, ids, key)
}

fn build_search_url_with_base(base: &str, ids: &DiscIds, key: &str) -> Option<Url> {
    let barcode = ids.barcode.as_deref().filter(|s| !s.is_empty())?;
    let mut url = Url::parse(base).ok()?;
    url.query_pairs_mut()
        .append_pair("barcode", barcode)
        .append_pair("formatted", "y")
        .append_pair("key", key);
    Some(url)
}

impl IdentificationProvider for BarcodelookupProvider {
    fn name(&self) -> &'static str {
        PROVIDER
    }

    fn supported_ids(&self) -> &[DiscIdKind] {
        // No catalog-number fallback — barcodelookup is strictly
        // barcode-indexed.
        &[DiscIdKind::Barcode]
    }

    fn lookup(
        &self,
        _toc: &Toc,
        ids: &DiscIds,
        creds: &Credentials,
    ) -> Result<Option<ProviderResult>, ProviderError> {
        let Some(key) = creds.get(CRED_KEY) else {
            return Err(ProviderError::MissingCredential("barcodelookup"));
        };
        let Some(url) = build_search_url(ids, key) else {
            return Ok(None);
        };
        let resp = self.http()?.get(url.as_str()).map_err(map_http_err)?;
        match resp.status {
            200 => parse_search_response(&resp.body),
            401 | 403 => Err(ProviderError::Auth("barcodelookup key rejected".into())),
            429 => Err(ProviderError::RateLimited),
            404 => Ok(None),
            code => Err(ProviderError::Network(format!(
                "barcodelookup HTTP {code}"
            ))),
        }
    }
}

impl AssetProvider for BarcodelookupProvider {
    fn name(&self) -> &'static str {
        PROVIDER
    }

    fn asset_types(&self) -> &[AssetType] {
        // The `images` array on a barcodelookup response is not
        // semantically typed — the first entry is generally a product
        // photo of the front of the packaging. Don't over-claim
        // back/booklet coverage.
        &[AssetType::FrontCover]
    }

    fn lookup_art(&self, ctx: &AssetLookupCtx<'_>) -> Result<Vec<AssetCandidate>, ProviderError> {
        let Some(key) = ctx.creds.get(CRED_KEY) else {
            // Missing credential on the asset half is NOT an error —
            // the identification path already surfaced it. Silent skip.
            return Ok(Vec::new());
        };
        let Some(url) = build_search_url(ctx.ids, key) else {
            return Ok(Vec::new());
        };
        let resp = self.http()?.get(url.as_str()).map_err(map_http_err)?;
        if resp.status != 200 {
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
// JSON shapes (barcodelookup v3 /products subset)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ProductsResponse {
    #[serde(default)]
    products: Vec<Product>,
}

#[derive(Debug, Deserialize)]
struct Product {
    #[serde(default)]
    barcode_number: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    manufacturer: Option<String>,
    #[serde(default)]
    brand: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    images: Vec<String>,
}

/// Parse a `/v3/products` response body into a [`ProviderResult`].
///
/// Empty `products` → `Ok(None)`. Multiple hits → picks the first and
/// logs a warning (same behaviour as Discogs / MusicBrainz multi-hit).
/// Track data is never populated — barcodelookup has no track endpoint.
pub fn parse_search_response(bytes: &[u8]) -> Result<Option<ProviderResult>, ProviderError> {
    let resp: ProductsResponse = serde_json::from_slice(bytes)
        .map_err(|e| ProviderError::Parse(format!("barcodelookup: {e}")))?;

    if resp.products.is_empty() {
        return Ok(None);
    }
    if resp.products.len() > 1 {
        log::warn!(
            "barcodelookup returned {} products; picking first",
            resp.products.len(),
        );
    }

    let raw_response = serde_json::from_slice::<serde_json::Value>(bytes).ok();
    let p = &resp.products[0];

    let artist_credit = p
        .manufacturer
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| p.brand.clone().filter(|s| !s.trim().is_empty()));

    let year = parse_release_year(p.release_date.as_deref());

    let album = Some(AlbumMeta {
        title: p.title.clone().filter(|s| !s.trim().is_empty()),
        artist_credit,
        year,
        mbid: None,
    });

    let release = Some(ReleaseMeta {
        country: None,
        date: p.release_date.clone().filter(|s| !s.trim().is_empty()),
        label: p.manufacturer.clone().filter(|s| !s.trim().is_empty()),
        catalog_number: None,
        barcode: p.barcode_number.clone().filter(|s| !s.trim().is_empty()),
        mbid: None,
        language: None,
        script: None,
    });

    Ok(Some(ProviderResult {
        album,
        release,
        tracks: Vec::new(),
        cover_art_urls: p.images.iter().cloned().collect(),
        provider: PROVIDER.to_string(),
        raw_response,
    }))
}

/// Parse a `/v3/products` response into asset candidates. The first
/// image is treated as the front cover. Any additional images are
/// ignored because barcodelookup doesn't semantically type them.
pub fn parse_search_assets(bytes: &[u8]) -> Result<Vec<AssetCandidate>, ProviderError> {
    let resp: ProductsResponse = serde_json::from_slice(bytes)
        .map_err(|e| ProviderError::Parse(format!("barcodelookup: {e}")))?;
    let mut out = Vec::new();
    for product in resp.products.iter().take(1) {
        let Some(url_str) = product.images.first() else {
            continue;
        };
        let Ok(url) = Url::parse(url_str) else {
            log::warn!("barcodelookup: invalid image URL: {url_str}");
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

/// Extract a 4-digit year from a release-date string. Accepts
/// `"1996"`, `"1996-05-03"`, `"1996/05/03"`, and tolerates padding.
fn parse_release_year(date: Option<&str>) -> Option<u16> {
    let s = date?.trim();
    if s.is_empty() {
        return None;
    }
    let head = s
        .split(['-', '/', ' ', 'T'])
        .next()
        .unwrap_or("")
        .trim();
    head.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn barcode_ids(bc: &str) -> DiscIds {
        DiscIds {
            barcode: Some(bc.into()),
            ..DiscIds::default()
        }
    }

    fn minimal_toc() -> Toc {
        Toc {
            first_track: 1,
            last_track: 1,
            leadout_sector: 0,
            track_offsets: vec![0],
        }
    }

    #[test]
    fn build_search_url_includes_barcode_and_key() {
        let url = build_search_url(&barcode_ids("0123456789012"), "KEY123").unwrap();
        let q: std::collections::HashMap<_, _> = url
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(q.get("barcode").map(String::as_str), Some("0123456789012"));
        assert_eq!(q.get("key").map(String::as_str), Some("KEY123"));
        assert_eq!(q.get("formatted").map(String::as_str), Some("y"));
    }

    #[test]
    fn build_search_url_returns_none_without_barcode() {
        assert!(build_search_url(&DiscIds::default(), "KEY").is_none());
        // Empty-string barcode is treated as absent — same defence as
        // Discogs uses against accidentally-blank-but-Some strings.
        let blank = DiscIds {
            barcode: Some(String::new()),
            ..DiscIds::default()
        };
        assert!(build_search_url(&blank, "KEY").is_none());
    }

    #[test]
    fn parse_search_response_barcode_hit() {
        let bytes = include_bytes!("../tests/fixtures/search_barcode_hit.json");
        let r = parse_search_response(bytes).unwrap().unwrap();
        assert_eq!(r.provider, "barcodelookup");
        let album = r.album.as_ref().unwrap();
        assert_eq!(album.title.as_deref(), Some("Test Album"));
        assert_eq!(album.artist_credit.as_deref(), Some("Test Label"));
        assert_eq!(album.year, Some(1996));
        let release = r.release.as_ref().unwrap();
        assert_eq!(release.barcode.as_deref(), Some("0123456789012"));
        assert_eq!(release.label.as_deref(), Some("Test Label"));
        assert_eq!(release.date.as_deref(), Some("1996-05-03"));
        assert!(r.tracks.is_empty(), "barcodelookup never populates tracks");
        assert_eq!(r.cover_art_urls.len(), 2);
    }

    #[test]
    fn parse_search_response_empty_products() {
        let bytes = br#"{"products":[]}"#;
        assert!(parse_search_response(bytes).unwrap().is_none());
    }

    #[test]
    fn parse_search_response_falls_back_to_brand_when_manufacturer_missing() {
        let bytes = br#"{"products":[{
            "barcode_number":"0000000000000",
            "title":"Brand-only album",
            "brand":"Some Brand",
            "release_date":"2012",
            "images":[]
        }]}"#;
        let r = parse_search_response(bytes).unwrap().unwrap();
        assert_eq!(
            r.album.as_ref().and_then(|a| a.artist_credit.as_deref()),
            Some("Some Brand")
        );
    }

    #[test]
    fn parse_search_assets_emits_first_image_as_front_cover() {
        let bytes = include_bytes!("../tests/fixtures/search_barcode_hit.json");
        let assets = parse_search_assets(bytes).unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].asset_type, AssetType::FrontCover);
        assert_eq!(assets[0].confidence, AssetConfidence::Identifier);
        assert_eq!(assets[0].provider, "barcodelookup");
    }

    #[test]
    fn parse_release_year_handles_formats() {
        assert_eq!(parse_release_year(Some("1996")), Some(1996));
        assert_eq!(parse_release_year(Some("1996-05-03")), Some(1996));
        assert_eq!(parse_release_year(Some("1996/05/03")), Some(1996));
        assert_eq!(parse_release_year(Some("1996-05-03T00:00:00")), Some(1996));
        assert_eq!(parse_release_year(Some("")), None);
        assert_eq!(parse_release_year(None), None);
        assert_eq!(parse_release_year(Some("not-a-date")), None);
    }

    #[test]
    fn missing_token_is_missing_credential_error() {
        let p = BarcodelookupProvider::new();
        let err = p
            .lookup(&minimal_toc(), &barcode_ids("0123456789012"), &Credentials::new())
            .unwrap_err();
        assert!(matches!(err, ProviderError::MissingCredential("barcodelookup")));
    }

    #[test]
    fn missing_barcode_is_none_not_error() {
        // With a credential but no barcode, the provider returns
        // Ok(None) rather than reaching for the network.
        let p = BarcodelookupProvider::new();
        let mut creds = Credentials::new();
        creds.set("barcodelookup", "KEY");
        let out = p.lookup(&minimal_toc(), &DiscIds::default(), &creds).unwrap();
        assert!(out.is_none());
    }
}
