# MusicBrainz + Cover Art Archive test fixtures

Fixture files here are **synthetic** — hand-written to match the documented
response shapes of the MusicBrainz Web Service v2 and Cover Art Archive APIs.
They are **not** captures of live responses. MBIDs (`id` fields) are
fabricated for testing and do not correspond to real releases.

Schema references used while authoring these fixtures (captured 2026-04-18):

- MusicBrainz Web Service v2 — <https://musicbrainz.org/doc/MusicBrainz_API>
- `/ws/2/discid/<id>` — <https://musicbrainz.org/doc/MusicBrainz_API#discid>
- `artist-credit` format — <https://musicbrainz.org/doc/MusicBrainz_API#Artist_Credits>
- Cover Art Archive API — <https://musicbrainz.org/doc/Cover_Art_Archive/API>
- CAA image types vocabulary — <https://musicbrainz.org/doc/Cover_Art/Types>

## Files

### MusicBrainz `/ws/2/discid/<id>?inc=artists+recordings+release-groups&fmt=json`

- `discid_single_release.json` — one match, multi-artist credit with joinphrase,
  label-info + barcode + release-group populated, three tracks with recording IDs.
- `discid_no_match.json` — empty `releases` array; MB's response when no
  release carries the given DiscID.
- `discid_multi_release.json` — two matches for the same DiscID (represents the
  common case where a DiscID is shared across regional pressings).

### Cover Art Archive `/release/<mbid>`

- `caa_front_only.json` — a single front-cover image.
- `caa_front_back_booklet.json` — front + back + two booklet pages. Used to
  verify that `types=["Booklet"]` takes precedence over the `front`/`back`
  flags and that multiple booklet pages are each classified as `Booklet`.

## Live-network smoke test

Each provider has a `#[ignore]`-gated test that hits the real endpoint. Invoke
manually — these are not run in CI:

```sh
cargo test -p phono-junk-musicbrainz -- --ignored
```

The live tests read the MB DiscID to look up from `PHONO_MB_LIVE_DISCID`
(defaults to a DiscID that has been stable at MB for years; if MB removes the
release, point the env var at a known-good DiscID yourself).
