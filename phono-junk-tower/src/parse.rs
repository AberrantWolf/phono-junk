//! HTML parsers for Tower Records MDB pages.
//!
//! Pure `&[u8] -> Result<T, ProviderError>` — no HTTP. Every entry point
//! takes raw bytes so unit tests can exercise parsers against recorded
//! fixtures.
//!
//! Selectors are pre-parsed via [`std::sync::LazyLock`] so each page
//! parse reuses the same CSS selector state; `scraper::Selector::parse`
//! is expensive enough for this to matter.
//!
//! Every selector here is documented in
//! `.claude/skills/music-scraping/tower-mdb.md`; the fixtures under
//! `tests/fixtures/` are the authoritative ground truth.

use std::sync::LazyLock;

use phono_junk_identify::ProviderError;
use scraper::{ElementRef, Html, Selector};

// ---------------------------------------------------------------------------
// Selectors (compiled once)
// ---------------------------------------------------------------------------

macro_rules! sel {
    ($expr:expr) => {
        LazyLock::new(|| Selector::parse($expr).expect(concat!("bad selector: ", $expr)))
    };
}

// Search result rows: one per listing card.
static SEARCH_RESULT_ROW: LazyLock<Selector> = sel!("div.col-12 > div.row.mb-1");
static SEARCH_CAT_LABEL: LazyLock<Selector> = sel!(".search-result-cat-label");
static SEARCH_RELEASE_LINK: LazyLock<Selector> = sel!("div.h5 a[href^='/release/']");
static SEARCH_ARTIST_LINK: LazyLock<Selector> = sel!("a[href^='/artist/']");
static SEARCH_FORMAT: LazyLock<Selector> = sel!(".format-val");
static SEARCH_GENRE_LINK: LazyLock<Selector> = sel!("a[href^='/genre/']");
static SEARCH_THUMB: LazyLock<Selector> = sel!("img.loading-img-icon");

// Release detail: the `release-info.item-*` row series.
static RELEASE_INFO_ROW: LazyLock<Selector> = sel!("div.release-info");
static RELEASE_INFO_VALUE: LazyLock<Selector> = sel!(".release-info-value");
// The artist-name row wraps the link directly in `.col-12.text-break`
// (no `.release-info-value` indirection), so a bare href selector is
// needed.
static RELEASE_INFO_ARTIST_LINK: LazyLock<Selector> = sel!("a[href^='/artist/']");
static RELEASE_INFO_GENRE_LINK: LazyLock<Selector> = sel!(".release-info-value a[href^='/genre/']");
static RELEASE_INFO_FORMAT_VAL: LazyLock<Selector> = sel!(".format-val");

// Main cover image.
static MAIN_JACKET: LazyLock<Selector> = sel!("img.main-jacket-photo");

// Tracklist: 収録内容 section. The tracklist container is the first
// .container-fluid > .container following the info block; we filter by
// the presence of the 収録内容 heading.
static INFO_LABEL: LazyLock<Selector> = sel!(".info-label");
static TRACK_ROW: LazyLock<Selector> = sel!(".row.pt-2.pb-2.border-bottom");
static TRACK_LABEL_HEADING: LazyLock<Selector> = sel!("div.col-12.text-break");

// Credits: each row has "role：name-link" in a text-break col.
static CREDIT_TEXT: LazyLock<Selector> = sel!("div.col-12.text-break");

// Tower shop link + release-ID text
static TOWER_SHOP_LINK: LazyLock<Selector> = sel!("a[href^='https://tower.jp/item/']");

// Version list rows — children of the section following "バージョンリスト".
static VERSION_TITLE_LINK: LazyLock<Selector> = sel!("a[href^='/release/']");

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// One row from a `/search/{query}/{page}` result page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchHit {
    pub release_id: u32,
    pub slug: String,
    pub title: String,
    pub artist_id: Option<u32>,
    pub artist_name: Option<String>,
    pub formats: Vec<String>,
    pub genre: Option<String>,
    pub release_date_text: Option<String>,
    pub thumb_url: Option<String>,
}

/// Parsed fields of a `/release/{id}` page.
#[derive(Debug, Clone, Default)]
pub struct ReleaseDetail {
    pub release_id: Option<u32>,
    pub title: Option<String>,
    pub artist_name: Option<String>,
    pub artist_id: Option<u32>,
    pub label: Option<String>,
    pub catalog_number: Option<String>,
    pub formats: Vec<String>,
    pub barcode: Option<String>,
    pub country: Option<String>,
    pub release_date_text: Option<String>,
    pub release_year: Option<u16>,
    pub genre: Option<String>,
    pub tower_shop_url: Option<String>,
    pub cover_url: Option<String>,
    pub info_completeness: Option<u8>,
    pub tracks: Vec<TrackEntry>,
    pub credits: Vec<CreditEntry>,
    pub description: Option<String>,
    pub version_list: Vec<VersionEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackEntry {
    pub disc: u8,
    pub position: u8,
    pub title: String,
    pub artist: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreditEntry {
    pub role_ja: String,
    pub artist_name: String,
    pub artist_id: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VersionEntry {
    pub release_id: u32,
    pub title: String,
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Parse a search-result page. Empty results → `Ok(vec![])`.
pub fn parse_search_page(bytes: &[u8]) -> Result<Vec<SearchHit>, ProviderError> {
    let html = html_from_bytes(bytes)?;
    let mut out = Vec::new();
    for row in html.select(&SEARCH_RESULT_ROW) {
        // Keep only rows explicitly labelled "Release". Artist / genre
        // cards use the same row template and would otherwise leak in.
        let cat = row
            .select(&SEARCH_CAT_LABEL)
            .next()
            .map(|e| trim_text(&e));
        if cat.as_deref() != Some("Release") {
            continue;
        }
        let Some(link) = row.select(&SEARCH_RELEASE_LINK).next() else {
            continue;
        };
        let href = link.value().attr("href").unwrap_or_default();
        let (release_id, slug) = match parse_release_href(href) {
            Some(v) => v,
            None => continue,
        };
        let title = trim_text(&link);
        let artist = row.select(&SEARCH_ARTIST_LINK).next();
        let (artist_id, artist_name) = match artist {
            Some(a) => {
                let id = a
                    .value()
                    .attr("href")
                    .and_then(parse_artist_id);
                (id, Some(trim_text(&a)))
            }
            None => (None, None),
        };
        let formats = row
            .select(&SEARCH_FORMAT)
            .map(|e| trim_text(&e))
            .filter(|s| !s.is_empty())
            .collect();
        let genre = row
            .select(&SEARCH_GENRE_LINK)
            .next()
            .map(|e| trim_text(&e));
        let release_date_text = find_date_after_label(&row);
        let thumb_url = row
            .select(&SEARCH_THUMB)
            .next()
            .and_then(|img| img.value().attr("src").map(|s| strip_size(s).to_string()));
        out.push(SearchHit {
            release_id,
            slug,
            title,
            artist_id,
            artist_name,
            formats,
            genre,
            release_date_text,
            thumb_url,
        });
    }
    Ok(out)
}

/// Parse a release detail page. Pages with no recognizable title row are
/// treated as "page exists but release not found" and return
/// `Ok(ReleaseDetail::default())`; the caller decides whether that's a
/// miss or a parser regression.
pub fn parse_release_page(bytes: &[u8]) -> Result<ReleaseDetail, ProviderError> {
    let html = html_from_bytes(bytes)?;
    let mut detail = ReleaseDetail::default();

    // --- Core `.release-info.item-*` rows ---
    for row in html.select(&RELEASE_INFO_ROW) {
        let Some(class) = row.value().attr("class") else {
            continue;
        };
        for token in class.split_ascii_whitespace() {
            match token {
                "item-title-name" => detail.title = first_text(&row),
                "item-artist-name" => {
                    let link = row.select(&RELEASE_INFO_ARTIST_LINK).next();
                    if let Some(l) = link {
                        detail.artist_name = Some(trim_text(&l));
                        detail.artist_id = l.value().attr("href").and_then(parse_artist_id);
                    } else {
                        detail.artist_name = first_text(&row);
                    }
                }
                "item-label" => detail.label = value_text(&row),
                "item-productno" => detail.catalog_number = value_text(&row),
                "item-format" => {
                    detail.formats = row
                        .select(&RELEASE_INFO_FORMAT_VAL)
                        .map(|e| trim_text(&e))
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                "item-sku" => detail.barcode = value_text(&row),
                "item-country" => detail.country = value_text(&row),
                "item-hatsubaibi" => {
                    let raw = value_text(&row);
                    if let Some(ref s) = raw
                        && let Some((y, _m, _d)) = parse_jp_date(s) {
                            detail.release_year = Some(y);
                        }
                    detail.release_date_text = raw;
                }
                "item-genre" => {
                    let link = row.select(&RELEASE_INFO_GENRE_LINK).next();
                    detail.genre = link.map(|l| trim_text(&l)).or_else(|| value_text(&row));
                }
                _ => {}
            }
        }
    }

    // --- Release ID: from Tower shop link if possible, then from the
    // "リリースID：{n}" marker in body text.
    detail.tower_shop_url = html
        .select(&TOWER_SHOP_LINK)
        .next()
        .and_then(|a| a.value().attr("href").map(|s| s.to_string()));
    detail.release_id = find_release_id_in_body(&html);

    // --- Cover image ---
    detail.cover_url = html
        .select(&MAIN_JACKET)
        .next()
        .and_then(|img| img.value().attr("src").map(|s| strip_size(s).to_string()));

    // --- Info completeness "基本情報充実度：{N}%" ---
    detail.info_completeness = find_info_completeness(&html);

    // --- Section-keyed content (tracklist, credits, description, version list) ---
    for label in html.select(&INFO_LABEL) {
        let text = trim_text(&label);
        // The section's sibling content lives in the `.container.pt-2.pb-2.mb-4`
        // that contains this `.info-label`. Walk up to that ancestor.
        let Some(container) = ancestor_with_class(&label, "container") else {
            continue;
        };
        match text.as_str() {
            "収録内容" => detail.tracks = parse_tracklist(container),
            "クレジット" => detail.credits = parse_credits(container),
            "リリース概要" => detail.description = parse_description(container),
            "バージョンリスト" => detail.version_list = parse_version_list(container, detail.release_id),
            _ => {}
        }
    }

    Ok(detail)
}

// ---------------------------------------------------------------------------
// Section parsers
// ---------------------------------------------------------------------------

fn parse_tracklist(container: ElementRef<'_>) -> Vec<TrackEntry> {
    // Track rows have the class pattern `row ... pt-2 pb-2 border-bottom`
    // with either `track-label` (disc heading) or `row-even` / `row-odd`
    // (actual tracks). We walk them in document order and keep a running
    // disc counter.
    let mut out = Vec::new();
    let mut disc: u8 = 1;
    let mut position_for_disc: u8 = 0;
    for row in container.select(&TRACK_ROW) {
        let class = row.value().attr("class").unwrap_or_default();
        if class.contains("track-label") {
            if position_for_disc > 0 {
                disc = disc.saturating_add(1);
            }
            position_for_disc = 0;
            continue;
        }
        if !(class.contains("row-even") || class.contains("row-odd")) {
            continue;
        }
        position_for_disc = position_for_disc.saturating_add(1);
        // Grab the first text-break col inside the row — that's the track
        // title. Track number is textual in .col-2 / .col-md-1 but we
        // infer it from ordering (the HTML's displayed number matches the
        // page position, and we trust the sequence).
        let mut cols = row.select(&TRACK_LABEL_HEADING);
        let title = cols.next().map(|e| trim_text(&e)).unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let artist = row
            .select(&SEARCH_ARTIST_LINK)
            .next()
            .map(|e| trim_text(&e));
        out.push(TrackEntry {
            disc,
            position: position_for_disc,
            title,
            artist,
        });
    }
    out
}

fn parse_credits(container: ElementRef<'_>) -> Vec<CreditEntry> {
    let mut out = Vec::new();
    for col in container.select(&CREDIT_TEXT) {
        let text = trim_text(&col);
        // Each credit row reads "{role}：{name}" with the name as a link.
        let Some((role, _rest)) = split_japanese_colon(&text) else {
            continue;
        };
        let Some(link) = col.select(&SEARCH_ARTIST_LINK).next() else {
            continue;
        };
        let artist_name = trim_text(&link);
        if artist_name.is_empty() {
            continue;
        }
        let artist_id = link.value().attr("href").and_then(parse_artist_id);
        out.push(CreditEntry {
            role_ja: role.to_string(),
            artist_name,
            artist_id,
        });
    }
    out
}

fn parse_description(container: ElementRef<'_>) -> Option<String> {
    // The description lives in a plain `<div>` sibling of the info-label,
    // with free-form Japanese prose. Take the largest-text child.
    let text = trim_text(&container);
    let stripped = text.strip_prefix("リリース概要").unwrap_or(&text).trim();
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

fn parse_version_list(container: ElementRef<'_>, exclude: Option<u32>) -> Vec<VersionEntry> {
    let mut out = Vec::new();
    for link in container.select(&VERSION_TITLE_LINK) {
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let Some((id, _slug)) = parse_release_href(href) else {
            continue;
        };
        if Some(id) == exclude {
            // Skip the current release — it's not a "version" of itself.
            continue;
        }
        let title = trim_text(&link);
        if title.is_empty() {
            continue;
        }
        // Dedup on release_id — the HTML puts the same href on multiple
        // wrapping elements per row.
        if out.iter().any(|v: &VersionEntry| v.release_id == id) {
            continue;
        }
        out.push(VersionEntry {
            release_id: id,
            title,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn html_from_bytes(bytes: &[u8]) -> Result<Html, ProviderError> {
    let s = std::str::from_utf8(bytes)
        .map_err(|e| ProviderError::Parse(format!("tower: invalid UTF-8: {e}")))?;
    Ok(Html::parse_document(s))
}

fn trim_text(el: &ElementRef<'_>) -> String {
    let mut buf = String::new();
    for chunk in el.text() {
        buf.push_str(chunk);
    }
    // Collapse whitespace runs.
    let mut out = String::with_capacity(buf.len());
    let mut last_ws = false;
    for c in buf.chars() {
        if c.is_whitespace() {
            if !last_ws && !out.is_empty() {
                out.push(' ');
            }
            last_ws = true;
        } else {
            out.push(c);
            last_ws = false;
        }
    }
    out.trim().to_string()
}

fn first_text(el: &ElementRef<'_>) -> Option<String> {
    let t = trim_text(el);
    if t.is_empty() { None } else { Some(t) }
}

fn value_text(row: &ElementRef<'_>) -> Option<String> {
    row.select(&RELEASE_INFO_VALUE)
        .next()
        .and_then(|v| first_text(&v))
}

/// Parse `/release/{id}/{slug}` or `/release/{id}` hrefs.
fn parse_release_href(href: &str) -> Option<(u32, String)> {
    let rest = href.strip_prefix("/release/")?;
    let mut parts = rest.splitn(2, '/');
    let id_str = parts.next()?;
    let id: u32 = id_str.parse().ok()?;
    let slug = parts.next().unwrap_or_default().to_string();
    Some((id, slug))
}

/// Parse the artist ID from `/artist/{id}` or `/artist/{id}/{slug}`.
fn parse_artist_id(href: &str) -> Option<u32> {
    let rest = href.strip_prefix("/artist/")?;
    rest.split('/').next().and_then(|s| s.parse().ok())
}

/// Strip Tower's `?size=WxH` query so we store canonical-original URLs.
fn strip_size(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

/// Parse a Japanese date of the form `1994年8月4日`, `1994年`, or
/// `1994年8月`. Year is mandatory; month and day are optional.
pub fn parse_jp_date(s: &str) -> Option<(u16, Option<u8>, Option<u8>)> {
    let (year_str, rest) = s.split_once('年')?;
    let year: u16 = year_str.trim().parse().ok()?;
    if rest.trim().is_empty() {
        return Some((year, None, None));
    }
    let (month_str, rest) = rest.split_once('月')?;
    let month: Option<u8> = month_str.trim().parse().ok();
    if rest.trim().is_empty() {
        return Some((year, month, None));
    }
    let day_str = rest.split_once('日').map(|(d, _)| d).unwrap_or(rest);
    let day: Option<u8> = day_str.trim().parse().ok();
    Some((year, month, day))
}

/// Find the first "発売日：YYYY年M月D日" value within a search row.
fn find_date_after_label(row: &ElementRef<'_>) -> Option<String> {
    let text = trim_text(row);
    let marker = "発売日：";
    let idx = text.find(marker)?;
    let tail = &text[idx + marker.len()..];
    // Cut at the first whitespace-adjacent label — the row ends with the
    // date in practice, so a trim_end is enough.
    let end = tail.find('\n').unwrap_or(tail.len());
    let value = tail[..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn find_release_id_in_body(html: &Html) -> Option<u32> {
    // The marker reads "リリースID：{n}" in the body text; scan once.
    static BODY_SEL: LazyLock<Selector> = sel!("body");
    let body = html.select(&BODY_SEL).next()?;
    let text = trim_text(&body);
    leading_digits_after(&text, "リリースID：").and_then(|s| s.parse().ok())
}

fn find_info_completeness(html: &Html) -> Option<u8> {
    static BODY_SEL: LazyLock<Selector> = sel!("body");
    let body = html.select(&BODY_SEL).next()?;
    let text = trim_text(&body);
    leading_digits_after(&text, "基本情報充実度：").and_then(|s| s.parse().ok())
}

/// Locate `marker` in `text`, skip leading whitespace after it, then
/// return the longest run of ASCII digits that follows. Tower's
/// prerendered HTML introduces a space between elements after
/// `trim_text` whitespace-collapse, so the digits are never immediately
/// flush with the marker.
fn leading_digits_after(text: &str, marker: &str) -> Option<String> {
    let idx = text.find(marker)?;
    let mut tail = text[idx + marker.len()..].chars();
    let digits: String = tail
        .by_ref()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() { None } else { Some(digits) }
}

/// Find the nearest ancestor with the given class token. Used to snap a
/// `.info-label` back up to its enclosing section container.
fn ancestor_with_class<'a>(el: &ElementRef<'a>, class_token: &str) -> Option<ElementRef<'a>> {
    let mut cur = el.parent();
    while let Some(node) = cur {
        if let Some(elem) = ElementRef::wrap(node)
            && let Some(class) = elem.value().attr("class")
                && class
                    .split_ascii_whitespace()
                    .any(|t| t == class_token)
                {
                    return Some(elem);
                }
        cur = node.parent();
    }
    None
}

/// Split on the Japanese full-width colon `：` used in credit rows.
/// Returns `(role, rest)` trimmed.
fn split_japanese_colon(s: &str) -> Option<(&str, &str)> {
    let idx = s.find('：')?;
    let role = s[..idx].trim();
    let rest = s[idx + '：'.len_utf8()..].trim();
    if role.is_empty() {
        None
    } else {
        Some((role, rest))
    }
}

#[path = "tests/parse_search_tests.rs"]
#[cfg(test)]
mod search_tests;

#[path = "tests/parse_release_tests.rs"]
#[cfg(test)]
mod release_tests;
