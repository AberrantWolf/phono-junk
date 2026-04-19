//! Discogs identification + image asset provider.
//!
//! Keyed on `barcode` or `catalog_number`. Requires user token (60 req/min
//! authenticated; 25 req/min anonymous). Credential name: `"discogs"`.
//!
//! Discogs has no TOC-based lookup. Text search is available but fuzzy;
//! the MVP only attempts identifier-based lookup and surfaces manual
//! text search as a UX affordance elsewhere.

use phono_junk_core::{DiscIds, Toc};
use phono_junk_identify::{
    AssetCandidate, AssetLookupCtx, AssetProvider, AssetType, Credentials, DiscIdKind,
    IdentificationProvider, ProviderError, ProviderResult,
};

pub struct DiscogsProvider;

impl DiscogsProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DiscogsProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl IdentificationProvider for DiscogsProvider {
    fn name(&self) -> &'static str {
        "discogs"
    }

    fn supported_ids(&self) -> &[DiscIdKind] {
        &[DiscIdKind::Barcode, DiscIdKind::CatalogNumber]
    }

    fn lookup(
        &self,
        _toc: &Toc,
        _ids: &DiscIds,
        _creds: &Credentials,
    ) -> Result<Option<ProviderResult>, ProviderError> {
        // TODO: GET api.discogs.com/database/search?type=release&barcode=<barcode>
        Ok(None)
    }
}

impl AssetProvider for DiscogsProvider {
    fn name(&self) -> &'static str {
        "discogs"
    }

    fn asset_types(&self) -> &[AssetType] {
        &[
            AssetType::FrontCover,
            AssetType::BackCover,
            AssetType::CdLabel,
        ]
    }

    fn lookup_art(&self, _ctx: &AssetLookupCtx<'_>) -> Result<Vec<AssetCandidate>, ProviderError> {
        // Deferred post-MVP: requires user token + credential persistence.
        // See TODO.md.
        Ok(Vec::new())
    }
}
