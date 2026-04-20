use governor::Quota;
use nonzero_ext::nonzero;
use phono_junk_accuraterip::{ACCURATERIP_HOST, AccurateRipClient};
use phono_junk_identify::{Aggregator, HttpClient, HttpError};

use crate::credentials::CredentialStore;

/// Single entry point consumed by both CLI and GUI.
///
/// Holds the provider registry (identification + asset), credential store,
/// and the AccurateRip verification client. Analog of retro-junk-lib's
/// `AnalysisContext`.
pub struct PhonoContext {
    pub aggregator: Aggregator,
    pub credentials: CredentialStore,
    pub accuraterip: Option<AccurateRipClient>,
    /// Shared, rate-limited HTTP client used for cross-cutting fetches
    /// that aren't tied to a specific provider — e.g. Sprint 12's
    /// cover-art byte cache on first export. Cloned from the same
    /// builder as every provider so per-host quotas stay coordinated.
    pub http: Option<HttpClient>,
}

impl PhonoContext {
    pub fn new() -> Self {
        Self {
            aggregator: Aggregator::new(),
            credentials: CredentialStore::new(),
            accuraterip: None,
            http: None,
        }
    }

    /// Register the MVP provider set: MusicBrainz (identification + CAA
    /// assets), iTunes (asset-only fallback), Discogs (barcode /
    /// catalog-number identification + asset), and Barcode Lookup
    /// (generic-barcode final fallback, identification + asset), backed
    /// by a single shared [`HttpClient`] so per-host token buckets
    /// coordinate across providers. MusicBrainz and Cover Art Archive
    /// notably both hit `musicbrainz.org`-adjacent hosts; running them
    /// on independent clients would double-spend the 1 req/sec quota
    /// under parallel fan-out.
    ///
    /// Registration order matters: consensus breaks ties by
    /// registration order, so MB → Discogs → Barcode Lookup places
    /// Barcode Lookup last — it contributes only when the
    /// music-specific databases return nothing.
    ///
    /// Also constructs an [`AccurateRipClient`] sharing the same client, so
    /// Sprint 13's `verify` subcommand has a ready handle.
    ///
    /// `user_agent` is forwarded to the shared client; MB requires a
    /// descriptive UA with contact info (e.g.
    /// `"phono-junk/0.1 ( you@example.com )"`).
    ///
    /// On construction, the credential store tries to populate itself
    /// from the OS keyring. A missing backend is non-fatal: identification
    /// still works, Discogs just silently skips without a token.
    ///
    /// Amazon is registered once an ASIN source exists (populated from
    /// Discogs responses) — deferred. See TODO.md.
    pub fn with_default_providers(user_agent: impl Into<String>) -> Result<Self, HttpError> {
        let http = HttpClient::builder()
            .user_agent(user_agent)
            .host_quota("musicbrainz.org", Quota::per_second(nonzero!(1u32)))
            .host_quota("coverartarchive.org", Quota::per_second(nonzero!(1u32)))
            .host_quota("itunes.apple.com", Quota::per_minute(nonzero!(20u32)))
            .host_quota("api.discogs.com", Quota::per_second(nonzero!(1u32)))
            .host_quota("api.barcodelookup.com", Quota::per_second(nonzero!(1u32)))
            .host_quota(ACCURATERIP_HOST, Quota::per_second(nonzero!(1u32)))
            .build()?;

        let mut ctx = Self::new();
        ctx.aggregator
            .register_identifier(Box::new(phono_junk_musicbrainz::MusicBrainzProvider::with_client(http.clone())));
        ctx.aggregator.register_asset_provider(Box::new(
            phono_junk_musicbrainz::CoverArtArchiveProvider::with_client(http.clone()),
        ));
        ctx.aggregator
            .register_asset_provider(Box::new(phono_junk_itunes::ITunesProvider::with_client(http.clone())));
        // Discogs implements both traits on one struct. Box twice so each
        // aggregator slot has its own owned pointer.
        ctx.aggregator
            .register_identifier(Box::new(phono_junk_discogs::DiscogsProvider::with_client(http.clone())));
        ctx.aggregator
            .register_asset_provider(Box::new(phono_junk_discogs::DiscogsProvider::with_client(http.clone())));
        // Barcode Lookup — final fallback. Registered after Discogs so
        // consensus registration-order ties favour the music-specific
        // databases. Same dual-trait / double-box pattern as Discogs.
        ctx.aggregator.register_identifier(Box::new(
            phono_junk_barcodelookup::BarcodelookupProvider::with_client(http.clone()),
        ));
        ctx.aggregator.register_asset_provider(Box::new(
            phono_junk_barcodelookup::BarcodelookupProvider::with_client(http.clone()),
        ));
        ctx.accuraterip = Some(AccurateRipClient::with_client(http.clone()));
        ctx.http = Some(http);

        if let Err(e) = ctx.credentials.load_from_keyring() {
            log::warn!("credentials: {e}");
        }

        Ok(ctx)
    }
}

impl Default for PhonoContext {
    fn default() -> Self {
        Self::new()
    }
}
