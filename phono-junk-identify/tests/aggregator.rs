//! Aggregator-level tests: identify() merges via consensus, lookup_assets()
//! dedupes, provider errors surface without poisoning the batch.

use phono_junk_core::{DiscIds, Toc};
use phono_junk_identify::{
    AlbumMeta, Aggregator, AssetCandidate, AssetConfidence, AssetLookupCtx, AssetProvider,
    AssetType, Credentials, DiscIdKind, IdentificationProvider, ProviderError, ProviderResult,
    ReleaseMeta,
};
use url::Url;

struct MockIdentifier {
    name: &'static str,
    outcome: Result<Option<ProviderResult>, &'static str>,
}

impl IdentificationProvider for MockIdentifier {
    fn name(&self) -> &'static str {
        self.name
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
        match &self.outcome {
            Ok(r) => Ok(r.clone()),
            Err(msg) => Err(ProviderError::Other((*msg).to_string())),
        }
    }
}

struct MockAssetProvider {
    name: &'static str,
    candidates: Vec<AssetCandidate>,
}

impl AssetProvider for MockAssetProvider {
    fn name(&self) -> &'static str {
        self.name
    }
    fn asset_types(&self) -> &[AssetType] {
        &[AssetType::FrontCover]
    }
    fn lookup_art(&self, _ctx: &AssetLookupCtx<'_>) -> Result<Vec<AssetCandidate>, ProviderError> {
        Ok(self.candidates.clone())
    }
}

fn default_toc() -> Toc {
    Toc {
        first_track: 1,
        last_track: 1,
        leadout_sector: 100,
        track_offsets: vec![0],
    }
}

fn discid_ids() -> DiscIds {
    DiscIds {
        mb_discid: Some("x".into()),
        ..Default::default()
    }
}

fn front_cover(provider: &'static str, url: &str) -> AssetCandidate {
    AssetCandidate {
        provider: provider.into(),
        asset_type: AssetType::FrontCover,
        source_url: Url::parse(url).unwrap(),
        width: None,
        height: None,
        confidence: AssetConfidence::Exact,
    }
}

#[test]
fn identify_merges_two_mock_providers() {
    let mut agg = Aggregator::new();
    agg.register_identifier(Box::new(MockIdentifier {
        name: "a",
        outcome: Ok(Some(ProviderResult {
            album: Some(AlbumMeta {
                title: Some("Shared Title".into()),
                artist_credit: Some("Shared Artist".into()),
                ..Default::default()
            }),
            provider: "a".into(),
            ..Default::default()
        })),
    }));
    agg.register_identifier(Box::new(MockIdentifier {
        name: "b",
        outcome: Ok(Some(ProviderResult {
            album: Some(AlbumMeta {
                title: Some("Shared Title".into()),
                artist_credit: Some("Shared Artist".into()),
                year: Some(2024),
                ..Default::default()
            }),
            provider: "b".into(),
            ..Default::default()
        })),
    }));
    let outcome = agg.identify(&default_toc(), &discid_ids(), &Credentials::new());
    assert!(outcome.any_match);
    assert_eq!(outcome.merged.album.title.as_deref(), Some("Shared Title"));
    assert_eq!(outcome.merged.album.year, Some(2024));
    assert!(outcome.errors.is_empty());
    assert!(outcome.merged.disagreements.is_empty());
}

#[test]
fn identify_unmatched_returns_any_match_false() {
    let mut agg = Aggregator::new();
    agg.register_identifier(Box::new(MockIdentifier {
        name: "a",
        outcome: Ok(None),
    }));
    agg.register_identifier(Box::new(MockIdentifier {
        name: "b",
        outcome: Ok(None),
    }));
    let outcome = agg.identify(&default_toc(), &discid_ids(), &Credentials::new());
    assert!(!outcome.any_match);
    assert!(outcome.errors.is_empty());
}

#[test]
fn identify_error_in_one_provider_surfaces_in_errors_list() {
    let mut agg = Aggregator::new();
    agg.register_identifier(Box::new(MockIdentifier {
        name: "a",
        outcome: Err("boom"),
    }));
    agg.register_identifier(Box::new(MockIdentifier {
        name: "b",
        outcome: Ok(Some(ProviderResult {
            album: Some(AlbumMeta {
                title: Some("Title".into()),
                ..Default::default()
            }),
            provider: "b".into(),
            ..Default::default()
        })),
    }));
    let outcome = agg.identify(&default_toc(), &discid_ids(), &Credentials::new());
    assert!(outcome.any_match, "b's Ok must still produce a match");
    assert_eq!(outcome.errors.len(), 1);
    assert_eq!(outcome.errors[0].0, "a");
}

#[test]
fn lookup_assets_dedupes_identical_urls() {
    let mut agg = Aggregator::new();
    agg.register_asset_provider(Box::new(MockAssetProvider {
        name: "caa",
        candidates: vec![front_cover("caa", "https://example.com/art.jpg")],
    }));
    agg.register_asset_provider(Box::new(MockAssetProvider {
        name: "itunes",
        candidates: vec![front_cover("itunes", "https://example.com/art.jpg")],
    }));
    let album = AlbumMeta::default();
    let release = ReleaseMeta::default();
    let ids = DiscIds::default();
    let creds = Credentials::new();
    let ctx = AssetLookupCtx {
        album: &album,
        release: &release,
        ids: &ids,
        creds: &creds,
    };
    let outcome = agg.lookup_assets(&ctx);
    assert_eq!(outcome.candidates.len(), 1);
    assert_eq!(outcome.candidates[0].provider, "caa", "priority should win");
}

#[test]
fn lookup_assets_keeps_distinct_urls() {
    let mut agg = Aggregator::new();
    agg.register_asset_provider(Box::new(MockAssetProvider {
        name: "caa",
        candidates: vec![front_cover("caa", "https://a.example.com/a.jpg")],
    }));
    agg.register_asset_provider(Box::new(MockAssetProvider {
        name: "itunes",
        candidates: vec![front_cover("itunes", "https://b.example.com/b.jpg")],
    }));
    let album = AlbumMeta::default();
    let release = ReleaseMeta::default();
    let ids = DiscIds::default();
    let creds = Credentials::new();
    let ctx = AssetLookupCtx {
        album: &album,
        release: &release,
        ids: &ids,
        creds: &creds,
    };
    let outcome = agg.lookup_assets(&ctx);
    assert_eq!(outcome.candidates.len(), 2);
}
