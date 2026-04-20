//! Parser tests for `/release/{id}` detail pages.

use super::*;

const FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/release-10054881.html");

#[test]
fn parse_release_page_extracts_core_fields() {
    let d = parse_release_page(FIXTURE).unwrap();
    assert_eq!(d.release_id, Some(10054881));
    assert_eq!(d.title.as_deref(), Some("Fourplay"));
    assert_eq!(d.artist_name.as_deref(), Some("Fourplay"));
    assert_eq!(d.artist_id, Some(10017811));
    assert_eq!(d.label.as_deref(), Some("Wea"));
    assert_eq!(d.catalog_number.as_deref(), Some("26656"));
    assert_eq!(d.barcode.as_deref(), Some("075992665629"));
    assert_eq!(
        d.country.as_deref(),
        Some("インターナショナル - International")
    );
    assert_eq!(d.release_date_text.as_deref(), Some("1994年8月4日"));
    assert_eq!(d.release_year, Some(1994));
    assert_eq!(d.genre.as_deref(), Some("Jazz"));
    assert_eq!(d.formats, vec!["CD".to_string()]);
    assert_eq!(d.info_completeness, Some(100));
}

#[test]
fn parse_release_page_extracts_cover_url_without_size_param() {
    let d = parse_release_page(FIXTURE).unwrap();
    let url = d.cover_url.expect("main-jacket-photo src should parse");
    assert!(url.starts_with("https://cdn.tower.jp/"));
    assert!(!url.contains("?size="), "cover URL should have ?size=... stripped");
}

#[test]
fn parse_release_page_extracts_tower_shop_link() {
    let d = parse_release_page(FIXTURE).unwrap();
    let url = d.tower_shop_url.expect("tower.jp/item/... link should be present");
    assert!(url.starts_with("https://tower.jp/item/"));
}

#[test]
fn parse_release_page_extracts_full_tracklist() {
    let d = parse_release_page(FIXTURE).unwrap();
    assert_eq!(d.tracks.len(), 11);
    let first = &d.tracks[0];
    assert_eq!(first.disc, 1);
    assert_eq!(first.position, 1);
    assert_eq!(first.title, "Bali Run");
    let last = d.tracks.last().unwrap();
    assert_eq!(last.position, 11);
    assert_eq!(last.title, "Rain Forest");
    // Every track on this disc is credited to Fourplay.
    for t in &d.tracks {
        assert_eq!(t.artist.as_deref(), Some("Fourplay"));
    }
}

#[test]
fn parse_release_page_extracts_credits() {
    let d = parse_release_page(FIXTURE).unwrap();
    // Fourplay fixture has 5 credit rows: 3 artists, 1 producer, 1 artist
    // (the ordering on the live page is non-grouped).
    assert_eq!(d.credits.len(), 5);
    let roles: Vec<&str> = d.credits.iter().map(|c| c.role_ja.as_str()).collect();
    assert!(roles.contains(&"アーティスト"));
    assert!(roles.contains(&"プロデューサー"));
    // Patti LaBelle appears among the credited artists.
    assert!(
        d.credits.iter().any(|c| c.artist_name == "Patti LaBelle"),
        "credits should include Patti LaBelle"
    );
}

#[test]
fn parse_release_page_extracts_version_list_excluding_self() {
    let d = parse_release_page(FIXTURE).unwrap();
    // Fixture shows 9 sibling pressings (JP WPCP-4463, SHM-CD WPCR-13459,
    // completion edition WPCR-28041, Music On Vinyl, Evolution,
    // Anniversary editions). The current release (10054881) must not
    // appear in its own version list.
    assert!(!d.version_list.is_empty(), "version list should have entries");
    assert!(
        !d.version_list.iter().any(|v| v.release_id == 10054881),
        "current release must be excluded from its own version list"
    );
}

#[test]
fn parse_release_page_extracts_description() {
    let d = parse_release_page(FIXTURE).unwrap();
    let desc = d.description.expect("fixture contains a リリース概要 block");
    assert!(desc.contains("Fourplay"));
}

#[test]
fn parse_release_page_handles_empty_bytes_gracefully() {
    let d = parse_release_page(b"<html></html>").unwrap();
    assert!(d.title.is_none());
    assert!(d.barcode.is_none());
    assert!(d.tracks.is_empty());
    assert!(d.version_list.is_empty());
}
