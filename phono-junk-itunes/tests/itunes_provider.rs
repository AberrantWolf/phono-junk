//! iTunes Search API provider tests. Parse-only + one fast-path test through
//! `AssetProvider::lookup_art`. Live network test is `#[ignore]`-gated.

use phono_junk_core::DiscIds;
use phono_junk_identify::{
    AlbumMeta, AssetConfidence, AssetLookupCtx, AssetProvider, AssetType, Credentials, ReleaseMeta,
};
use phono_junk_itunes::{ITunesProvider, parse_search_response};

const FIXTURE_EXACT: &[u8] = include_bytes!("fixtures/search_exact_hit.json");
const FIXTURE_EMPTY: &[u8] = include_bytes!("fixtures/search_no_results.json");

#[test]
fn parses_exact_hit_and_rewrites_url() {
    let candidates = parse_search_response(FIXTURE_EXACT).expect("parse ok");
    assert_eq!(candidates.len(), 1);
    let c = &candidates[0];
    assert_eq!(c.asset_type, AssetType::FrontCover);
    assert_eq!(c.confidence, AssetConfidence::Fuzzy);
    assert_eq!(c.provider, "itunes");
    assert!(
        c.source_url.as_str().ends_with("1000x1000bb.jpg"),
        "expected 1000x1000 rewrite, got {}",
        c.source_url
    );
    assert_eq!(c.width, Some(1000));
    assert_eq!(c.height, Some(1000));
}

#[test]
fn empty_results_returns_empty_candidates() {
    let candidates = parse_search_response(FIXTURE_EMPTY).expect("parse ok");
    assert!(candidates.is_empty());
}

#[test]
fn invalid_json_maps_to_parse_error() {
    let err = parse_search_response(b"{").expect_err("bad JSON");
    assert!(
        matches!(err, phono_junk_identify::ProviderError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
}

#[test]
fn provider_without_album_title_skips_lookup() {
    let provider =
        ITunesProvider::new("phono-junk-tests/0.1 (+tests@example.invalid)").expect("construct");
    let ids = DiscIds::default();
    let creds = Credentials::new();
    let release = ReleaseMeta::default();
    let ctx = AssetLookupCtx {
        album: None,
        release: &release,
        ids: &ids,
        creds: &creds,
    };
    let candidates = provider.lookup_art(&ctx).expect("ok");
    assert!(candidates.is_empty());
}

#[test]
fn provider_without_artist_or_title_skips_lookup() {
    let provider =
        ITunesProvider::new("phono-junk-tests/0.1 (+tests@example.invalid)").expect("construct");
    let ids = DiscIds::default();
    let creds = Credentials::new();
    let release = ReleaseMeta::default();
    let album_no_title = AlbumMeta {
        title: None,
        artist_credit: Some("Artist".into()),
        ..Default::default()
    };
    let ctx = AssetLookupCtx {
        album: Some(&album_no_title),
        release: &release,
        ids: &ids,
        creds: &creds,
    };
    assert!(provider.lookup_art(&ctx).expect("ok").is_empty());
}

/// Live smoke test against itunes.apple.com. Gated behind `#[ignore]`.
#[test]
#[ignore = "live network"]
fn live_lookup_against_itunes() {
    let provider =
        ITunesProvider::new("phono-junk-tests/0.1 ( live-smoke-test / tests@example.invalid )")
            .expect("construct");
    let ids = DiscIds::default();
    let creds = Credentials::new();
    let release = ReleaseMeta::default();
    let album = AlbumMeta {
        title: Some("Discovery".to_string()),
        artist_credit: Some("Daft Punk".to_string()),
        ..Default::default()
    };
    let ctx = AssetLookupCtx {
        album: Some(&album),
        release: &release,
        ids: &ids,
        creds: &creds,
    };
    let candidates = provider.lookup_art(&ctx).expect("live lookup");
    assert!(!candidates.is_empty(), "expected at least one iTunes hit");
}
