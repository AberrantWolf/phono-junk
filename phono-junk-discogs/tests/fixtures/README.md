# Discogs test fixtures

`search_barcode_hit.json` is a hand-authored response matching the shape
documented at <https://www.discogs.com/developers/#page:database,header:database-search>
(search → release by barcode). Field selection covers what
`parse_search_response` / `parse_search_assets` inspect:

- `results[].title` — shaped as `"Artist - Title"` per Discogs search output
- `results[].year` / `country` / `label` / `catno` / `barcode` / `cover_image`

No real token or user data is embedded. Identifiers (`999001` /
`TEST-001` / `0123456789012`) are obviously synthetic.

Captured 2026-04-20 against the public docs; refresh only if the
response envelope changes in a breaking way.
