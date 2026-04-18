# phono-junk

Rust workspace for identifying, cataloging, verifying, and exporting audio CD rips. Identifies discs via their Table of Contents against multiple online databases, verifies rip quality against AccurateRip, and exports tagged FLAC with embedded artwork.

Sibling of `retro-junk` — shares the `junk-libs` crate family for disc I/O and generic utilities. The two products are independent but follow the same architectural patterns.

**IMPORTANT:** When learning about audio-disc formats, identification algorithms, or data sources, always document where information was learned. Cache knowledge in the skills directory and give credit to upstream sources.

- The correct location for documenting disc formats and algorithms is: `.claude/skills/phono-archive/formats/`
- The correct location for documenting identification/asset provider databases is: `.claude/skills/music-scraping/` (not yet populated — create on first research pass)

## Build & Test

```bash
cargo build                              # build all crates
cargo test                               # test all crates
cargo test -p phono-junk-toc             # test one crate
cargo install --path phono-junk-cli      # install CLI
cargo run -p phono-junk-cli -- scan /path/to/rips
cargo run -p phono-junk-cli -- identify /path/to/disc.cue
```

The sibling `junk-libs` workspace lives at `../junk-libs/` and is consumed via path dependencies. When adding workspace-external deps, update `Cargo.toml` in both workspaces if the new dep would also benefit retro-junk and the shared crates.

## Architecture

**Workspace crates:**

*Analysis foundation:*
- `phono-junk-core` — bottom-level types (`Toc`, `DiscIds`, `AlbumIdentification`, `AudioError`, `IdentificationConfidence`, `IdentificationSource`). Re-exports `junk_libs_core::ReadSeek` for convenience. No I/O.
- `phono-junk-toc` — TOC extraction from CUE/CHD and **single canonical implementation** of every disc-ID algorithm (MusicBrainz DiscID, FreeDB/CDDB ID, AccurateRip id1/id2). All providers consume its output via `DiscIds`.
- `phono-junk-accuraterip` — AccurateRip CRC v1/v2 computation and dBAR database lookup. Verification, not identification.

*Identification providers (each implements `IdentificationProvider` and/or `AssetProvider` from `phono-junk-identify`):*
- `phono-junk-identify` — `IdentificationProvider` + `AssetProvider` traits, `Aggregator` (registers providers), `Credentials`, `ProviderResult`, disagreement detection.
- `phono-junk-musicbrainz` — MusicBrainz Web Service v2 provider + Cover Art Archive asset provider. Unauthenticated. 1 req/sec.
- `phono-junk-discogs` — Discogs API provider (barcode/catalog-number keyed) + Discogs image asset provider. Requires user token.
- `phono-junk-itunes` — iTunes Search API asset-only provider (album art; URL-rewrite to hi-res). Unauthenticated.
- `phono-junk-amazon` — Amazon image asset provider. ASIN-direct fetch unauthenticated; PA-API search with affiliate credentials.

*Catalog foundation:*
- `phono-junk-catalog` — data model: `Album`, `Release`, `Disc`, `Track`, `RipFile`, `Asset`, `Disagreement`, `Override`. YAML I/O for seed data and overrides; no DB.
- `phono-junk-db` — SQLite persistence for the catalog: schema/migrations, CRUD, library cache with per-file mtime+size invalidation.

*Cross-cutting:*
- `phono-junk-extract` — BIN/CHD → per-track FLAC transcode with embedded Vorbis comments and cover art. Target layout: `<AlbumArtist>/<Album> (<Year>)/NN - Title.flac`.
- `phono-junk-lib` — glue layer: `PhonoContext` (registers all providers, exposes scan/identify/verify/export facade), `CredentialStore`, rate-limited HTTP client. Shared by CLI and GUI.

*Presentation:*
- `phono-junk-cli` — CLI frontend (clap)
- `phono-junk-gui` — desktop GUI (egui/eframe) with pan-script font bundle baked in (NotoSans + NotoSansCJK + NotoSansThai + NotoSansArabic + NotoSansDevanagari). No feature flag for fonts — foreign discs are the point.

*Sibling shared workspace:*
- `junk-libs-core` (at `../junk-libs/junk-libs-core`) — generic utilities: `AnalysisError`, `MultiHasher`, `ChecksumAlgorithm`, multi-disc filename grouping, `ReadSeek`, byte/ASCII helpers.
- `junk-libs-disc` (at `../junk-libs/junk-libs-disc`) — CUE parser (standard + CDRWin), CHD reader, ISO 9660, CD sector constants, format detection.

**Dependency graph:**
```
  junk-libs-core          junk-libs-disc
       |                        |
       +------ phono-junk-core -+
                    |
         +----------+----------+----------+
         |          |          |          |
    phono-junk-   phono-junk-  phono-junk-
       toc        accuraterip   identify
                                    |
              +------+------+-------+------+
              |      |      |       |      |
             mb   discogs  itunes  amazon  (more providers)
              |      |      |       |      |
              +------+------+-------+------+
                          |
                          |      phono-junk-catalog
                          |             |
                          |       phono-junk-db
                          |             |
                          +-- phono-junk-extract
                                        |
                                 phono-junk-lib
                                        |
                              +---------+---------+
                              |                   |
                         phono-junk-cli    phono-junk-gui
```

Notes:
- `phono-junk-toc` is the sole owner of the DiscID, FreeDB/CDDB, and AccurateRip-id algorithms. Providers never recompute IDs; they always take a populated `DiscIds` from the context.
- `phono-junk-identify` holds only trait definitions + the `Aggregator` registry. Every provider crate depends on it, not on every other provider.
- `phono-junk-lib` is the single facade shared by CLI and GUI. Neither presentation crate talks directly to provider crates.
- No crate in `phono-junk-*` depends on `retro-junk`. Shared code lives in `junk-libs`.

**Key types:**
- `Toc` and `DiscIds` (in `phono-junk-core`) — every identification hinges on these.
- `IdentificationProvider` trait (in `phono-junk-identify`) — central abstraction for new databases; parallel of retro-junk's `RomAnalyzer`. Adding a new music DB = one new crate, one trait impl, zero changes elsewhere.
- `AssetProvider` trait (in `phono-junk-identify`) — parallel trait for album art sources (Cover Art Archive, iTunes, Amazon, future Bandcamp / Last.fm / fanart.tv).
- `AlbumIdentification` — builder-style output; analog of `RomIdentification`.
- `PhonoContext` (in `phono-junk-lib`) — registry of all providers and credentials; single entry point for CLI and GUI.
- `AudioError` — error enum using `thiserror`; wraps `junk_libs_core::AnalysisError` for disc-I/O errors.
- `ReadSeek` — re-exported from `junk-libs-core`; trait alias for `Read + Seek`.
- Catalog model (in `phono-junk-catalog`): `Album` (abstract release), `Release` (pressing/region variant), `Disc` (per-CD TOC + disc-IDs), `Track` (per-track metadata), `RipFile` (local file with identification confidence/source), `Asset` (artwork with ordered groups for booklets), `Disagreement` (cross-source conflict), `Override` (user correction with `sub_path` targeting nested fields like `track[6].title`).

Provider crates own ALL database-specific knowledge. No provider-specific code exists in `phono-junk-core`, `phono-junk-identify`, `phono-junk-catalog`, or `phono-junk-lib`.

## Shared Code Principles

- **One implementation per algorithm.** DiscID algorithms (MB, FreeDB, AccurateRip) live exclusively in `phono-junk-toc`. AccurateRip CRC v1/v2 lives exclusively in `phono-junk-accuraterip`. Credential handling lives exclusively in `phono-junk-lib::credentials`. Rate-limited HTTP lives exclusively in `phono-junk-lib::http`.
- **Identification vs. verification split.** Identification answers "which disc is this?" (TOC → DiscIDs → provider lookup). Verification answers "is this rip bit-perfect?" (PCM → AccurateRip CRC → dBAR lookup). Never collapse these — retro-junk's `checksum_status` hack is a cautionary tale.
- **Unidentified is a first-class state.** When no provider returns a match, the disc is cataloged with `IdentificationConfidence::Unidentified` and its TOC is preserved for later retry or manual workflows. It is not a dead-end error.
- **Providers are pluggable.** New music database = new sibling crate implementing `IdentificationProvider` and/or `AssetProvider`, registered with `PhonoContext`. No core, catalog, CLI, GUI, or other-provider changes required. See `.claude/skills/phono-archive/SKILL.md`.
- **Catalog is the long-lived store.** The SQLite DB is the source of truth for library state; YAML overrides apply on top. Provider responses are cached into the catalog during identification and merged with disagreement detection.
- **Shared disc I/O via `junk-libs-disc`.** CUE parsing, CHD reading, and sector constants exist exactly once, shared with retro-junk. When `phono-junk` needs a new disc-layer feature (e.g. per-track raw PCM iterator for AccurateRip), add it upstream to `junk-libs-disc` rather than reimplementing locally.

**IMPORTANT**: Prioritize code change suggestions that avoid repeated code! Actively look for ways to keep the codebase "DRY". With every plan, include a section about how the plan keeps the codebase DRY, and how the plan improves the codebase.

**IMPORTANT**: Include in the plan a section about how the plan maintains and improves best practices.

**NOTE**: If DRY and best-practices improvements are out of scope for a plan, include a section to document in TODO.md the potential improvements for later updates.

## Conventions

- **Builder pattern** on `AlbumIdentification`: chain `.with_title()`, `.with_artist()`, `.with_year()`, `.with_mbid()`, `.with_confidence()`, `.with_source()`.
- **Provider-specific data** that doesn't fit the generic schema goes in a raw-response JSON field on `ProviderResult` for forensic inspection. Don't bloat the catalog with every provider's exotic fields — add a field only when two+ providers return it.
- **`&'static str`** for all provider metadata methods (`name`, `asset_types`, `supported_ids`).
- **`thiserror`** for errors. Providers return `ProviderError` from `phono-junk-identify`; it converts to `AudioError` via `From`. Library functions return `AudioError` or a crate-specific error type.
- **TOC-first identification**: always derive all disc IDs via `phono_junk_toc::compute_disc_ids` before dispatching to providers. Providers must not re-extract TOC data.
- **Disagreements over silent precedence.** When two providers disagree on a field, write a `Disagreement` row and pick one via the configured consensus policy. Never silently drop one provider's answer.
- **Overrides take precedence.** A matching `Override` row always wins, regardless of consensus. Overrides are user-authoritative.
- **Edition 2024**, workspace-level package metadata.
- **Separate tests** from code files, either via a `tests/` folder or an `X_tests.rs` included by path in the source.
- **Don't Repeat Yourself** (DRY) means that if we're rewriting basically the same thing in multiple places, that should become a shared function. Cross-workspace sharing goes through `junk-libs`.
- **Refactor** is better than rewrite.
- **Pointless tests** are the kind that are trivially provable — don't write `#[test] fn it_constructs()`. DiscID algorithm tests, on the other hand, are load-bearing: every ID kind needs fixtures with canonical values from authoritative sources (MusicBrainz spec page, `libdiscid` test vectors, real dBAR files).
- **Pan-script by default.** The GUI's font stack is NotoSans + NotoSansCJK + NotoSansThai + NotoSansArabic + NotoSansDevanagari, loaded unconditionally in `phono-junk-gui/src/fonts.rs`. Foreign discs (Thai, Korean, Chinese, Japanese, Arabic, Hindi) are a primary use case; requiring a rebuild to see them would be wrong.
- **Rate-limit respect.** MusicBrainz 1 req/sec with User-Agent; Discogs 60 req/min authenticated (25 req/min anonymous); iTunes ~20 req/min soft; Cover Art Archive piggybacks MB. Per-host limits live in `phono-junk-lib::http` so every provider inherits correct behavior.
- **Compressed containers decompress before hashing**, same as retro-junk's rule for DAT hashing. A CHD of an audio CD decompresses to raw sector data before AccurateRip CRC; the `.chd` bytes themselves are never hashed for verification.

## Relationship to retro-junk and junk-libs

`phono-junk`, `retro-junk`, and `junk-libs` are three sibling repos under `~/Programming/rust/`. Dependency direction is strictly:

```
phono-junk ──┐
             ├──► junk-libs
retro-junk ──┘
```

`junk-libs` never depends on either product. `phono-junk` never depends on `retro-junk`. Don't introduce cycles.

When making a change that might affect retro-junk (via `junk-libs`), run `cargo check` in both workspaces. When extracting new code from `phono-junk` into `junk-libs` (e.g. rate-limited HTTP, credential handling, catalog migration helpers), do it as a separate migration pass — don't mix extraction with feature work.
