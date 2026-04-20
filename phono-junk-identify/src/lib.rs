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

pub mod consensus;
pub mod fanout;
pub mod http;

pub use consensus::{DisagreementEntity, MergedDisc, RawDisagreement, merge};
pub use fanout::{identify_parallel, lookup_assets_parallel, spawn_all};
pub use http::{HttpClient, HttpClientBuilder, HttpError, HttpResponse};

/// Re-exports of the header types used by [`HttpClient::get_with_headers`].
/// Provider crates construct headers through this module so they don't
/// need to declare their own `reqwest` dependency.
pub mod header {
    pub use reqwest::header::{
        AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, InvalidHeaderValue,
    };
}

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
///
/// Never leaks via `Debug` — the custom impl emits provider names only.
#[derive(Clone, Default)]
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
    pub fn has(&self, provider: &str) -> bool {
        self.entries.contains_key(provider)
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut keys: Vec<&str> = self.entries.keys().map(String::as_str).collect();
        keys.sort_unstable();
        f.debug_struct("Credentials")
            .field("providers", &keys)
            .finish()
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
    /// No credential registered for this provider. Fan-out collects this
    /// as a per-provider error so the GUI's detail panel can show a
    /// "no token — open Settings" row instead of failing the identify call.
    #[error("missing credential: {0}")]
    MissingCredential(&'static str),
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
    /// ISO 639-3 language code (MB `text-representation.language`).
    pub language: Option<String>,
    /// ISO 15924 script code (MB `text-representation.script`).
    pub script: Option<String>,
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
/// user hints, etc.) are additive rather than trait-breaking. The
/// aggregator guarantees an `AlbumMeta` is resolved by consensus before
/// asset fan-out fires, so `album` is borrowed directly rather than
/// `Option<&_>`.
#[derive(Debug, Clone, Copy)]
pub struct AssetLookupCtx<'a> {
    pub album: &'a AlbumMeta,
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

/// Output of [`Aggregator::identify`]. Pure value — no DB side effects.
/// Persistence (writing `Album` / `Release` / `Disc` / `Track` /
/// `Disagreement` rows) is orchestrated by `phono-junk-lib::identify`.
pub struct IdentifyOutcome {
    pub merged: MergedDisc,
    /// Provider errors that did not short-circuit the batch.
    pub errors: Vec<(String, ProviderError)>,
    /// `true` iff at least one provider returned `Ok(Some(...))`.
    pub any_match: bool,
}

/// Output of [`Aggregator::lookup_assets`]. Candidates are deduplicated
/// across providers by `(asset_type, source_url)` — CAA and iTunes both
/// offer front covers, and double-inserting the same URL twice would
/// dirty the catalog.
pub struct AssetOutcome {
    pub candidates: Vec<AssetCandidate>,
    pub errors: Vec<(String, ProviderError)>,
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

    /// Fan out to every registered [`IdentificationProvider`] that can
    /// answer with the ids available, merge the matches via
    /// [`consensus::merge`], and return the outcome. Provider errors are
    /// collected in `errors` and never short-circuit the batch.
    pub fn identify(&self, toc: &Toc, ids: &DiscIds, creds: &Credentials) -> IdentifyOutcome {
        let raw = fanout::identify_parallel(&self.identifiers, toc, ids, creds);

        let mut matches: Vec<ProviderResult> = Vec::new();
        let mut errors: Vec<(String, ProviderError)> = Vec::new();
        for (name, result) in raw {
            match result {
                Ok(Some(r)) => matches.push(r),
                Ok(None) => {}
                Err(e) => errors.push((name, e)),
            }
        }

        let any_match = !matches.is_empty();
        let merged = if any_match {
            consensus::merge(&matches)
        } else {
            MergedDisc::default()
        };

        IdentifyOutcome {
            merged,
            errors,
            any_match,
        }
    }

    /// Fan out to every registered [`AssetProvider`], collect candidates
    /// in priority order, and deduplicate by `(asset_type, source_url)`
    /// so CAA and iTunes can't both insert the same front-cover URL.
    pub fn lookup_assets(&self, ctx: &AssetLookupCtx<'_>) -> AssetOutcome {
        let raw = fanout::lookup_assets_parallel(&self.assets, ctx);
        let mut candidates: Vec<AssetCandidate> = Vec::new();
        let mut seen: std::collections::HashSet<(AssetType, String)> =
            std::collections::HashSet::new();
        let mut errors: Vec<(String, ProviderError)> = Vec::new();
        for (name, result) in raw {
            match result {
                Ok(batch) => {
                    for c in batch {
                        let key = (c.asset_type, c.source_url.as_str().to_string());
                        if seen.insert(key) {
                            candidates.push(c);
                        }
                    }
                }
                Err(e) => errors.push((name, e)),
            }
        }
        AssetOutcome { candidates, errors }
    }
}

impl Default for Aggregator {
    fn default() -> Self {
        Self::new()
    }
}
