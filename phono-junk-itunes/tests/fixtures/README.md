# iTunes Search API test fixtures

Fixture files here are **synthetic** — hand-written to match the documented
response shape of the iTunes Search API. They are not captures of live
responses.

Schema reference (captured 2026-04-18):

- iTunes Search API — <https://developer.apple.com/library/archive/documentation/AudioVideo/Conceptual/iTuneSearchAPI/>

## Files

- `search_exact_hit.json` — a single album hit with a 100x100bb artwork URL
  that exercises the `100 → 1000` rewrite.
- `search_no_results.json` — empty `results` array; the API's response when
  no album matches.

## Live-network smoke test

```sh
cargo test -p phono-junk-itunes -- --ignored
```

Opt-in; not run in CI.
