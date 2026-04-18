# TODO

Work queue for phono-junk, organized as an ordered sprint list. Each sprint is sized to fit comfortably in a single focused conversation; dependencies on prior sprints are called out explicitly. Add new items freely — this is a living list, not a spec.

## Completed sprints

- [x] **Sprint 1 — DiscID algorithms** (`phono-junk-toc/src/discid.rs`, `phono-junk-toc/src/lib.rs`)
  MusicBrainz DiscID, FreeDB/CDDB ID, AccurateRip id1/id2 + `compute_disc_ids` wrapper. Fixtures: MB spec 6-track, libdiscid `test_put.c` 22-track, four ARver test fixtures. Corrected skill-doc error (CDDB seconds formula uses raw offset/75 with single-pass digit-sum, not LSN-based with iterative sum).

- [x] **Sprint 2 — TOC extraction from CUE + CHD**
  Generic absolute-sector layout (`TrackLayout`, `TrackKind`, `LEAD_IN_FRAMES`, `compute_cue_layout`, `read_cue_layout`, `compute_chd_layout`, `read_chd_layout`) landed upstream in `junk-libs-disc` so retro-junk can reuse it. Audio-CD-specific CD-Extra -11,400 correction lives in `phono-junk-toc::toc_from_layout::layout_to_toc`. Integration test reproduces Sprint 1's ARver 3-track DiscIDs end-to-end from a real `.cue` + sparse `.bin`, and verifies CD-Extra handling against a synthetic mixed-session fixture. Fixture BINs are sparse (`File::set_len`) — 0 bytes on disk, ~750 MB logical.

## Sprint queue (implement in order)

### Sprint 3 — Rate-limited HTTP + User-Agent foundation
Location: `phono-junk-identify/src/http.rs` (canonical), `phono-junk-lib/src/http.rs` (re-export)
Depends on: nothing (shared by every network-hitting sprint after this)
- [x] Rate-limited `HttpClient` with per-host `governor` token buckets (MB/CAA 1/s, iTunes 20/min; Discogs 60/min quota known but unused until the Discogs provider un-defers)
- [x] Mandatory User-Agent injection (MB requires it; builder returns `MissingUserAgent` if unset)
- [x] Tests: UA injection, UA required, per-host rate limiting with `FakeRelativeClock`, independent per-host buckets, HTTP 429 mapping

Credential persistence and Discogs are **deferred post-MVP** (see deferred list below).

### Sprint 4 — MusicBrainz provider + Cover Art Archive
Location: `phono-junk-musicbrainz/`
Depends on: Sprint 1, Sprint 3
- [ ] `/ws/2/discid/<id>?inc=recordings+artists+release-groups&fmt=json` lookup
- [ ] JSON response → `ProviderResult` / `AlbumIdentification`
- [ ] Cover Art Archive `/release/<mbid>/front` asset fetch (`AssetProvider` impl)
- [ ] Tests against recorded JSON fixtures (no live network in CI)

### Sprint 6 — iTunes + Amazon asset providers
Location: `phono-junk-itunes/`, `phono-junk-amazon/`
Depends on: Sprint 3
- [ ] iTunes Search API lookup + `100x100bb.jpg` → `1000x1000bb.jpg` URL rewrite
- [ ] Amazon ASIN-direct image URL construction + fetch (mode 1 only; PA-API deferred)
- [ ] Tests for URL rewrite logic + fixture-based search parsing

### Sprint 7 — AccurateRip CRC v1/v2 + PCM iterator
Location: `phono-junk-accuraterip/`, possibly upstream to `junk-libs-disc`
Depends on: Sprint 2
- [ ] Per-track raw PCM sample iterator (yields stereo u32 samples; 588/sector). If it fits in `junk-libs-disc`, add it there; otherwise keep local with a TODO to upstream.
- [ ] CRC v1: sample-position-weighted sum, first-track 2940-sample skip, last-track intact
- [ ] CRC v2: same weighting but 64-bit intermediate product folded back, last-track 2940-sample skip
- [ ] Tests against known-good rip (raw PCM fixture → published AR CRC; ARver's test WAVs are a good source)

### Sprint 8 — AccurateRip dBAR fetch + parse
Location: `phono-junk-accuraterip/`
Depends on: Sprint 1 (id1/id2/cddb), Sprint 3 (HTTP)
- [ ] dBAR URL construction (`accuraterip.com/accuraterip/<c3>/<c2>/<c1>/dBAR-NNN-<id1>-<id2>-<cddb>.bin`)
- [ ] Binary parse: 13-byte header + 9-byte per-track entries, little-endian
- [ ] Tests against stored dBAR bytes

### Sprint 9 — SQLite schema + migrations
Location: `phono-junk-db/src/schema.rs`, `phono-junk-db/src/migrate.rs`
Depends on: `phono-junk-catalog` types
- [ ] DDL for Album / Release / Disc / Track / RipFile / Asset / Disagreement / Override
- [ ] Migration-version table + idempotent `migrate()`
- [ ] Tests: create fresh DB, migrate twice (idempotency), schema round-trip

### Sprint 10 — Catalog CRUD + library cache
Location: `phono-junk-db/src/crud.rs`, `phono-junk-db/src/cache.rs`
Depends on: Sprint 9
- [ ] Insert / read / update for every catalog entity
- [ ] Override `sub_path` application (supports `track[6].title`, etc.)
- [ ] Library cache: per-file `(mtime, size)` invalidation — not per-folder fingerprint
- [ ] Tests

### Sprint 11 — Aggregator + consensus + disagreement + override
Location: `phono-junk-identify/src/aggregator.rs` (if stub), `phono-junk-lib/src/identify.rs`
Depends on: Sprints 4, 6, 10
- [ ] Parallel provider fan-out with per-host rate limiting honored
- [ ] Consensus policy selects one value per field; losers become `Disagreement` rows
- [ ] Override application: YAML `Override` rows win over consensus
- [ ] Tests with mock providers forcing each code path (consensus, conflict, override)

### Sprint 12 — Extract pipeline (PCM → FLAC, tags, layout)
Location: `phono-junk-extract/`
Depends on: Sprint 2 + Sprint 7 (PCM iterator), Sprint 11 (populated catalog)
- [ ] FLAC encoding via `flac-bound` or `claxon + flac`
- [ ] Vorbis comment writer (full 12-tag spec from CLAUDE.md)
- [ ] `METADATA_BLOCK_PICTURE` front-cover embed
- [ ] Output tree `<library>/<AlbumArtist>/<Album> (<Year>)/NN - Title.flac` + `cover.jpg`
- [ ] Tests on a short synthetic PCM fixture

### Sprint 13 — CLI wiring
Location: `phono-junk-cli/src/main.rs`
Depends on: Sprints 2, 11, 12
- [ ] Wire `scan`, `identify`, `verify`, `export`, `list` to `PhonoContext`
- [ ] `--filter` syntax for `list` (artist / year / genre / language)
- [ ] Smoke tests via `assert_cmd`

### Sprint 14 — GUI fonts + album list
Location: `phono-junk-gui/src/fonts.rs`, `phono-junk-gui/src/views/album_list.rs`
Depends on: Sprint 10
- [ ] Pan-script Noto bundle — bake `.ttf`/`.otf` via `include_bytes!` and install in egui font system (Noto Sans + CJK + Thai + Arabic + Devanagari, per CLAUDE.md "Pan-script by default")
- [ ] Album list view with structured filter bar
- [ ] Manual smoke test notes (GUI can't be auto-tested cleanly)

### Sprint 15 — GUI bulk ops + activity bar
Location: `phono-junk-gui/src/views/`
Depends on: Sprint 14
- [ ] Multi-select for bulk operations (export, re-verify, re-identify)
- [ ] Activity bar showing background operations with progress + cancel
- [ ] Scan / identify / verify / export dispatch via `spawn_background_op`

## Known deferred from completed sprints

- [ ] **Shared HttpClient across providers (Sprint 3)** — Sprint 3 has each provider construct its own `HttpClient`, so MB and CAA hold independent musicbrainz.org buckets. Harmless while provider calls are sequential; revisit when Sprint 11 introduces parallel fan-out. Options: a domain-pool primitive in `phono-junk-identify` that coordinates across client instances, or move ownership to `PhonoContext` and thread it into providers at call time.
- [ ] **Real redumper-output CD-Extra fixture** — Sprint 2 verifies the -11,400 correction against a synthetic CUE; a genuine redumper rip from a 90s enhanced CD (e.g. an album with a CD-ROM bonus track) would be a stronger acceptance test. Requires sourcing a specific physical disc or a user-contributed rip.
- [ ] **Real CHD integration test** — Sprint 2 covers `compute_chd_layout` with hand-built `ChdTrackInfo` slices and `read_chd_layout` inherits `junk-libs-disc`'s existing CHD I/O tests. A real-CHD fixture belongs alongside Sprint 7's PCM iterator, which will stress the same I/O paths. A `#[test] #[ignore]`'d stub can be added in `phono-junk-toc/tests/` when Sprint 7 begins.
- [ ] **CDRWin-format fuzz corpus for `read_cue_layout`** — happy-path CDRWin auto-conversion is already tested upstream in `junk-libs-disc`; Sprint 2 exercises the combined pipeline on a standard CUE. A broader CDRWin fuzz corpus belongs with `junk-libs-disc`'s own test suite as a follow-up.
- [ ] **Full multi-session DiscID handling** — Sprint 2 handles audio-then-data (CD-Extra) cleanly and errors on leading-data mixed-mode. Discs with multiple data sessions or audio after a data session are exotic; pick up when a real user fixture surfaces.
- [ ] **redumper `.toc` / MDS / MDF layout adapters** — add `junk_libs_disc::toc::read_toc_layout(path) -> Vec<TrackLayout>` (and similar for MDS/MDF) so alternative TOC sources plug into the same `TrackLayout` funnel. Zero phono-junk work when added; each is a separate sprint.
- [ ] **ISRC extraction from CUE sheets** — `parse_cue` drops ISRC lines today; extend during Sprint 4 (MusicBrainz) when ISRC fallback matching becomes relevant. Stash in a per-track sidecar struct, not `Toc`.
- [ ] **`libdiscid` FFI byte-for-byte cross-check harness** — CI-style belt-and-braces check that links `libdiscid` via FFI and asserts byte-equal output on every fixture. Sprint 1's string equality against libdiscid/ARver test values is already gold-standard; this is future hygiene.

## Deferred (post-MVP)

Captured from the original bootstrap plan; not blocking v1 but known wants.

- [ ] **CredentialStore TOML persistence + obfuscation** — deferred until the first token-requiring provider (Discogs or Amazon PA-API) moves back into scope. Previously Sprint 3 scope. Intended on-disk format: plain TOML for keys, base64-wrapped XOR for values, key embedded in the binary. Not crypto — matches retro-junk-scraper's "prevent casual leaks" goal, but applied to user tokens at rest (distinct from retro-junk-scraper's compile-time embedded dev credentials, which is a different use case and should not be conflated).
- [ ] **Discogs provider + image asset** (was Sprint 5) — requires a user token, so moves here alongside credential persistence. Location: `phono-junk-discogs/`. Work: `/database/search?type=release&barcode=...` + `catno=...` lookup with user token; JSON → `ProviderResult` populating `barcode` / `catalog_number` in `DiscIds` enrichment; Discogs image URL asset fetch; tests against recorded JSON fixtures.
- [ ] **Manual search UI** for unidentified discs across all providers — visual cover-picker, romanization search, submit-as-new-release flow for MusicBrainz
- [ ] **Barcode extraction** from redumper log and CD-Text to enable automatic Discogs (and other barcode-keyed) lookup without user intervention
- [ ] **Additional identification providers** as the user researches endpoints: VGMdb (vgmdb.net, strong on anime/game OSTs), Tower Records Japan (if a usable API exists), Gracenote/CDDB, CDJapan
- [ ] **Additional asset providers**: Bandcamp, Last.fm, fanart.tv
- [ ] **Consensus-policy UI** — let the user choose per-field priority per language/region (e.g. "prefer Japanese provider for Japan-region releases")
- [ ] **Multi-format export** — MP3, OGG Vorbis, Opus alongside FLAC
- [ ] **Multi-page booklet assets** UI — the Asset model's `group_id` + `sequence` fields already support this; UX needed
- [ ] **Per-track overrides UI** — Override model's `sub_path` field already supports targeting `track[N].field`; UX needed
- [ ] **ISRC-based MB matching** as a fallback when DiscID misses
- [ ] **CD-Text and redumper log parsing** as a metadata fallback
- [ ] **Thai / Arabic IME testing** — rendering works via Noto bundle; input testing is separate

## Cross-repo infrastructure

Items that affect both phono-junk and retro-junk via `junk-libs`. Out of scope for MVP; schedule when both products are stable enough to refactor.

- [ ] **Migrate retro-junk to consume `junk-libs`** — retro-junk-disc → junk-libs-disc, retro-junk-core shared bits → junk-libs-core. Separate plan.
- [ ] **Extract catalog schema + DB migration helpers** from phono-junk-catalog/db into junk-libs once both products have settled schemas
- [ ] **Extract rate-limited HTTP client** from `phono-junk-identify::http` into junk-libs once retro-junk-scraper is ready to consume it (currently re-exported via `phono-junk-lib::http` for CLI/GUI convenience)
- [ ] **Extract credential store** (TOML + XOR obfuscation idiom) from `phono-junk-lib::credentials` into junk-libs (blocked on the credential persistence work itself, which is deferred above)
- [ ] **`parse_chd_tracks_from_path(&Path)` in `junk-libs-disc`** — small convenience so CHD consumers don't each need a direct `chd` crate dep. `read_chd_layout` already does this internally in Sprint 2; generalise if a second consumer appears.

## Open questions

- Which Japanese-focused disc database is best as the first JP-region provider? Candidates: VGMdb, Tower Records Japan, CDJapan, HMV Japan. Needs research into which has a usable API.
- How to model HDCD / SACD / mixed-mode carriers — extend `Disc` with a carrier-type enum, or add to `extra` map?
- User-facing vs. archival override semantics — should overrides fully replace scraped data, or sit alongside it as "preferred display" while keeping the provider's value for audit?
