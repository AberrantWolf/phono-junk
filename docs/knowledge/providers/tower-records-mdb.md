---
name: tower-mdb
description: Tower Records Japan music information database — scraping barcode/catalog-number search and release pages to cover the domestic-Japan gap left by Discogs/MusicBrainz
---

# Tower Records Japan MDB (mdb.tower.jp)

Crowd-edited music information database run by Tower Records Japan. Over 4.4 million releases, 5.1 million artists. Strongest coverage for domestic-Japan pressings (JP-only singles, city-pop reissues, anime/idol discographies) — exactly the gap where Discogs/MusicBrainz queries tend to return empty.

**Site:** <https://mdb.tower.jp/>
**Role in phono-junk:** `IdentificationProvider` fallback, queried **after** Discogs returns no barcode/catalog-number match. Also exposes `AssetProvider` for cover art.
**Data completeness:** variable per release — every record has a 基本情報充実度 (info-completeness percentage) in the page body. Entry-level records have title/artist/label/catalog#/barcode only; well-filled records (like the Fourplay fixture) add tracklist, credits, and release description.

## Terms & Robots

- `robots.txt` returns 404 (no explicit rules).
- Site terms: <https://tower.jp/information/kiyaku> (Japanese).
- Treat data as user-contributed reference material; don't republish pages verbatim. Storing extracted fields in our local catalog is fine; always retain the source release URL for attribution.
- Copyright: © Tower Records Japan Inc.

## Rate & Caching Policy

- **1 request / 2 seconds per host**, configured via `phono-junk-lib::http` like other providers.
- **User-Agent:** `phono-junk/<version> (+<project-url>)`.
- **Cache TTL:** 30 days for release/search pages, 7 days for barcode search (misses should negative-cache for 7 days too — the site takes community edits frequently enough that a barcode that misses today may hit next week).
- Avoid concurrent requests. The app is Blazor Server; repeated rapid hits hit a live connection pool on their side.

## Technology Note

mdb.tower.jp is a **Blazor Server** app with server-side prerendering. The initial HTTP response contains all page content in HTML; the Blazor JS bundle only adds interactivity (collection/wishlist buttons, image gallery). Scrape from the prerendered HTML — **do not** execute JS.

Every page includes a reconnect modal and an error banner in the HTML that read as if the server has disconnected:

```html
<h5>サーバとの接続が切断されました。ページをリロードしてください。</h5>
...
<div id="blazor-error-ui">アプリケーションでエラーが発生しました。...</div>
```

These are **always present** in the prerendered HTML regardless of state — they only become visible to a real browser when the Blazor WebSocket drops. A scraper must ignore them; do not use their presence as a signal that a request failed.

## URL Patterns

| Purpose | URL | Notes |
|---------|-----|-------|
| Release detail | `/release/{id}` or `/release/{id}/{slug}` | Slug is cosmetic; ID alone works. |
| Artist detail | `/artist/{id}` or `/artist/{id}/{slug}` | |
| Genre | `/genre/{id}` | |
| Search (all fields) | `/search/{query}/{page}` | `page` is 0-indexed. Query is URL-encoded. Barcode and catalog # both work. |
| Cover art CDN | `https://cdn.tower.jp/...` | Path appears to be derived from barcode or a catalog-internal key; extract from `<img>` src. `?size=WxH` parameter resizes. |

The search form has a type filter (すべて / アーティスト / タイトル / バーコード / 規格品番) but the options live on a client-side Blazor dropdown with no URL counterpart. A bare `/search/{query}/0` fuzzy-matches across all fields; that's what we use. Barcodes and catalog numbers are specific enough that the all-fields search is effectively a targeted lookup.

## Search Results (`/search/{query}/{page}`)

Each result row is a `<div class="col-12">` block inside the main `.container`. Filter to **releases** only by selecting blocks whose category label reads `Release`:

```html
<div class="col-9 col-md-11">
  <div class="... search-result-cat-label ...">Release</div>
  <div class="h5 f18 mb-1">
    <a href="/release/10054881/Fourplay" title="Fourplay">Fourplay</a>
  </div>
  <div style="color:darkgrey;">
    <a href="/artist/10017811/Fourplay">Fourplay</a>
  </div>
  <div class="disp-fl mb-1"><div class="format-val mr-1">CD</div></div>
  <div><a href="/genre/1301">Jazz</a></div>
  <div class="disp-fl mb-1">
    <div>発売日：</div>
    <div>1994年8月4日</div>
  </div>
</div>
```

### Selectors

| Field | Selector (CSS, scoped per result row) |
|-------|---------------------------------------|
| Category (filter) | `.search-result-cat-label` — keep `Release`, skip `Artist` |
| Release ID + slug | `a[href^="/release/"]` — parse `/release/{id}/{slug}` |
| Release title | same `<a>` text content |
| Artist ID + name | `a[href^="/artist/"]` — parse `/artist/{id}/{slug}` |
| Format(s) | `.format-val` — multiple elements for multi-format releases |
| Genre | `a[href^="/genre/"]` |
| Release date | text following the `発売日：` label div |
| Cover thumb | `img.loading-img-icon` src |

### Empty State

When nothing matches:

```html
<!-- Results container is empty; no .col-12 rows -->
<div id="observerTarget"></div>
```

The string 表示するデータが見つかりませんでした ("no data to display") also appears elsewhere on empty-state pages. Simplest reliable check: no `a[href^="/release/"]` descendants inside the main `.container`.

## Release Detail (`/release/{id}/{slug}`)

All core metadata lives in `.release-info.item-*` rows within `.container-fluid`. Each row has the form:

```html
<div class="row release-info item-{FIELD}">
  <div class="release-item-lable release-info-label">{Japanese label}</div>
  <div>：</div>
  <div class="release-item-info release-info-value text-break">{value}</div>
</div>
```

### Core Field Selectors

| Field | Row class | Value selector | Notes |
|-------|-----------|----------------|-------|
| Title | `.item-title-name` | `.col-12.text-break` | Plain text |
| Artist | `.item-artist-name` | `.col-12 a` | Parse `/artist/{id}/{slug}` from href; text is display name |
| Label | `.item-label` | `.release-info-value` | Plain text |
| Catalog number (規格品番) | `.item-productno` | `.release-info-value` | e.g. `WPCR-13459`, `26656` |
| Format | `.item-format` | `.format-val` | May repeat (`SHM-CD、CD`); split on `、` |
| Barcode / JAN (バーコード) | `.item-sku` | `.release-info-value` | 12–13 digit EAN/UPC |
| Country | `.item-country` | `.release-info-value` | e.g. `日本`, `インターナショナル - International` |
| Release date (発売日) | `.item-hatsubaibi` | `.release-info-value` | Japanese format `1994年8月4日` — parse with regex `(\d+)年(\d+)月(\d+)日` |
| Genre | `.item-genre` | `.release-info-value a` | Parse `/genre/{id}` from href |
| Release ID | — | Plain text `リリースID：{id}` in `.right-add-btn-area` or `.center-add-btn-area` | Or parse from URL path |
| Tower shop link | — | `a[href^="https://tower.jp/item/"]` | Useful as an external ID |
| Info completeness % | — | `.h5.font-weight-bold` following `基本情報充実度：` | Use as a confidence hint |
| Main cover image | — | `img.main-jacket-photo` `src` | Strip `?size=...` to get original; append `?size=500x500` etc. for resizing |

### Tracklist (optional — present when populated)

Located after the `<div class="info-label ...">収録内容</div>` heading. One row per disc label (`track-label` class) followed by per-track rows:

```html
<div class="row pt-2 pb-2 border-bottom track-label">
  <div class="col"><div class="col-12 text-break">CD</div></div>  <!-- disc heading -->
</div>
<div class="row ... row-even d-flex">
  <div class="col-2 pl-2 col-md-1">1</div>                       <!-- track number -->
  <div class="col-10 col-md-11 pl-2 pr-2">
    <div class="row mb-2"><div class="col-12 pl-0 col-md-10 text-break">Bali Run</div></div>  <!-- title -->
    <div class="row track-info pr-2">
      <div class="col-6 col-sm-6 col-md-4 col-lg-3">
        <div class="row pl-3 text-break">
          <a href="/artist/10017811/Fourplay">Fourplay</a>       <!-- per-track artist, optional -->
        </div>
      </div>
    </div>
  </div>
</div>
```

Selectors (scoped to the 収録内容 section):

| Field | Selector |
|-------|----------|
| Disc heading (multi-disc sets) | `.track-label .col-12.text-break` |
| Track number | `.row.row-even .col-2, .row.row-odd .col-2` (first col of each non-label row) |
| Track title | first `.col-12.text-break` inside the row's right column |
| Per-track artist | `a[href^="/artist/"]` inside `.track-info` |

Multi-disc sets produce multiple `track-label` dividers; reset per-disc numbering on each.

### Credits (optional)

After `<div class="info-label ...">クレジット</div>`:

```html
<div class="row mb-2">
  <div class="col-12 text-break">
    アーティスト：<a href="/artist/.../Patti-LaBelle">Patti LaBelle</a>
  </div>
</div>
<div class="row mb-2">
  <div class="col-12 text-break">
    プロデューサー：<a href="/artist/.../Fourplay">Fourplay</a>
  </div>
</div>
```

Each row is `role：name`. Common roles: アーティスト (artist), プロデューサー (producer), エンジニア (engineer), 作詞 (lyricist), 作曲 (composer), 編曲 (arranger). Store as `(role_ja, artist_name, artist_id)` tuples; let the catalog layer normalize roles if needed.

### Release Description (optional)

After `<div class="info-label ...">リリース概要</div>` — free-form Japanese prose, usually from the label's marketing copy. Store verbatim in the catalog's raw-response JSON field; don't try to structure it.

### Version List (other pressings of the same album)

After `<div class="info-label ...">バージョンリスト</div>` — rows of title / format / label / catalog # / country / year, each linking to another `/release/{id}`. This is the **killer feature** for our fallback use case: a hit on a JP-only pressing by catalog number exposes the sibling international pressing with the barcode that Discogs/MB will recognize. Worth storing the version-list release IDs on the catalog entry for later enrichment.

## Mapping to `AlbumIdentification`

| Tower MDB field | `AlbumIdentification` builder |
|-----------------|-------------------------------|
| Title | `.with_title()` |
| Artist (main) | `.with_artist()` |
| Release date year | `.with_year()` (parse from `1994年...`) |
| Barcode | `.with_barcode()` (add if not already on builder) |
| Catalog # | `.with_catalog_number()` (add if not already on builder) |
| Release ID (Tower-specific) | raw-response JSON on `ProviderResult` |
| Cover image URLs | `AssetProvider::fetch_assets` — one `Asset` per image |
| Credits | raw-response JSON; only promote to first-class catalog fields once 2+ providers agree |
| Version list release IDs | raw-response JSON (used to cross-reference other DBs) |

Confidence policy: default to `IdentificationConfidence::Medium` on a barcode hit (user-edited data, occasional errors). Promote to `High` only when the Tower record's 基本情報充実度 is ≥90% and another provider corroborates.

## Empty vs. Missing

- **Search empty state:** HTTP 200, no `a[href^="/release/"]` in main container. Negative-cache 7 days.
- **Missing release ID:** HTTP 200, but the core field rows (`.item-title-name`, `.item-sku` etc.) are absent. Treat as not-found. Do not retry.
- **HTTP 404:** may occur for legitimately bad URLs (typo'd slug, for instance). Retry with the ID-only form `/release/{id}` before giving up.

## Fixtures

- [fixtures/tower-mdb-release-10054881.html](fixtures/tower-mdb-release-10054881.html) — Fourplay / Fourplay (Wea, CD, 1994). Source: <https://mdb.tower.jp/release/10054881/Fourplay>. Fetched 2026-04-20. 100% info-completeness, 11-track tracklist, 5-row credits block, 10-entry version list.
- [fixtures/tower-mdb-search-barcode-075992665629.html](fixtures/tower-mdb-search-barcode-075992665629.html) — barcode search returning one result (the Fourplay release above). Source: <https://mdb.tower.jp/search/075992665629/0>. Fetched 2026-04-20.

Refresh fixtures (and update the fetch date above) whenever the parser is updated against a new site revision.

## Sources

- Tower Records Music Information Database: <https://mdb.tower.jp/>
- Site terms (Japanese): <https://tower.jp/information/kiyaku>
- Fourplay (Fourplay) reference release: <https://mdb.tower.jp/release/10054881/Fourplay>
