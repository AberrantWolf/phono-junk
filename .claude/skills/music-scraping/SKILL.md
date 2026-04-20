---
name: music-scraping
description: Knowledge about third-party music databases and asset providers used for album identification and cover art — per-provider endpoints, auth, rate limits, response shapes, and scraping recipes
---

# Music Scraping

Per-provider knowledge base for the identification and asset providers registered with `PhonoContext` in `phono-junk-lib`. Each provider documented here corresponds to a `phono-junk-*` crate implementing `IdentificationProvider` and/or `AssetProvider` from `phono-junk-identify`.

**IMPORTANT:** When adding or extending a provider, document every upstream endpoint, auth requirement, rate-limit policy, and response format here. Cite official API docs; save HTML fixtures under `fixtures/` for scraper-backed providers. Same rule as the sibling `phono-archive` skill — see [../phono-archive/SKILL.md](../phono-archive/SKILL.md).

## Provider Classification

| Provider | Crate | Identify | Assets | Auth | Shape |
|----------|-------|----------|--------|------|-------|
| MusicBrainz | `phono-junk-musicbrainz` | yes (DiscID, barcode) | via Cover Art Archive | none (User-Agent required) | JSON/XML API |
| Discogs | `phono-junk-discogs` | yes (barcode, catno) | yes | user token | JSON API |
| iTunes Search | `phono-junk-itunes` | — | yes | none | JSON API |
| Amazon | `phono-junk-amazon` | — | yes | optional PA-API | ASIN + PA-API |
| Tower Records MDB | (planned) `phono-junk-tower` | yes (barcode, catno) | cover art | none | HTML scrape — see [tower-mdb.md](tower-mdb.md) |

Official JSON-API providers each get a short notes file only where their behavior diverges from the public docs (rate-limit surprises, undocumented fields, auth edge cases). Scraper-backed providers always get a full doc: URL patterns, selectors, caveats, and at least one fixture under `fixtures/`.

## Scraping Etiquette (applies to all scraper-backed providers)

1. **One request per configured interval per host.** Default 1 request per 2 seconds. Rate-limiter lives in `phono-junk-lib::http`, shared with rate-limited JSON providers; per-host policy is configured at provider registration.
2. **Identifying User-Agent.** Include product name + contact URL, e.g. `phono-junk/0.x (+https://github.com/.../phono-junk)`. Never impersonate a browser to bypass throttling.
3. **Cache aggressively.** Release/detail pages change rarely; cache for 30 days. Search pages change more often but are cheap to re-fetch; 7 days is fine. Honor a negative cache (empty-result TTL) to avoid hammering on misses.
4. **Respect robots.txt when present.** If the host adds one later, the shared HTTP client should check it.
5. **Fall back, don't hammer.** Scraped providers are last-resort fallbacks after authoritative API providers have been queried and returned nothing.

## Fixtures

`fixtures/` contains verbatim HTML responses used when documenting scraper selectors and as regression fodder for parser tests. Filename convention: `{provider}-{page-kind}-{key}.html`. Always record the source URL and fetch date in the accompanying provider doc so fixtures can be refreshed.

## Sources

- MusicBrainz Web Service v2: <https://musicbrainz.org/doc/MusicBrainz_API>
- Discogs API v2: <https://www.discogs.com/developers>
- iTunes Search API: <https://performance-partners.apple.com/search-api>
- Cover Art Archive: <https://musicbrainz.org/doc/Cover_Art_Archive/API>
