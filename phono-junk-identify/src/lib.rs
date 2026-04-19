//! Provider traits and aggregation.
//!
//! Defines [`IdentificationProvider`] (MusicBrainz, Discogs, future sources)
//! and [`AssetProvider`] (Cover Art Archive, iTunes, Amazon, future sources).
//! Aggregation merges results across providers, writes `Disagreement` records
//! on conflict, and respects user `Override` rows.
//!
//! Also the home of the shared rate-limited [`http::HttpClient`] that every
//! provider constructs and uses. Co-located with the traits because provider
//! crates can't depend on `phono-junk-lib` (cycle) but all already depend on
//! this crate.

pub mod http;

pub use http::{HttpClient, HttpClientBuilder, HttpError, HttpResponse};

use phono_junk_core::{AudioError, DiscIds, Toc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

/// Which identifier a provider can key its lookup on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiscIdKind {
    MbDiscId,
    CddbId,
    AccurateRipId,
    Barcode,
    CatalogNumber,
}

/// Asset categories a provider may return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssetType {
    FrontCover,
    BackCover,
    CdLabel,
    Booklet,
    ObiStrip,
    TrayInsert,
    Other,
}

/// Confidence that an asset actually matches the release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetConfidence {
    /// Exact MBID or barcode match — trust it.
    Exact,
    /// Barcode/catalog-number match where available.
    Identifier,
    /// Fuzzy text match on artist+album — needs user confirmation.
    Fuzzy,
}

/// Credentials passed to providers that need them.
///
/// Providers that don't need auth (MusicBrainz, Cover Art Archive, iTunes)
/// ignore this. Providers that do (Discogs, Amazon PA-API) pull their
/// token out by name.
#[derive(Debug, Clone, Default)]
pub struct Credentials {
    entries: std::collections::HashMap<String, String>,
}

impl Credentials {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set(&mut self, provider: impl Into<String>, token: impl Into<String>) {
        self.entries.insert(provider.into(), token.into());
    }
    pub fn get(&self, provider: &str) -> Option<&str> {
        self.entries.get(provider).map(String::as_str)
    }
}

/// Errors from provider lookups.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("Network error: {0}")]
    Network(String),
    #[error("Auth error: {0}")]
    Auth(String),
    #[error("Rate limited")]
    RateLimited,
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Other: {0}")]
    Other(String),
}

impl From<ProviderError> for AudioError {
    fn from(e: ProviderError) -> Self {
        match e {
            ProviderError::Network(s) => AudioError::Network(s),
            ProviderError::RateLimited => AudioError::Network("rate limited".into()),
            other => AudioError::Other(other.to_string()),
        }
    }
}

/// Partial metadata returned by a single identification provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderResult {
    pub album: Option<AlbumMeta>,
    pub release: Option<ReleaseMeta>,
    pub tracks: Vec<TrackMeta>,
    pub cover_art_urls: Vec<String>,
    pub provider: String,
    /// Raw response for forensic inspection / disagreement drill-down.
    pub raw_response: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlbumMeta {
    pub title: Option<String>,
    pub artist_credit: Option<String>,
    pub year: Option<u16>,
    pub mbid: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReleaseMeta {
    pub country: Option<String>,
    pub date: Option<String>,
    pub label: Option<String>,
    pub catalog_number: Option<String>,
    pub barcode: Option<String>,
    pub mbid: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrackMeta {
    pub position: u8,
    pub title: Option<String>,
    pub artist_credit: Option<String>,
    pub length_frames: Option<u64>,
    pub isrc: Option<String>,
    pub mbid: Option<String>,
}

/// An asset candidate — one image from one provider.
#[derive(Debug, Clone)]
pub struct AssetCandidate {
    pub provider: String,
    pub asset_type: AssetType,
    pub source_url: Url,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub confidence: AssetConfidence,
}

/// Identification provider trait — implement once per external database.
pub trait IdentificationProvider: Send + Sync {
    fn name(&self) -> &'static str;

    /// Which IDs this provider can resolve. Aggregator uses this to skip
    /// providers that can't answer with the data available.
    fn supported_ids(&self) -> &[DiscIdKind];

    /// Attempt to identify the disc. `Ok(None)` means no match found.
    fn lookup(
        &self,
        toc: &Toc,
        ids: &DiscIds,
        creds: &Credentials,
    ) -> Result<Option<ProviderResult>, ProviderError>;
}

/// Context passed to [`AssetProvider::lookup_art`].
///
/// Bundled into a struct so future fields (language/country preference,
/// user hints, etc.) are additive rather than trait-breaking. `album` is
/// optional today because the aggregator (Sprint 11) isn't wired yet to
/// guarantee an album is resolved before art lookup; tighten to `&AlbumMeta`
/// once that path exists.
#[derive(Debug, Clone, Copy)]
pub struct AssetLookupCtx<'a> {
    pub album: Option<&'a AlbumMeta>,
    pub release: &'a ReleaseMeta,
    pub ids: &'a DiscIds,
    pub creds: &'a Credentials,
}

/// Asset provider trait — album art sources.
pub trait AssetProvider: Send + Sync {
    fn name(&self) -> &'static str;

    /// Which asset types this provider can return.
    fn asset_types(&self) -> &[AssetType];

    /// Enumerate candidate assets for a release. Caller decides which to pick.
    fn lookup_art(&self, ctx: &AssetLookupCtx<'_>) -> Result<Vec<AssetCandidate>, ProviderError>;
}

/// Aggregator: fans out to registered providers and merges results.
pub struct Aggregator {
    identifiers: Vec<Box<dyn IdentificationProvider>>,
    assets: Vec<Box<dyn AssetProvider>>,
}

impl Aggregator {
    pub fn new() -> Self {
        Self {
            identifiers: Vec::new(),
            assets: Vec::new(),
        }
    }

    pub fn register_identifier(&mut self, p: Box<dyn IdentificationProvider>) {
        self.identifiers.push(p);
    }

    pub fn register_asset_provider(&mut self, p: Box<dyn AssetProvider>) {
        self.assets.push(p);
    }

    pub fn identifiers(&self) -> &[Box<dyn IdentificationProvider>] {
        &self.identifiers
    }

    pub fn asset_providers(&self) -> &[Box<dyn AssetProvider>] {
        &self.assets
    }
}

impl Default for Aggregator {
    fn default() -> Self {
        Self::new()
    }
}
