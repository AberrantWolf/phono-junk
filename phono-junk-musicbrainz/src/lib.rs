//! MusicBrainz identification + Cover Art Archive asset provider.
//!
//! Implements both [`IdentificationProvider`] (keyed on `mb_discid`) and
//! [`AssetProvider`] (front cover / back cover / booklet from CAA).
//!
//! Unauthenticated. 1 req/sec rate limit, mandatory User-Agent.

use phono_junk_core::{DiscIds, Toc};
use phono_junk_identify::{
    AssetCandidate, AssetProvider, AssetType, Credentials, DiscIdKind, IdentificationProvider,
    ProviderError, ProviderResult, ReleaseMeta,
};

pub struct MusicBrainzProvider;

impl MusicBrainzProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MusicBrainzProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl IdentificationProvider for MusicBrainzProvider {
    fn name(&self) -> &'static str {
        "musicbrainz"
    }

    fn supported_ids(&self) -> &[DiscIdKind] {
        &[DiscIdKind::MbDiscId]
    }

    fn lookup(
        &self,
        _toc: &Toc,
        _ids: &DiscIds,
        _creds: &Credentials,
    ) -> Result<Option<ProviderResult>, ProviderError> {
        // TODO: GET musicbrainz.org/ws/2/discid/<id>?inc=artists+recordings+release-groups&fmt=json
        Ok(None)
    }
}

pub struct CoverArtArchiveProvider;

impl CoverArtArchiveProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CoverArtArchiveProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl AssetProvider for CoverArtArchiveProvider {
    fn name(&self) -> &'static str {
        "cover-art-archive"
    }

    fn asset_types(&self) -> &[AssetType] {
        &[
            AssetType::FrontCover,
            AssetType::BackCover,
            AssetType::Booklet,
        ]
    }

    fn lookup_art(
        &self,
        _release: &ReleaseMeta,
        _ids: &DiscIds,
        _creds: &Credentials,
    ) -> Result<Vec<AssetCandidate>, ProviderError> {
        // TODO: GET coverartarchive.org/release/<mbid>
        Ok(Vec::new())
    }
}
