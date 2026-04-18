//! iTunes Search API — asset-only (album art) provider.
//!
//! No auth. `itunes.apple.com/search?term=<artist>+<album>&entity=album`
//! returns `artworkUrl100`; rewrite URL segment `/100x100bb.jpg` →
//! `/1000x1000bb.jpg` for high-resolution art.
//!
//! Fuzzy confidence — the hit is based on text search, not an authoritative
//! identifier. UX should surface candidates for user confirmation.

use phono_junk_core::DiscIds;
use phono_junk_identify::{
    AssetCandidate, AssetProvider, AssetType, Credentials, ProviderError, ReleaseMeta,
};

pub struct ITunesProvider;

impl ITunesProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ITunesProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetProvider for ITunesProvider {
    fn name(&self) -> &'static str {
        "itunes"
    }

    fn asset_types(&self) -> &[AssetType] {
        &[AssetType::FrontCover]
    }

    fn lookup_art(
        &self,
        _release: &ReleaseMeta,
        _ids: &DiscIds,
        _creds: &Credentials,
    ) -> Result<Vec<AssetCandidate>, ProviderError> {
        // TODO: GET itunes.apple.com/search?term=...&entity=album, rewrite URLs to 1000x1000
        Ok(Vec::new())
    }
}
