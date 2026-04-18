use phono_junk_identify::Aggregator;

use crate::credentials::CredentialStore;

/// Single entry point consumed by both CLI and GUI.
///
/// Holds the provider registry (identification + asset) and credential store.
/// Analog of retro-junk-lib's `AnalysisContext`.
pub struct PhonoContext {
    pub aggregator: Aggregator,
    pub credentials: CredentialStore,
}

impl PhonoContext {
    pub fn new() -> Self {
        Self {
            aggregator: Aggregator::new(),
            credentials: CredentialStore::new(),
        }
    }

    /// Register the full day-1 provider set (MB + Discogs + CAA + iTunes + Amazon).
    pub fn with_default_providers() -> Self {
        let mut ctx = Self::new();
        ctx.aggregator
            .register_identifier(Box::new(phono_junk_musicbrainz::MusicBrainzProvider::new()));
        ctx.aggregator
            .register_identifier(Box::new(phono_junk_discogs::DiscogsProvider::new()));
        ctx.aggregator.register_asset_provider(Box::new(
            phono_junk_musicbrainz::CoverArtArchiveProvider::new(),
        ));
        ctx.aggregator
            .register_asset_provider(Box::new(phono_junk_discogs::DiscogsProvider::new()));
        ctx.aggregator
            .register_asset_provider(Box::new(phono_junk_itunes::ITunesProvider::new()));
        ctx.aggregator
            .register_asset_provider(Box::new(phono_junk_amazon::AmazonProvider::new()));
        ctx
    }
}

impl Default for PhonoContext {
    fn default() -> Self {
        Self::new()
    }
}
