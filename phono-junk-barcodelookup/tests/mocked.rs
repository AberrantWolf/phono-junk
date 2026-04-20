//! Integration tests for the Barcode Lookup provider against a mocked
//! HTTP server. The provider's URL builder targets the real
//! `api.barcodelookup.com` host; these tests drive the lower-level
//! `HttpClient::get` + `parse_search_response` pair against a mock,
//! which is the same wire path the provider exercises.
//!
//! Parallels `phono-junk-discogs/tests/mocked.rs`.

use httpmock::prelude::*;
use phono_junk_barcodelookup::BarcodelookupProvider;
use phono_junk_core::{DiscIds, Toc};
use phono_junk_identify::{Credentials, HttpClient, IdentificationProvider, ProviderError};

const FIXTURE: &[u8] = include_bytes!("fixtures/search_barcode_hit.json");

fn default_toc() -> Toc {
    Toc {
        first_track: 1,
        last_track: 1,
        leadout_sector: 0,
        track_offsets: vec![0],
    }
}

fn ids_with_barcode(bc: &str) -> DiscIds {
    DiscIds {
        barcode: Some(bc.into()),
        ..DiscIds::default()
    }
}

fn test_client() -> HttpClient {
    HttpClient::builder()
        .user_agent("phono-junk-test/0.1")
        .build()
        .unwrap()
}

#[test]
fn round_trips_barcode_hit_through_client_and_parser() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET)
            .path("/v3/products")
            .query_param("barcode", "0123456789012")
            .query_param("key", "integration-secret")
            .query_param("formatted", "y")
            .header("user-agent", "phono-junk-test/0.1");
        then.status(200)
            .header("content-type", "application/json")
            .body(FIXTURE);
    });

    let client = test_client();
    let url = server.url(
        "/v3/products?barcode=0123456789012&formatted=y&key=integration-secret",
    );
    let resp = client.get(&url).unwrap();
    assert_eq!(resp.status, 200);

    let parsed = phono_junk_barcodelookup::parse_search_response(&resp.body)
        .unwrap()
        .expect("barcode hit should return Some");
    assert_eq!(
        parsed.album.as_ref().and_then(|a| a.title.as_deref()),
        Some("Test Album"),
    );
    assert_eq!(
        parsed.release.as_ref().and_then(|r| r.barcode.as_deref()),
        Some("0123456789012"),
    );
    assert_eq!(parsed.cover_art_urls.len(), 2);
    mock.assert();

    // Secret must not leak via the response's debug output. Headers
    // aren't stored on the response today; this guards against a
    // future regression that would echo the request URL back.
    let dbg = format!("{:?}", resp);
    assert!(!dbg.contains("integration-secret"), "resp debug leaked: {dbg}");
}

#[test]
fn server_401_maps_to_status_ready_for_auth_mapping() {
    // The provider's own lookup() points at the real API host, so we
    // exercise the 401 → Auth mapping through the status-code match
    // directly. Any 401 the transport returns is surfaced as
    // `ProviderError::Auth` — documented in the provider source.
    let server = MockServer::start();
    let _mock = server.mock(|when, then| {
        when.method(GET).path("/v3/products");
        then.status(401).body(r#"{"message":"invalid api key"}"#);
    });

    let client = test_client();
    let resp = client
        .get(&server.url("/v3/products?barcode=x&key=bad"))
        .unwrap();
    assert_eq!(resp.status, 401);
    // The provider maps this to ProviderError::Auth — covered by the
    // match arm in lib.rs. Direct status-code assertion here documents
    // the wire shape the provider depends on.
}

#[test]
fn server_404_returns_not_found_status_for_ok_none_mapping() {
    // 404 from the API indicates "no product with that barcode" and
    // the provider maps it to `Ok(None)` (disc is unidentified by this
    // source — a first-class outcome, not an error).
    let server = MockServer::start();
    let _mock = server.mock(|when, then| {
        when.method(GET).path("/v3/products");
        then.status(404).body(r#"{"message":"not found"}"#);
    });

    let client = test_client();
    let resp = client
        .get(&server.url("/v3/products?barcode=x&key=ok"))
        .unwrap();
    assert_eq!(resp.status, 404);
}

#[test]
fn missing_token_returns_missing_credential_error() {
    let provider = BarcodelookupProvider::new();
    let creds = Credentials::new();
    match provider.lookup(&default_toc(), &ids_with_barcode("0123456789012"), &creds) {
        Err(ProviderError::MissingCredential("barcodelookup")) => {}
        other => panic!("expected MissingCredential(barcodelookup), got {other:?}"),
    }
}
