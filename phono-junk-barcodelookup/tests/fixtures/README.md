# Barcode Lookup test fixtures

`search_barcode_hit.json` is a hand-authored response modelled on the
envelope documented at <https://www.barcodelookup.com/api>. Field
selection covers what `parse_search_response` / `parse_search_assets`
inspect:

- `products[].barcode_number` / `title` / `manufacturer` / `brand`
- `products[].release_date` (full `YYYY-MM-DD` form; the year-only
  fallback is exercised inline in unit tests)
- `products[].images[]`

No real API key or user data is embedded. Identifiers (`0123456789012`,
`TEST-001`) are obviously synthetic and the image URLs resolve to
non-existent paths.

Captured 2026-04-20 against the public docs; refresh only if the
response envelope changes in a breaking way.
