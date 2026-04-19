# TODO

Work queue for phono-junk, organized as an ordered sprint list. Each sprint is sized to fit comfortably in a single focused conversation; dependencies on prior sprints are called out explicitly. Add new items freely — this is a living list, not a spec.

## Completed sprints

- [x] **Sprint 1 — DiscID algorithms** (`phono-junk-toc/src/discid.rs`, `phono-junk-toc/src/lib.rs`)
  MusicBrainz DiscID, FreeDB/CDDB ID, AccurateRip id1/id2 + `compute_disc_ids` wrapper. Fixtures: MB spec 6-track, libdiscid `test_put.c` 22-track, four ARver test fixtures. Corrected skill-doc error (CDDB seconds formula uses raw offset/75 with single-pass digit-sum, not LSN-based with iterative sum).

- [x] **Sprint 2 — TOC extraction from CUE + CHD**
  Generic absolute-sector layout (`TrackLayout`, `TrackKind`, `LEAD_IN_FRAMES`, `compute_cue_layout`, `read_cue_layout`, `compute_chd_layout`, `read_chd_layout`) landed upstream in `junk-libs-disc` so retro-junk can reuse it. Audio-CD-specific CD-Extra -11,400 correction lives in `phono-junk-toc::toc_from_layout::layout_to_toc`. Integration test reproduces Sprint 1's ARver 3-track DiscIDs end-to-end from a real `.cue` + sparse `.bin`, and verifies CD-Extra handling against a synthetic mixed-session fixture. Fixture BINs are sparse (`File::set_len`) — 0 bytes on disk, ~750 MB logical.

- [x] **Sprint 3 — Rate-limited HTTP + User-Agent foundation** (`phono-junk-identify/src/http.rs`)
  Rate-limited `HttpClient` with per-host `governor` token buckets (MB/CAA 1/s, iTunes 20/min; Discogs quota parked until the provider un-defers). Mandatory User-Agent injection (MB requires it; builder returns `MissingUserAgent` if unset). Tests cover UA injection, UA-required, per-host rate limiting with `FakeRelativeClock`, independent per-host buckets, and HTTP 429 mapping. Credential persistence and Discogs deferred post-MVP.

- [x] **Sprint 4 + 6 (bundled) — MusicBrainz + CAA + iTunes providers** (`phono-junk-musicbrainz/`, `phono-junk-itunes/`)
  `MusicBrainzProvider::lookup` implements `/ws/2/discid/<id>?inc=artists+recordings+release-groups&fmt=json`; response → `ProviderResult` with album/release/track metadata and a shared `artist_credit::format` helper. `CoverArtArchiveProvider::lookup_art` implements `/release/<mbid>` JSON listing with six-way `AssetType` classification (booklet/tray/medium/obi beat back/front when tagged). `ITunesProvider::lookup_art` implements Search API with `100x100bb.jpg` → `1000x1000bb.jpg` rewrite. Trait change: `AssetProvider::lookup_art(&AssetLookupCtx)` replaces the three-parameter signature so iTunes can see album title + artist. `PhonoContext::with_default_providers(user_agent)` threads UA into every provider. Hand-rolled JSON fixtures under each crate's `tests/fixtures/` with source-citing README; `#[ignore]`-gated live-network smoke tests per crate. Amazon half split out — see deferred section.

- [x] **Sprint 7 — AccurateRip CRC v1/v2 + PCM iterator** (`junk-libs-disc/src/pcm.rs`, `phono-junk-accuraterip/src/crc.rs`)
  PCM iterator landed upstream in `junk-libs-disc`: `TrackPcmReader` with `from_bin` / `from_chd` constructors, yielding `Result<[u32; 588], AnalysisError>` per CDDA frame via a shared `sector_to_samples` helper. New `read_chd_raw_sector` primitive extracted so BIN and CHD audio reads share one hunk-decode path. CRC v1 and v2 computed in a single pass via `track_crc_streaming(samples, total_samples, TrackPosition)` with `TrackPosition::{First, Middle, Last, Only}` driving skip bounds. Cross-verified against ARver's `tests/checksums_test.py` fixture CRCs (sample.wav at four positions + silence.wav). Corrected an off-by-one in `.claude/skills/phono-archive/formats/AccurateRip.md` — the reference C implementations include position 2940 (using `multiplier >= skip_frames`), not 2941.

- [x] **Sprint 8 — AccurateRip dBAR fetch + parse** (`phono-junk-accuraterip/{url.rs,dbar.rs,verify.rs,client.rs,error.rs}`)
  `dbar_url` builds ARver-format URLs from `DiscIds`; `DbarFile::parse` decodes stacked 13-byte headers + 9-byte track entries; `verify_track`/`verify_disc` compare computed `TrackCrc` against a dBAR, guarding against the legacy `v2 == 0` sentinel to avoid spurious zero matches. `AccurateRipClient` wraps `phono-junk-identify`'s shared rate-limited `HttpClient` (1 req/sec on `www.accuraterip.com`) — removes the direct `reqwest` dep and the stub `lookup()`. `fetch_dbar` maps 200→`Some(DbarFile)`, 404→`None`, other→`AccurateRipError::Parse`; `fetch_at_url` exposes the response-dispatch path so `httpmock`-backed tests cover every branch offline. `TrackVerification::status_string` produces the stable short form (`"v2 confidence 8"`, `"v1 confidence 3 (v2 no match)"`, `"no match"`) that will persist to `RipFile.accuraterip_status` once Sprint 10 wires it. Ignored live test fetches a real dBAR end-to-end.

## Sprint queue (implement in order)

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
- [ ] **Multi-disc MB release handling (Sprint 4+6)** — `parse_discid_response` picks `media[0]` and logs when `>1`. Full medium-selection (match TOC track count against each medium, handle the multi-disc catalog model) deferred until a real multi-disc fixture surfaces.
- [ ] **MB multi-release disambiguation UI (Sprint 4+6)** — when `releases.len() > 1` for a DiscID (legitimate case: region variants and re-issues), MVP picks the first and warns. User-facing picker is a manual-search-UI scope item (listed in the post-MVP deferred block).
- [ ] **CAA multi-size thumbnail picker (Sprint 4+6)** — `parse_caa_response` returns the full-size `image` URL only; `thumbnails.small` / `thumbnails.large` / `thumbnails.1200` are ignored. Revisit alongside the GUI asset picker (Sprint 14+).
- [ ] **CAA approved-only filtering (Sprint 4+6)** — the `approved` flag is dropped during parsing. MVP trusts all images since most CAA submissions are approved; add a filter/penalty once a disagreement surfaces.
- [ ] **`AssetLookupCtx.album` → non-optional (Sprint 4+6)** — currently `Option<&AlbumMeta>` because Sprint 11 wiring isn't in place yet. Tighten to `&AlbumMeta` once the aggregator guarantees an album is resolved before art lookup fires.
- [ ] **iTunes fuzzy-match scoring (Sprint 4+6)** — current impl labels every Search API hit `AssetConfidence::Fuzzy`. Post-MVP, score each hit against `album.title + album.artist_credit` (Levenshtein / token-sort ratio) and either drop mismatches or rank by score.
- [ ] **iTunes URL rewrite via regex (Sprint 4+6)** — `replace("100x100bb.jpg", ...)` covers the canonical path; extend to `NxNbb\.(jpg|png)` regex once a non-100 source surfaces.
- [ ] **MB per-track artist credit (Sprint 4+6)** — track-level artist credits (for compilations, split releases) live at `media[].tracks[].recording.artist-credit` in MB. `TrackMeta.artist_credit` stays `None` in MVP; wire up when a compilation fixture surfaces.
- [ ] **Sample-offset compensation for AR lookup (Sprint 7)** — `track_crc_streaming` computes CRCs at drive offset 0. Real-world verification often requires trying ±offsets (typically −30 to +30 samples) to find a match. Defer to a mini-sprint between Sprint 8 (dBAR parse) and Sprint 11 — offset scan only makes sense once there's a target CRC to compare against. The `SA`/`SB` compensated-sum technique documented in `AccurateRip.md` "Implementation notes" is the path.
- [ ] **Multi-FILE CUE PCM iteration (Sprint 7)** — `TrackPcmReader::from_bin` assumes the CUE sheet is backed by a single whole-disc BIN. CDRWin-style CUEs with one BIN per track need a `from_cue(cue_path, &TrackLayout)` constructor that consults FILE directives. Defer until a real multi-FILE CUE fixture surfaces.
- [ ] **Real-CHD PCM integration test (Sprint 2/7)** — Sprint 2's deferred note promised a real CHD PCM round-trip alongside Sprint 7. The PCM iterator's CHD path has unit coverage but no real-CHD integration test; add one when a usable fixture is available.
- [ ] **CHD hunk caching for PCM streaming (Sprint 7)** — `TrackPcmReader::from_chd` currently re-opens the CHD file and re-decompresses the target hunk per sector (inherited from existing `read_chd_sector` behaviour). For a 70-minute CD's ~316,000 audio sectors this is a major waste. Fix by caching the current hunk's bytes on the reader, or by holding a long-lived `chd::Chd` handle inside `PcmSource::Chd`. Not blocking MVP correctness; high-value speedup for Sprint 12's batch extract.
- [ ] **Sample-offset scan for verification (Sprint 8)** — same theme as the Sprint 7 deferred "sample-offset compensation": `verify_track` today checks only the drive-offset-0 CRC. Once the aggregator is live, wrap `verify_disc` with a compensated-sum scan (`SA`/`SB` technique from `AccurateRip.md`) so ±30-sample drive-offset mismatches become verifiable. Belongs between Sprint 8 and Sprint 11.
- [ ] **Persist dBAR raw bytes on `Disc` (Sprint 8 → Sprint 9)** — Sprint 9's schema should add a `dbar_raw BLOB` column (or a sibling table keyed by discid triple) so the catalog can re-run verification without re-fetching. Parse-from-cache is cheap; re-fetching burns the accuraterip.com quota.
- [ ] **Register `AccurateRipClient` on `PhonoContext` (Sprint 8 → Sprint 13)** — currently constructed ad-hoc by callers. Add `pub accuraterip: AccurateRipClient` to `PhonoContext` and initialise inside `with_default_providers` once the CLI `verify` subcommand needs it. Trivial; left opportunistic because `PhonoContext`'s public shape is still settling around Sprint 11.
- [ ] **`VerificationProvider` trait (post-MVP)** — if/when CUETools CTDB joins AccurateRip as a second verification source, extract a `VerificationProvider` trait from `AccurateRipClient` in `phono-junk-identify` (sibling to `IdentificationProvider` / `AssetProvider`). Premature today — one implementation is not a trait.

## Deferred (post-MVP)

Captured from the original bootstrap plan; not blocking v1 but known wants.

- [ ] **CredentialStore TOML persistence + obfuscation** — deferred until the first token-requiring provider (Discogs or Amazon PA-API) moves back into scope. Previously Sprint 3 scope. Intended on-disk format: plain TOML for keys, base64-wrapped XOR for values, key embedded in the binary. Not crypto — matches retro-junk-scraper's "prevent casual leaks" goal, but applied to user tokens at rest (distinct from retro-junk-scraper's compile-time embedded dev credentials, which is a different use case and should not be conflated).
- [ ] **Discogs provider + image asset** (was Sprint 5) — requires a user token, so moves here alongside credential persistence. Location: `phono-junk-discogs/`. Work: `/database/search?type=release&barcode=...` + `catno=...` lookup with user token; JSON → `ProviderResult` populating `barcode` / `catalog_number` in `DiscIds` enrichment; Discogs image URL asset fetch; tests against recorded JSON fixtures.
- [ ] **Amazon asset provider** (was second half of Sprint 6) — split out of Sprint 4+6 and moved here. ASIN-direct fetch from `m.media-amazon.com/images/I/<asin>.jpg` can't fire without an ASIN, and the only ASIN sources in the pipeline are Discogs responses and user entry (both deferred). Reactivate alongside Discogs: add an `asin` slot to `DiscIds`, populate it from Discogs, then wire `AmazonProvider::lookup_art` to emit a candidate when `DiscIds.asin.is_some()`. PA-API search mode stays deferred indefinitely (needs affiliate credentials). Crate stub stays a workspace member; `PhonoContext::with_default_providers` re-registers it when the ASIN path exists.
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
- [ ] **Extract DB version-guard scaffold into `junk-libs-db`** (new sibling crate or module in `junk-libs-core`) — `schema_version` table, `open_database`/`open_memory`, `CURRENT_VERSION` const idiom, `SchemaError::VersionMismatch`. Shared between `retro-junk-db/src/schema.rs` and `phono-junk-db/src/schema.rs` as of Sprint 9; two live consumers = extraction trigger. Pre-release, neither product writes a stepwise `migrate(from)` — if version mismatches, the caller deletes the DB and re-scans. Extract the real migration runner (when one or both products start shipping migrations post-release) as a separate later pass.
- [ ] **Extract catalog schema helpers** from phono-junk-catalog/db into junk-libs once both products have settled schemas (note: different from the scaffold entry above — this is for the DDL authoring helpers if any emerge, not the versioning plumbing)
- [ ] **Extract JSON-column helpers** (`json_column::read<T>` / `json_column::write<T>`) to `junk-libs-db` if Sprint 10's CRUD layer repeatedly wraps `serde_json::to_string` / `from_str` around `Vec<PathBuf>`, `Toc`, `secondary_types_json`, etc. Evaluate during Sprint 10.
- [ ] **Extract `(mtime, size)` library-cache primitive** to `junk-libs-db::cache` if Sprint 10's per-file invalidation table in phono-junk-db is clearly generic enough to replace retro-junk-db's `library_entries` cache behaviour. Evaluate during Sprint 10.
- [ ] **Extract rate-limited HTTP client** from `phono-junk-identify::http` into junk-libs once retro-junk-scraper is ready to consume it (currently re-exported via `phono-junk-lib::http` for CLI/GUI convenience)
- [ ] **Extract credential store** (TOML + XOR obfuscation idiom) from `phono-junk-lib::credentials` into junk-libs (blocked on the credential persistence work itself, which is deferred above)
- [ ] **`parse_chd_tracks_from_path(&Path)` in `junk-libs-disc`** — small convenience so CHD consumers don't each need a direct `chd` crate dep. `read_chd_layout` already does this internally in Sprint 2; generalise if a second consumer appears.

## Open questions

- Which Japanese-focused disc database is best as the first JP-region provider? Candidates: VGMdb, Tower Records Japan, CDJapan, HMV Japan. Needs research into which has a usable API.
- How to model HDCD / SACD / mixed-mode carriers — extend `Disc` with a carrier-type enum, or add to `extra` map?
- User-facing vs. archival override semantics — should overrides fully replace scraped data, or sit alongside it as "preferred display" while keeping the provider's value for audit?
