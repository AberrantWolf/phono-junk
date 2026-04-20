//! Disk-cache behaviour: write-through, TTL expiry, 5xx bypass.

use std::cell::Cell;
use std::time::Duration;

use phono_junk_identify::{HttpError, HttpResponse};
use tempfile::TempDir;

use super::*;

fn resp(status: u16, body: &[u8]) -> HttpResponse {
    HttpResponse {
        status,
        body: body.to_vec(),
        content_type: Some("text/html; charset=utf-8".into()),
    }
}

fn cache_under(dir: &TempDir) -> ResponseCache {
    ResponseCache::with_root(dir.path().to_path_buf())
}

#[test]
fn write_through_then_read_from_cache() {
    let dir = TempDir::new().unwrap();
    let cache = cache_under(&dir);
    let hits = Cell::new(0u32);
    let do_fetch = || {
        hits.set(hits.get() + 1);
        Ok(resp(200, b"<html>ok</html>"))
    };
    let a = cache
        .get_or_fetch("https://example.test/a", CacheKind::Release, do_fetch)
        .unwrap();
    assert_eq!(a.status, 200);
    assert_eq!(hits.get(), 1);
    // Second call should be served from cache; closure not invoked.
    let b = cache
        .get_or_fetch("https://example.test/a", CacheKind::Release, do_fetch)
        .unwrap();
    assert_eq!(b.body, a.body);
    assert_eq!(hits.get(), 1, "second call must hit the cache");
}

#[test]
fn expired_entry_triggers_refetch() {
    let dir = TempDir::new().unwrap();
    // Zero TTL — anything written is instantly stale.
    let cache = cache_under(&dir).with_ttls(Duration::from_secs(0), Duration::from_secs(0));
    let hits = Cell::new(0u32);
    let do_fetch = || {
        hits.set(hits.get() + 1);
        Ok(resp(200, b"<html>v1</html>"))
    };
    cache
        .get_or_fetch("https://example.test/a", CacheKind::Search, do_fetch)
        .unwrap();
    // Wait a tick to ensure mtime delta is observable.
    std::thread::sleep(Duration::from_millis(20));
    cache
        .get_or_fetch("https://example.test/a", CacheKind::Search, do_fetch)
        .unwrap();
    assert_eq!(hits.get(), 2, "expired entry must trigger a re-fetch");
}

#[test]
fn fivehundred_response_is_not_cached() {
    let dir = TempDir::new().unwrap();
    let cache = cache_under(&dir);
    let hits = Cell::new(0u32);
    let do_fetch_500 = || {
        hits.set(hits.get() + 1);
        Ok(resp(503, b"oops"))
    };
    cache
        .get_or_fetch("https://example.test/x", CacheKind::Release, do_fetch_500)
        .unwrap();
    cache
        .get_or_fetch("https://example.test/x", CacheKind::Release, do_fetch_500)
        .unwrap();
    assert_eq!(hits.get(), 2, "5xx responses must bypass the cache");
}

#[test]
fn fourohfour_is_cached() {
    let dir = TempDir::new().unwrap();
    let cache = cache_under(&dir);
    let hits = Cell::new(0u32);
    let do_fetch_404 = || {
        hits.set(hits.get() + 1);
        Ok(resp(404, b"nope"))
    };
    cache
        .get_or_fetch("https://example.test/y", CacheKind::Search, do_fetch_404)
        .unwrap();
    cache
        .get_or_fetch("https://example.test/y", CacheKind::Search, do_fetch_404)
        .unwrap();
    assert_eq!(hits.get(), 1, "404 should be cached under the search TTL");
}

#[test]
fn transport_errors_bypass_cache() {
    let dir = TempDir::new().unwrap();
    let cache = cache_under(&dir);
    let hits = Cell::new(0u32);
    let do_err = || -> Result<HttpResponse, HttpError> {
        hits.set(hits.get() + 1);
        Err(HttpError::Timeout)
    };
    assert!(
        cache
            .get_or_fetch("https://example.test/z", CacheKind::Release, do_err)
            .is_err()
    );
    assert!(
        cache
            .get_or_fetch("https://example.test/z", CacheKind::Release, do_err)
            .is_err()
    );
    assert_eq!(hits.get(), 2, "transport errors must not be cached");
}
