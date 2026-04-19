//! Amazon image lookup — asset-only (album art) provider.
//!
//! Two modes:
//! 1. **ASIN-direct** — if an ASIN is present in the catalog (populated from
//!    Discogs or user entry), fetch `m.media-amazon.com/images/I/<id>.jpg`
//!    directly. No auth required.
//! 2. **PA-API search** — Amazon Product Advertising API v5. Requires affiliate
//!    credentials (`amazon_access_key` + `amazon_secret_key` + `amazon_partner_tag`).
//!    Fuzzy confidence.
//!
//! The MVP lands mode 1; mode 2 is wired but returns an empty result until
//! credentials are supplied.

use phono_junk_identify::{
    AssetCandidate, AssetLookupCtx, AssetProvider, AssetType, ProviderError,
};

pub struct AmazonProvider;

impl AmazonProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AmazonProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetProvider for AmazonProvider {
    fn name(&self) -> &'static str {
        "amazon"
    }

    fn asset_types(&self) -> &[AssetType] {
        &[AssetType::FrontCover]
    }

    fn lookup_art(&self, _ctx: &AssetLookupCtx<'_>) -> Result<Vec<AssetCandidate>, ProviderError> {
        // Deferred post-MVP: ASIN source (Discogs / user entry) isn't wired yet.
        // See TODO.md ("Amazon provider impl").
        Ok(Vec::new())
    }
}
