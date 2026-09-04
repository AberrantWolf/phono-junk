//! Provider traits and aggregation.
//!
//! Defines [`IdentificationProvider`] (MusicBrainz, Discogs, future sources)
//! and [`AssetProvider`] (Cover Art Archive, iTunes, future sources).
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
pub mod pipeline;

pub use consensus::{
    DisagreementEntity, MergedDisc, RawDisagreement, merge, merge_with_toc_fallback,
};
pub use fanout::{lookup_assets_parallel, spawn_all};
pub use http::{HttpClient, HttpClientBuilder, HttpError, HttpResponse};
pub use pipeline::{ProviderObservation, StagedIdentifyOutcome, score_and_resolve};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ProviderTier {
    ExactDisc,
    MusicApi,
    MusicFallback,
    GenericBarcode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostRatePolicy {
    pub host: &'static str,
    pub requests: u32,
    pub period_seconds: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub name: &'static str,
    pub tier: ProviderTier,
    pub required_ids: &'static [DiscIdKind],
    pub emitted_ids: &'static [DiscIdKind],
    pub identifies: bool,
    pub supplies_assets: bool,
    pub required_credential: Option<&'static str>,
    pub host_rate_policy: Option<HostRatePolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LearnedExternalId {
    pub kind: DiscIdKind,
    pub value: String,
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
/// ignore this. Providers that do (Discogs, Barcode Lookup) pull their
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseCandidate {
    pub candidate_key: String,
    pub provider: String,
    pub album: AlbumMeta,
    pub release: ReleaseMeta,
    pub tracks: Vec<TrackMeta>,
    pub physical_disc_number: Option<u8>,
    pub exact_disc_association: bool,
    pub raw_response: Option<serde_json::Value>,
    pub cover_art_urls: Vec<String>,
}

impl ReleaseCandidate {
    pub fn from_result(result: ProviderResult) -> Option<Self> {
        let album = result.album?;
        let release = result.release.unwrap_or_default();
        let candidate_key = release
            .mbid
            .as_ref()
            .map(|id| format!("musicbrainz:release:{id}"))
            .unwrap_or_else(|| {
                format!(
                    "{}:{}:{}:{}",
                    result.provider,
                    release.barcode.as_deref().unwrap_or(""),
                    release.catalog_number.as_deref().unwrap_or(""),
                    album.title.as_deref().unwrap_or("")
                )
            });
        Some(Self {
            candidate_key,
            provider: result.provider,
            album,
            release,
            tracks: result.tracks,
            physical_disc_number: None,
            exact_disc_association: false,
            raw_response: result.raw_response,
            cover_art_urls: result.cover_art_urls,
        })
    }

    pub fn into_provider_result(self) -> ProviderResult {
        ProviderResult {
            album: Some(self.album),
            release: Some(self.release),
            tracks: self.tracks,
            cover_art_urls: self.cover_art_urls,
            provider: self.provider,
            raw_response: self.raw_response,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderLookup {
    pub release_candidates: Vec<ReleaseCandidate>,
    pub learned_ids: Vec<LearnedExternalId>,
    pub asset_candidates: Vec<String>,
    pub raw_response: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateScore {
    pub exact_disc_association: u8,
    pub exact_release_corroboration: u8,
    pub barcode_catalog_corroboration: u8,
    pub music_provider_support: u8,
    pub track_duration_agreement: u32,
    pub metadata_completeness: u8,
    pub provider_priority: u8,
}

impl CandidateScore {
    pub fn evidence_components(self) -> (u8, u8, u8, u8, u32, u8) {
        (
            self.exact_disc_association,
            self.exact_release_corroboration,
            self.barcode_catalog_corroboration,
            self.music_provider_support,
            self.track_duration_agreement,
            self.metadata_completeness,
        )
    }

    fn rank(self) -> (u8, u8, u8, u8, u32, u8, u8) {
        let (a, b, c, d, e, f) = self.evidence_components();
        (a, b, c, d, e, f, self.provider_priority)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredCandidate {
    pub candidate: ReleaseCandidate,
    pub score: CandidateScore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateResolution {
    pub selected: ScoredCandidate,
    pub alternatives: Vec<ScoredCandidate>,
    pub evidentially_ambiguous: bool,
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
    fn supported_ids(&self) -> &'static [DiscIdKind];

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            name: self.name(),
            tier: ProviderTier::MusicApi,
            required_ids: self.supported_ids(),
            emitted_ids: &[],
            identifies: true,
            supplies_assets: false,
            required_credential: None,
            host_rate_policy: None,
        }
    }

    /// Return every release candidate and learned identifier observed for this
    /// exact query. An empty lookup is a successful no-match result.
    fn lookup_many(
        &self,
        toc: &Toc,
        ids: &DiscIds,
        creds: &Credentials,
    ) -> Result<ProviderLookup, ProviderError>;
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

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            name: self.name(),
            tier: ProviderTier::MusicApi,
            required_ids: &[],
            emitted_ids: &[],
            identifies: false,
            supplies_assets: true,
            required_credential: None,
            host_rate_policy: None,
        }
    }

    /// Which asset types this provider can return.
    fn asset_types(&self) -> &'static [AssetType];

    /// Enumerate candidate assets for a release. Caller decides which to pick.
    fn lookup_art(&self, ctx: &AssetLookupCtx<'_>) -> Result<Vec<AssetCandidate>, ProviderError>;
}

/// Aggregator: executes registered providers through a staged evidence graph.
pub struct Aggregator {
    identifiers: Vec<Box<dyn IdentificationProvider>>,
    assets: Vec<Box<dyn AssetProvider>>,
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

    pub fn identify_staged(
        &self,
        toc: &Toc,
        ids: &DiscIds,
        creds: &Credentials,
    ) -> StagedIdentifyOutcome {
        pipeline::identify_staged(&self.identifiers, toc, ids, creds)
    }

    /// Fan out to every registered [`AssetProvider`], collect candidates
    /// in priority order, and deduplicate by `(asset_type, source_url)`
    /// so CAA and iTunes can't both insert the same front-cover URL.
    pub fn lookup_assets(&self, ctx: &AssetLookupCtx<'_>) -> AssetOutcome {
        self.lookup_assets_excluding(ctx, &[])
    }

    /// Reuse metadata-stage asset URLs by skipping providers already queried
    /// for the selected candidate. Asset-only providers still participate.
    pub fn lookup_assets_excluding(
        &self,
        ctx: &AssetLookupCtx<'_>,
        excluded_providers: &[&str],
    ) -> AssetOutcome {
        let providers: Vec<&dyn AssetProvider> = self
            .assets
            .iter()
            .map(|provider| provider.as_ref())
            .filter(|provider| !excluded_providers.contains(&provider.name()))
            .collect();
        let raw = fanout::lookup_assets_parallel_refs(&providers, ctx);
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
