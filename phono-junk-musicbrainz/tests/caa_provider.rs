//! Cover Art Archive `/release/<mbid>` provider tests.
//!
//! Parse-path tests run against hand-rolled fixtures under `tests/fixtures/`.
//! The fast-path (no release MBID → empty) is exercised through the
//! `AssetProvider` impl. Live network test is `#[ignore]`-gated.

use phono_junk_core::DiscIds;
use phono_junk_identify::{
    AssetLookupCtx, AssetProvider, AssetType, Credentials, ReleaseMeta,
};
use phono_junk_musicbrainz::{CoverArtArchiveProvider, parse_caa_response};

const FIXTURE_FRONT_ONLY: &[u8] = include_bytes!("fixtures/caa_front_only.json");
const FIXTURE_FRONT_BACK_BOOKLET: &[u8] =
    include_bytes!("fixtures/caa_front_back_booklet.json");

#[test]
fn parses_front_only_fixture() {
    let candidates = parse_caa_response(FIXTURE_FRONT_ONLY).expect("parse ok");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].asset_type, AssetType::FrontCover);
    assert_eq!(candidates[0].provider, "cover-art-archive");
}

#[test]
fn front_back_booklet_fixture_classifies_each_image() {
    let candidates = parse_caa_response(FIXTURE_FRONT_BACK_BOOKLET).expect("parse ok");
    assert_eq!(candidates.len(), 4);
    let types: Vec<AssetType> = candidates.iter().map(|c| c.asset_type).collect();
    assert_eq!(
        types,
        vec![
            AssetType::FrontCover,
            AssetType::BackCover,
            AssetType::Booklet, // types=["Booklet","Front"] — Booklet wins
            AssetType::Booklet,
        ]
    );
}

#[test]
fn empty_images_returns_empty_candidates() {
    let candidates = parse_caa_response(br#"{"images":[]}"#).expect("parse ok");
    assert!(candidates.is_empty());
}

#[test]
fn invalid_json_maps_to_parse_error() {
    let err = parse_caa_response(b"not json").expect_err("bad JSON");
    assert!(
        matches!(err, phono_junk_identify::ProviderError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
}

#[test]
fn provider_with_no_release_mbid_returns_empty() {
    let provider = CoverArtArchiveProvider::new("phono-junk-tests/0.1 (+tests@example.invalid)")
        .expect("construct provider");
    let ids = DiscIds::default();
    let creds = Credentials::new();
    let release = ReleaseMeta::default(); // mbid = None
    let ctx = AssetLookupCtx {
        album: None,
        release: &release,
        ids: &ids,
        creds: &creds,
    };
    let candidates = provider.lookup_art(&ctx).expect("ok");
    assert!(candidates.is_empty());
}

/// Live smoke test against coverartarchive.org. Gated behind `#[ignore]`.
/// Override with `PHONO_CAA_LIVE_MBID` if default is removed.
#[test]
#[ignore = "live network"]
fn live_lookup_against_caa() {
    // Default MBID points at a release that has been stable on MB for years.
    // Override via `PHONO_CAA_LIVE_MBID` if the release changes.
    let mbid = std::env::var("PHONO_CAA_LIVE_MBID")
        .unwrap_or_else(|_| "76df3287-6cda-33eb-8e9a-044b5e15ffdd".to_string());
    let provider = CoverArtArchiveProvider::new(
        "phono-junk-tests/0.1 ( live-smoke-test / tests@example.invalid )",
    )
    .expect("construct provider");
    let ids = DiscIds::default();
    let creds = Credentials::new();
    let release = ReleaseMeta {
        mbid: Some(mbid),
        ..Default::default()
    };
    let ctx = AssetLookupCtx {
        album: None,
        release: &release,
        ids: &ids,
        creds: &creds,
    };
    let candidates = provider.lookup_art(&ctx).expect("live lookup");
    // Many MB releases have CAA art; if this specific one is missing,
    // override the MBID via env var. Empty is not a test failure per se
    // (the 404 path returns empty by design), so we just smoke-assert
    // the request completed without error.
    let _ = candidates;
}
