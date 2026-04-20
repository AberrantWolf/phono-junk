//! Parser tests for `/search/{barcode}/0` result pages.

use super::*;

const FIXTURE_HIT: &[u8] = include_bytes!("../../tests/fixtures/search-barcode-hit.html");

#[test]
fn parse_search_page_returns_single_release_hit() {
    let hits = parse_search_page(FIXTURE_HIT).unwrap();
    assert_eq!(hits.len(), 1, "fixture should contain exactly one Release hit");
    let hit = &hits[0];
    assert_eq!(hit.release_id, 10054881);
    assert_eq!(hit.slug, "Fourplay");
    assert_eq!(hit.title, "Fourplay");
    assert_eq!(hit.artist_id, Some(10017811));
    assert_eq!(hit.artist_name.as_deref(), Some("Fourplay"));
    assert_eq!(hit.formats, vec!["CD".to_string()]);
    assert_eq!(hit.genre.as_deref(), Some("Jazz"));
    assert_eq!(hit.release_date_text.as_deref(), Some("1994年8月4日"));
    assert!(hit.thumb_url.is_some());
    // The CDN URL's `?size=...` parameter must be stripped so we store a
    // canonical-original URL.
    assert!(
        !hit.thumb_url.as_ref().unwrap().contains("size="),
        "thumb URL should have ?size=... stripped"
    );
}

#[test]
fn parse_search_page_empty_state_returns_empty_vec() {
    // Synthesize a minimal no-results page by keeping the chrome but
    // dropping everything inside the main container.
    let empty = b"<!DOCTYPE html><html><body><div class=\"container\"><div id=\"observerTarget\"></div></div></body></html>";
    let hits = parse_search_page(empty).unwrap();
    assert!(hits.is_empty());
}

#[test]
fn parse_release_href_handles_slug_optional() {
    assert_eq!(
        parse_release_href("/release/10054881/Fourplay"),
        Some((10054881, "Fourplay".to_string()))
    );
    assert_eq!(
        parse_release_href("/release/11800001"),
        Some((11800001, "".to_string()))
    );
    assert_eq!(parse_release_href("/artist/10054881"), None);
}

#[test]
fn parse_artist_id_strips_slug() {
    assert_eq!(parse_artist_id("/artist/10017811/Fourplay"), Some(10017811));
    assert_eq!(parse_artist_id("/artist/10017811"), Some(10017811));
    assert_eq!(parse_artist_id("/release/10017811"), None);
}

#[test]
fn parse_jp_date_handles_full_partial_and_year_only() {
    assert_eq!(parse_jp_date("1994年8月4日"), Some((1994, Some(8), Some(4))));
    assert_eq!(parse_jp_date("2009年"), Some((2009, None, None)));
    assert_eq!(parse_jp_date("2014年12月"), Some((2014, Some(12), None)));
    assert_eq!(parse_jp_date("not-a-date"), None);
}

#[test]
fn strip_size_removes_query_string() {
    assert_eq!(
        strip_size("https://cdn.tower.jp/za/o/29/075992665629.jpg?size=300x300"),
        "https://cdn.tower.jp/za/o/29/075992665629.jpg"
    );
    assert_eq!(
        strip_size("https://cdn.tower.jp/za/o/29/075992665629.jpg"),
        "https://cdn.tower.jp/za/o/29/075992665629.jpg"
    );
}
