//! Tower Records Japan MDB identification + asset provider.
//!
//! Scrapes `mdb.tower.jp` — a Blazor Server SSR site — for releases
//! keyed on barcode or catalog number. Fills the domestic-Japan
//! coverage gap that Discogs and MusicBrainz leave behind.
//!
//! Two trait impls live on one struct, mirroring
//! `phono-junk-discogs`. No credentials required. Rate limiting is
//! coordinated through the shared [`phono_junk_identify::HttpClient`];
//! `PhonoContext::with_default_providers` registers a 1 req / 2 sec
//! host quota for `mdb.tower.jp`.
//!
//! Responses are cached on disk (30-day TTL for release pages, 7-day
//! TTL for barcode searches including negative hits). The cache lives
//! in this crate today; once a second scraper provider ships, extract
//! to shared infra — see TODO.md Cross-repo section.
//!
//! # Parse surface
//!
//! [`parse::parse_search_page`] and [`parse::parse_release_page`]
//! operate on raw bytes so unit tests can exercise them against the
//! recorded HTML fixtures under `tests/fixtures/`.

pub mod cache;
pub mod parse;

use phono_junk_core::{DiscIds, Toc};
use phono_junk_identify::{
    AlbumMeta, AssetCandidate, AssetConfidence, AssetLookupCtx, AssetProvider, AssetType,
    Credentials, DiscIdKind, HttpClient, HttpError, HttpResponse, IdentificationProvider,
    ProviderError, ProviderResult, ReleaseMeta, TrackMeta,
};
use url::Url;

use crate::cache::{CacheKind, ResponseCache};
use crate::parse::ReleaseDetail;

const PROVIDER: &str = "tower";
const DEFAULT_BASE_URL: &str = "https://mdb.tower.jp";

/// Tower Records MDB provider. Implements both [`IdentificationProvider`]
/// and [`AssetProvider`].
#[derive(Clone)]
pub struct TowerProvider {
    http: Option<HttpClient>,
    cache: Option<ResponseCache>,
    base_url: String,
}

impl TowerProvider {
    /// Construct without HTTP. Lookups silently return `Ok(None)`.
    /// Matches the `DiscogsProvider::new` shape for the aggregator's
    /// wiring-only tests.
    pub fn new() -> Self {
        Self {
            http: None,
            cache: None,
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    /// Canonical constructor — accepts the shared [`HttpClient`] with
    /// `mdb.tower.jp`'s host quota registered. Populates a default
    /// on-disk response cache under the platform cache root; if that
    /// fails (no `$HOME` etc.) the provider runs uncached.
    pub fn with_client(http: HttpClient) -> Self {
        let cache = ResponseCache::default_for(PROVIDER)
            .map_err(|e| log::warn!("tower: cache disabled ({e})"))
            .ok();
        Self {
            http: Some(http),
            cache,
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    /// Inject an explicit cache (tests, or callers wanting custom TTLs).
    pub fn with_client_and_cache(http: HttpClient, cache: ResponseCache) -> Self {
        Self {
            http: Some(http),
            cache: Some(cache),
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    /// Override the base URL — tests point this at an httpmock server.
    /// Production uses the default.
    pub fn with_base_url(mut self, base: impl Into<String>) -> Self {
        self.base_url = base.into();
        self
    }

    fn http(&self) -> Result<&HttpClient, ProviderError> {
        self.http
            .as_ref()
            .ok_or_else(|| ProviderError::Other("tower: HTTP client not configured".into()))
    }

    /// Fetch a URL, checking the cache first when one is configured.
    fn fetch(&self, url: &str, kind: CacheKind) -> Result<HttpResponse, ProviderError> {
        let client = self.http()?;
        match &self.cache {
            Some(c) => c
                .get_or_fetch(url, kind, || client.get(url))
                .map_err(map_http_err),
            None => client.get(url).map_err(map_http_err),
        }
    }

    fn search_url(&self, query: &str) -> String {
        // `{base}/search/{query}/0` — page 0. No URL-encoding of
        // barcodes (digits) or catalog numbers (alphanumeric + dashes)
        // is strictly required, but encode to be safe.
        let enc = url_encode_segment(query);
        format!("{}/search/{}/0", self.base_url, enc)
    }

    fn release_url(&self, id: u32) -> String {
        format!("{}/release/{}", self.base_url, id)
    }

    /// Run the search→release pipeline and return the populated release
    /// detail along with the chosen release ID. `Ok(None)` is "no match".
    fn identify_via_search(&self, ids: &DiscIds) -> Result<Option<ReleaseDetail>, ProviderError> {
        let Some(query) = primary_lookup_key(ids) else {
            return Ok(None);
        };
        let search_url = self.search_url(query);
        let resp = self.fetch(&search_url, CacheKind::Search)?;
        match resp.status {
            200 => {}
            404 => return Ok(None),
            429 => return Err(ProviderError::RateLimited),
            code => {
                return Err(ProviderError::Network(format!(
                    "tower search HTTP {code}"
                )));
            }
        }
        let hits = parse::parse_search_page(&resp.body)?;
        let Some(hit) = hits.first() else {
            return Ok(None);
        };
        if hits.len() > 1 {
            log::warn!(
                "tower search returned {} hits for '{query}'; picking first (id {})",
                hits.len(),
                hit.release_id,
            );
        }
        let release_url = self.release_url(hit.release_id);
        let resp = self.fetch(&release_url, CacheKind::Release)?;
        match resp.status {
            200 => {}
            404 => return Ok(None),
            429 => return Err(ProviderError::RateLimited),
            code => {
                return Err(ProviderError::Network(format!(
                    "tower release HTTP {code}"
                )));
            }
        }
        let detail = parse::parse_release_page(&resp.body)?;
        Ok(Some(detail))
    }
}

impl Default for TowerProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// Which `DiscIds` key Tower keys the lookup on. Barcode preferred,
/// catalog number fallback — identical precedence to Discogs.
fn primary_lookup_key(ids: &DiscIds) -> Option<&str> {
    if let Some(bc) = ids.barcode.as_deref().filter(|s| !s.is_empty()) {
        return Some(bc);
    }
    ids.catalog_number.as_deref().filter(|s| !s.is_empty())
}

impl IdentificationProvider for TowerProvider {
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
        _creds: &Credentials,
    ) -> Result<Option<ProviderResult>, ProviderError> {
        let Some(detail) = self.identify_via_search(ids)? else {
            return Ok(None);
        };
        Ok(Some(result_from_detail(detail)))
    }
}

impl AssetProvider for TowerProvider {
    fn name(&self) -> &'static str {
        PROVIDER
    }

    fn asset_types(&self) -> &[AssetType] {
        &[AssetType::FrontCover]
    }

    fn lookup_art(&self, ctx: &AssetLookupCtx<'_>) -> Result<Vec<AssetCandidate>, ProviderError> {
        let detail = match self.identify_via_search(ctx.ids) {
            Ok(Some(d)) => d,
            Ok(None) => return Ok(Vec::new()),
            // Silent on asset-side failures — identify already logged it.
            Err(_) => return Ok(Vec::new()),
        };
        let Some(url_str) = detail.cover_url.as_deref() else {
            return Ok(Vec::new());
        };
        let Ok(source_url) = Url::parse(url_str) else {
            log::warn!("tower: invalid cover URL: {url_str}");
            return Ok(Vec::new());
        };
        Ok(vec![AssetCandidate {
            provider: PROVIDER.to_string(),
            asset_type: AssetType::FrontCover,
            source_url,
            width: None,
            height: None,
            confidence: AssetConfidence::Identifier,
        }])
    }
}

/// Convert a parsed release detail into the shared provider output shape.
fn result_from_detail(d: ReleaseDetail) -> ProviderResult {
    let album = Some(AlbumMeta {
        title: d.title.clone(),
        artist_credit: d.artist_name.clone(),
        year: d.release_year,
        mbid: None,
    });
    let release = Some(ReleaseMeta {
        country: d.country.clone(),
        date: d.release_date_text.clone(),
        label: d.label.clone(),
        catalog_number: d.catalog_number.clone(),
        barcode: d.barcode.clone(),
        mbid: None,
        language: None,
        script: None,
    });
    let tracks: Vec<TrackMeta> = d
        .tracks
        .iter()
        .map(|t| TrackMeta {
            position: t.position,
            title: Some(t.title.clone()),
            artist_credit: t.artist.clone(),
            length_frames: None,
            isrc: None,
            mbid: None,
        })
        .collect();
    let cover_art_urls = d.cover_url.clone().into_iter().collect();
    let raw_response = Some(serde_json::json!({
        "release_id": d.release_id,
        "tower_shop_url": d.tower_shop_url,
        "info_completeness": d.info_completeness,
        "description": d.description,
        "credits": d.credits.iter().map(|c| serde_json::json!({
            "role_ja": c.role_ja,
            "artist_name": c.artist_name,
            "artist_id": c.artist_id,
        })).collect::<Vec<_>>(),
        "version_list": d.version_list.iter().map(|v| serde_json::json!({
            "release_id": v.release_id,
            "title": v.title,
        })).collect::<Vec<_>>(),
        "formats": d.formats,
        "genre": d.genre,
    }));
    ProviderResult {
        album,
        release,
        tracks,
        cover_art_urls,
        provider: PROVIDER.to_string(),
        raw_response,
    }
}

fn map_http_err(e: HttpError) -> ProviderError {
    match e {
        HttpError::ServerRateLimited => ProviderError::RateLimited,
        HttpError::Timeout => ProviderError::Network("request timed out".into()),
        other => ProviderError::Network(other.to_string()),
    }
}

/// URL-encode a single path segment. Keep unreserved characters as-is;
/// percent-encode everything else (Japanese catalog numbers can contain
/// `/` or spaces in rare cases, and full-width chars need encoding).
fn url_encode_segment(s: &str) -> String {
    const UNRESERVED: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        if UNRESERVED.contains(b) {
            out.push(*b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod lib_tests {
    use super::*;

    #[test]
    fn primary_lookup_prefers_barcode() {
        let ids = DiscIds {
            barcode: Some("075992665629".into()),
            catalog_number: Some("26656".into()),
            ..DiscIds::default()
        };
        assert_eq!(primary_lookup_key(&ids), Some("075992665629"));
    }

    #[test]
    fn primary_lookup_falls_back_to_catalog_number() {
        let ids = DiscIds {
            catalog_number: Some("WPCR-13459".into()),
            ..DiscIds::default()
        };
        assert_eq!(primary_lookup_key(&ids), Some("WPCR-13459"));
    }

    #[test]
    fn primary_lookup_empty_strings_skipped() {
        let ids = DiscIds {
            barcode: Some("".into()),
            catalog_number: Some("WPCR-13459".into()),
            ..DiscIds::default()
        };
        assert_eq!(primary_lookup_key(&ids), Some("WPCR-13459"));
    }

    #[test]
    fn missing_http_yields_other_error() {
        let toc = Toc {
            first_track: 1,
            last_track: 1,
            leadout_sector: 0,
            track_offsets: vec![0],
        };
        let ids = DiscIds {
            barcode: Some("123".into()),
            ..DiscIds::default()
        };
        let p = TowerProvider::new();
        let creds = Credentials::new();
        let err = p.lookup(&toc, &ids, &creds).unwrap_err();
        assert!(matches!(err, ProviderError::Other(_)));
    }

    #[test]
    fn missing_ids_returns_ok_none() {
        let toc = Toc {
            first_track: 1,
            last_track: 1,
            leadout_sector: 0,
            track_offsets: vec![0],
        };
        let ids = DiscIds::default();
        let p = TowerProvider::new();
        let creds = Credentials::new();
        assert!(p.lookup(&toc, &ids, &creds).unwrap().is_none());
    }

    #[test]
    fn search_url_formats_query() {
        let p = TowerProvider::new().with_base_url("https://example.test");
        assert_eq!(
            p.search_url("075992665629"),
            "https://example.test/search/075992665629/0"
        );
    }

    #[test]
    fn search_url_percent_encodes_catalog() {
        let p = TowerProvider::new().with_base_url("https://example.test");
        assert_eq!(
            p.search_url("WPCR 13459"),
            "https://example.test/search/WPCR%2013459/0"
        );
    }
}
