use std::time::Duration;

use governor::Quota;
use governor::clock::{Clock, FakeRelativeClock};
use httpmock::prelude::*;
use nonzero_ext::nonzero;

use crate::http::{HttpClient, HttpError};

fn elapsed(clock: &FakeRelativeClock) -> Duration {
    Duration::from(clock.now())
}

#[test]
fn build_without_user_agent_fails() {
    let res = HttpClient::builder().build();
    assert!(matches!(res, Err(HttpError::MissingUserAgent)));
}

#[test]
fn get_injects_user_agent() {
    let server = MockServer::start();
    let ua = "phono-junk-test/0.1 ( test@example.com )";
    let mock = server.mock(|when, then| {
        when.method(GET).path("/x").header("user-agent", ua);
        then.status(200).body("ok");
    });

    let client = HttpClient::builder().user_agent(ua).build().unwrap();
    let resp = client.get(&server.url("/x")).unwrap();

    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, b"ok");
    mock.assert();
}

#[test]
fn status_429_maps_to_server_rate_limited() {
    let server = MockServer::start();
    let _mock = server.mock(|when, then| {
        when.method(GET).path("/slow");
        then.status(429).body("nope");
    });

    let client = HttpClient::builder()
        .user_agent("test/1.0")
        .build()
        .unwrap();
    let err = client.get(&server.url("/slow")).unwrap_err();
    assert!(matches!(err, HttpError::ServerRateLimited));
}

#[test]
fn per_host_bucket_waits_deterministically_with_fake_clock() {
    // Register host at 1 req/sec. Fire three requests and verify the fake
    // clock advanced by ~2s total — two waits of 1s each after the initial
    // burst token is consumed.
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path("/tick");
        then.status(200).body("t");
    });

    let clock = FakeRelativeClock::default();
    let t0 = elapsed(&clock);

    let client = HttpClient::builder()
        .user_agent("test/1.0")
        .fake_host_quota("127.0.0.1", Quota::per_second(nonzero!(1u32)), clock.clone())
        .build()
        .unwrap();

    let url = server.url("/tick");
    client.get(&url).unwrap();
    client.get(&url).unwrap();
    client.get(&url).unwrap();

    let elapsed = elapsed(&clock) - t0;
    // Two refills of 1s each. Small tolerance for internal governor rounding
    // (should be exact, but avoid brittleness if the crate changes).
    assert!(
        elapsed >= Duration::from_millis(1990) && elapsed <= Duration::from_millis(2010),
        "expected ~2s of fake-clock advancement, got {elapsed:?}"
    );
    mock.assert_hits(3);
}

#[test]
fn per_host_buckets_are_independent() {
    // Register two separate host strings that both resolve to the loopback
    // interface ("localhost" and "127.0.0.1"). Exhaust one bucket; requests
    // to the other should not wait.
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path("/ping");
        then.status(200);
    });

    let clock = FakeRelativeClock::default();
    let t0 = elapsed(&clock);

    let client = HttpClient::builder()
        .user_agent("test/1.0")
        .fake_host_quota("127.0.0.1", Quota::per_second(nonzero!(1u32)), clock.clone())
        .fake_host_quota("localhost", Quota::per_second(nonzero!(1u32)), clock.clone())
        .build()
        .unwrap();

    // Exhaust 127.0.0.1 bucket (one token).
    client.get(&server.url("/ping")).unwrap();
    let after_first = elapsed(&clock) - t0;

    // Hit via localhost — should not wait since its bucket is full.
    let localhost_url = format!("http://localhost:{}/ping", server.port());
    client.get(&localhost_url).unwrap();
    let after_second = elapsed(&clock) - t0;

    assert_eq!(
        after_first, after_second,
        "crossing to an independent host bucket should not advance the clock"
    );
    mock.assert_hits(2);
}

#[test]
fn unregistered_host_passes_through_without_limiting() {
    let server = MockServer::start();
    let mock = server.mock(|when, then| {
        when.method(GET).path("/free");
        then.status(200);
    });

    let client = HttpClient::builder()
        .user_agent("test/1.0")
        .build()
        .unwrap();

    for _ in 0..5 {
        client.get(&server.url("/free")).unwrap();
    }
    mock.assert_hits(5);
}
