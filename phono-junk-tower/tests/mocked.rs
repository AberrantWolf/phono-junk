//! End-to-end integration tests against a mocked Tower MDB. Exercises
//! the full search→release pipeline through `IdentificationProvider`
//! and `AssetProvider`, plus cache behaviour on repeated calls.

use httpmock::prelude::*;
use phono_junk_core::{DiscIds, Toc};
use phono_junk_identify::{
    AlbumMeta, AssetLookupCtx, AssetProvider, AssetType, Credentials, HttpClient,
    IdentificationProvider, ReleaseMeta,
};
use phono_junk_tower::cache::ResponseCache;
use phono_junk_tower::TowerProvider;
use tempfile::TempDir;

const SEARCH_HTML: &[u8] = include_bytes!("fixtures/search-barcode-hit.html");
const RELEASE_HTML: &[u8] = include_bytes!("fixtures/release-10054881.html");

fn toc_stub() -> Toc {
    Toc {
        first_track: 1,
        last_track: 11,
        leadout_sector: 0,
        track_offsets: vec![0; 11],
    }
}

fn ids_with_barcode(bc: &str) -> DiscIds {
    DiscIds {
        barcode: Some(bc.into()),
        ..DiscIds::default()
    }
}

fn client() -> HttpClient {
    HttpClient::builder()
        .user_agent("phono-junk-tower-test/0.1")
        .build()
        .unwrap()
}

/// Wire up the mock server with both endpoints.
fn mount(server: &MockServer) -> (httpmock::Mock<'_>, httpmock::Mock<'_>) {
    let search = server.mock(|when, then| {
        when.method(GET).path("/search/075992665629/0");
        then.status(200)
            .header("content-type", "text/html; charset=utf-8")
            .body(SEARCH_HTML);
    });
    let release = server.mock(|when, then| {
        when.method(GET).path("/release/10054881");
        then.status(200)
            .header("content-type", "text/html; charset=utf-8")
            .body(RELEASE_HTML);
    });
    (search, release)
}

#[test]
fn lookup_round_trips_search_and_release() {
    let server = MockServer::start();
    let (search_mock, release_mock) = mount(&server);
    let dir = TempDir::new().unwrap();
    let cache = ResponseCache::with_root(dir.path().into());
    let provider = TowerProvider::with_client_and_cache(client(), cache)
        .with_base_url(server.base_url());

    let result = provider
        .lookup(&toc_stub(), &ids_with_barcode("075992665629"), &Credentials::new())
        .unwrap()
        .expect("fixture release should be returned");

    assert_eq!(result.provider, "tower");
    let album = result.album.as_ref().expect("album meta present");
    assert_eq!(album.title.as_deref(), Some("Fourplay"));
    assert_eq!(album.artist_credit.as_deref(), Some("Fourplay"));
    assert_eq!(album.year, Some(1994));

    let release = result.release.as_ref().expect("release meta present");
    assert_eq!(release.label.as_deref(), Some("Wea"));
    assert_eq!(release.catalog_number.as_deref(), Some("26656"));
    assert_eq!(release.barcode.as_deref(), Some("075992665629"));

    assert_eq!(result.tracks.len(), 11);
    assert_eq!(result.tracks[0].title.as_deref(), Some("Bali Run"));
    assert_eq!(result.cover_art_urls.len(), 1);
    assert!(result.cover_art_urls[0].contains("cdn.tower.jp"));

    // Raw response forensic payload is populated with the Tower-specific
    // extras we don't promote to the generic schema.
    let raw = result.raw_response.expect("raw_response populated");
    assert_eq!(raw.get("release_id").and_then(|v| v.as_u64()), Some(10054881));
    assert!(raw.get("version_list").and_then(|v| v.as_array()).is_some());

    search_mock.assert();
    release_mock.assert();
}

#[test]
fn lookup_art_returns_front_cover() {
    let server = MockServer::start();
    let (_s, _r) = mount(&server);
    let dir = TempDir::new().unwrap();
    let cache = ResponseCache::with_root(dir.path().into());
    let provider = TowerProvider::with_client_and_cache(client(), cache)
        .with_base_url(server.base_url());

    let album = AlbumMeta::default();
    let release = ReleaseMeta::default();
    let ids = ids_with_barcode("075992665629");
    let creds = Credentials::new();
    let ctx = AssetLookupCtx {
        album: &album,
        release: &release,
        ids: &ids,
        creds: &creds,
    };
    let assets = provider.lookup_art(&ctx).unwrap();
    assert_eq!(assets.len(), 1);
    assert_eq!(assets[0].asset_type, AssetType::FrontCover);
    assert_eq!(assets[0].provider, "tower");
    assert!(assets[0].source_url.as_str().contains("cdn.tower.jp"));
}

#[test]
fn cache_absorbs_second_call() {
    let server = MockServer::start();
    let search = server.mock(|when, then| {
        when.method(GET).path("/search/075992665629/0");
        then.status(200)
            .header("content-type", "text/html; charset=utf-8")
            .body(SEARCH_HTML);
    });
    let release = server.mock(|when, then| {
        when.method(GET).path("/release/10054881");
        then.status(200)
            .header("content-type", "text/html; charset=utf-8")
            .body(RELEASE_HTML);
    });
    let dir = TempDir::new().unwrap();
    let cache = ResponseCache::with_root(dir.path().into());
    let provider = TowerProvider::with_client_and_cache(client(), cache)
        .with_base_url(server.base_url());

    // First call — fills the cache.
    provider
        .lookup(&toc_stub(), &ids_with_barcode("075992665629"), &Credentials::new())
        .unwrap();
    // Second call — should not hit the network at all.
    provider
        .lookup(&toc_stub(), &ids_with_barcode("075992665629"), &Credentials::new())
        .unwrap();

    search.assert_hits(1);
    release.assert_hits(1);
}

#[test]
fn empty_search_results_yields_ok_none() {
    let server = MockServer::start();
    let empty_html = b"<!DOCTYPE html><html><body><div class=\"container\"><div id=\"observerTarget\"></div></div></body></html>";
    let _ = server.mock(|when, then| {
        when.method(GET).path("/search/0000000000000/0");
        then.status(200)
            .header("content-type", "text/html; charset=utf-8")
            .body(empty_html);
    });
    let provider = TowerProvider::with_client(client()).with_base_url(server.base_url());
    let r = provider
        .lookup(&toc_stub(), &ids_with_barcode("0000000000000"), &Credentials::new())
        .unwrap();
    assert!(r.is_none());
}
