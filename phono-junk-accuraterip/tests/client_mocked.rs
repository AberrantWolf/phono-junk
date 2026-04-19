//! Offline HTTP coverage for [`AccurateRipClient`]. Binds a local
//! `httpmock` server and drives [`AccurateRipClient::fetch_at_url`]
//! against it to exercise the 200 / 404 / 500 branches without the
//! real network.
//!
//! The wire URL produced by [`phono_junk_accuraterip::dbar_url`] is
//! validated separately in `url_builder.rs`; this file only covers the
//! response-dispatch layer.

use httpmock::prelude::*;
use phono_junk_accuraterip::{AccurateRipClient, AccurateRipError, ExpectedCrc};
use phono_junk_identify::HttpClient;

fn client() -> AccurateRipClient {
    let http = HttpClient::builder()
        .user_agent("phono-junk-test/0.1")
        .build()
        .unwrap();
    AccurateRipClient::with_client(http)
}

/// Smallest valid dBAR: 1 response, 1 track entry.
fn minimal_dbar_bytes() -> Vec<u8> {
    let mut v = Vec::new();
    v.push(1u8); // track_count
    v.extend_from_slice(&0x0008_4264u32.to_le_bytes());
    v.extend_from_slice(&0x001c_c184u32.to_le_bytes());
    v.extend_from_slice(&0x1911_7f03u32.to_le_bytes());
    v.push(7u8); // confidence
    v.extend_from_slice(&0xdead_beefu32.to_le_bytes()); // v1
    v.extend_from_slice(&0xcafe_f00du32.to_le_bytes()); // v2
    v
}

const PATH: &str = "/accuraterip/4/6/2/dBAR-001-00084264-001cc184-19117f03.bin";

#[test]
fn status_200_parses_body() {
    let server = MockServer::start();
    let body = minimal_dbar_bytes();
    let mock = server.mock(|when, then| {
        when.method(GET).path(PATH);
        then.status(200).body(body.clone());
    });

    let parsed = client().fetch_at_url(&server.url(PATH)).unwrap().unwrap();
    mock.assert();
    assert_eq!(parsed.responses.len(), 1);
    assert_eq!(
        parsed.responses[0].tracks[0],
        ExpectedCrc {
            confidence: 7,
            v1: 0xdead_beef,
            v2: 0xcafe_f00d,
        }
    );
}

#[test]
fn status_404_yields_none() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path(PATH);
        then.status(404);
    });

    let result = client().fetch_at_url(&server.url(PATH)).unwrap();
    mock.assert();
    assert!(result.is_none(), "expected Ok(None) for 404");
}

#[test]
fn status_500_errors_with_code_in_message() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path(PATH);
        then.status(500).body("boom");
    });

    let err = client().fetch_at_url(&server.url(PATH)).unwrap_err();
    mock.assert();
    match err {
        AccurateRipError::Parse(msg) => assert!(msg.contains("500"), "msg: {msg}"),
        other => panic!("expected Parse error, got {other:?}"),
    }
}

#[test]
fn status_200_with_bad_body_surfaces_parse_error() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path(PATH);
        // Too short for a header — should surface as Parse.
        then.status(200).body([0x03u8, 0x00]);
    });

    let err = client().fetch_at_url(&server.url(PATH)).unwrap_err();
    mock.assert();
    assert!(matches!(err, AccurateRipError::Parse(_)));
}
