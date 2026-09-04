//! Aggregator-level asset tests.

use phono_junk_core::DiscIds;
use phono_junk_identify::{
    Aggregator, AlbumMeta, AssetCandidate, AssetConfidence, AssetLookupCtx, AssetProvider,
    AssetType, Credentials, ProviderError, ReleaseMeta,
};
use url::Url;

struct MockAssetProvider {
    name: &'static str,
    candidates: Vec<AssetCandidate>,
}

impl AssetProvider for MockAssetProvider {
    fn name(&self) -> &'static str {
        self.name
    }
    fn asset_types(&self) -> &'static [AssetType] {
        &[AssetType::FrontCover]
    }
    fn lookup_art(&self, _ctx: &AssetLookupCtx<'_>) -> Result<Vec<AssetCandidate>, ProviderError> {
        Ok(self.candidates.clone())
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
