//! MusicBrainz `/ws/2/discid/<id>` provider tests.
//!
//! Parse-path tests run against hand-rolled fixtures under `tests/fixtures/`
//! (see the README there). One `#[ignore]`-gated live-network test hits
//! musicbrainz.org and is only run with `cargo test -- --ignored`.

use phono_junk_core::{DiscIds, Toc};
use phono_junk_identify::{Credentials, IdentificationProvider};
use phono_junk_musicbrainz::{MusicBrainzProvider, parse_discid_response};

const FIXTURE_SINGLE: &[u8] = include_bytes!("fixtures/discid_single_release.json");
const FIXTURE_NO_MATCH: &[u8] = include_bytes!("fixtures/discid_no_match.json");
const FIXTURE_MULTI: &[u8] = include_bytes!("fixtures/discid_multi_release.json");

fn toc_stub() -> Toc {
    Toc {
        first_track: 1,
        last_track: 3,
        leadout_sector: 0,
        track_offsets: vec![0, 0, 0],
    }
}

#[test]
fn parses_single_release_fixture() {
    let result = parse_discid_response(FIXTURE_SINGLE)
        .expect("parse ok")
        .expect("some result");

    let album = result.album.expect("album populated");
    assert_eq!(album.title.as_deref(), Some("Testing The Waters"));
    assert_eq!(album.artist_credit.as_deref(), Some("Simon & Garfunkel"));
    assert_eq!(album.year, Some(2001));
    assert_eq!(
        album.mbid.as_deref(),
        Some("22222222-2222-2222-2222-222222222222")
    );

    let release = result.release.expect("release populated");
    assert_eq!(release.country.as_deref(), Some("FR"));
    assert_eq!(release.label.as_deref(), Some("Fixture Records"));
    assert_eq!(release.catalog_number.as_deref(), Some("TEST-001"));
    assert_eq!(release.barcode.as_deref(), Some("724381045527"));
    assert_eq!(
        release.mbid.as_deref(),
        Some("11111111-1111-1111-1111-111111111111")
    );

    assert_eq!(result.tracks.len(), 3);
    assert_eq!(result.tracks[0].title.as_deref(), Some("First Track"));
    assert_eq!(result.tracks[2].title.as_deref(), Some("Final Track"));
    // 180000 ms -> 180000 * 75 / 1000 = 13500 frames
    assert_eq!(result.tracks[0].length_frames, Some(13_500));

    assert_eq!(result.provider, "musicbrainz");
    assert!(result.raw_response.is_some());
}

#[test]
fn no_match_fixture_returns_none() {
    let result = parse_discid_response(FIXTURE_NO_MATCH).expect("parse ok");
    assert!(result.is_none());
}

#[test]
fn multi_release_picks_first() {
    let result = parse_discid_response(FIXTURE_MULTI)
        .expect("parse ok")
        .expect("some result");
    let release = result.release.expect("release");
    assert_eq!(
        release.mbid.as_deref(),
        Some("33333333-3333-3333-3333-333333333333")
    );
    assert_eq!(release.country.as_deref(), Some("GB"));
    assert_eq!(result.album.and_then(|a| a.year), Some(1999));
}

#[test]
fn invalid_json_maps_to_parse_error() {
    let err = parse_discid_response(b"{").expect_err("bad JSON");
    assert!(
        matches!(err, phono_junk_identify::ProviderError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
}

#[test]
fn provider_with_no_mb_discid_skips_lookup() {
    let provider = MusicBrainzProvider::new("phono-junk-tests/0.1 (+tests@example.invalid)")
        .expect("construct provider");
    let result = provider
        .lookup(&toc_stub(), &DiscIds::default(), &Credentials::new())
        .expect("lookup ok when no DiscID");
    assert!(result.is_none());
}

/// Live smoke test against musicbrainz.org. Gated behind `#[ignore]`.
/// Override the DiscID with `PHONO_MB_LIVE_DISCID` if the default is removed.
#[test]
#[ignore = "live network"]
fn live_lookup_against_musicbrainz() {
    let discid = std::env::var("PHONO_MB_LIVE_DISCID")
        .unwrap_or_else(|_| "arIS30RPWowvwNEqsqdDnZzDGhk-".to_string());
    let provider = MusicBrainzProvider::new(
        "phono-junk-tests/0.1 ( live-smoke-test / tests@example.invalid )",
    )
    .expect("construct provider");
    let ids = DiscIds {
        mb_discid: Some(discid),
        ..Default::default()
    };
    let result = provider
        .lookup(&toc_stub(), &ids, &Credentials::new())
        .expect("live lookup");
    let result = result.expect("DiscID should match a real release");
    assert!(result.album.and_then(|a| a.title).is_some());
}
