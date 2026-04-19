use phono_junk_identify::{Aggregator, HttpError};

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

    /// Register the MVP provider set: MusicBrainz (identification + CAA
    /// assets) plus iTunes (asset-only fallback).
    ///
    /// `user_agent` is forwarded to every provider's `HttpClient`; MB requires
    /// a descriptive UA with contact info (e.g.
    /// `"phono-junk/0.1 ( you@example.com )"`).
    ///
    /// Amazon is registered once an ASIN source exists (Discogs or user
    /// entry) — both deferred post-MVP. See TODO.md.
    pub fn with_default_providers(user_agent: impl Into<String>) -> Result<Self, HttpError> {
        let ua = user_agent.into();
        let mut ctx = Self::new();
        ctx.aggregator.register_identifier(Box::new(
            phono_junk_musicbrainz::MusicBrainzProvider::new(&ua)?,
        ));
        ctx.aggregator.register_asset_provider(Box::new(
            phono_junk_musicbrainz::CoverArtArchiveProvider::new(&ua)?,
        ));
        ctx.aggregator
            .register_asset_provider(Box::new(phono_junk_itunes::ITunesProvider::new(&ua)?));
        Ok(ctx)
    }
}

impl Default for PhonoContext {
    fn default() -> Self {
        Self::new()
    }
}
