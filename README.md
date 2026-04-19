# phono-junk

Audio CD rip identification, cataloging, verification, and export. Designed around identifying **uncommon and foreign-language discs** (Thai, Korean, Chinese, Japanese, etc.) where the user may not even know how to type the album title.

Sibling of [retro-junk](https://github.com/AberrantWolf/retro-junk); shares disc-parsing infrastructure via [junk-libs](https://github.com/AberrantWolf/junk-libs).

## What it does

- **Identify** CD rips (CUE/BIN, CHD) by computing canonical disc IDs from the TOC and querying multiple databases in parallel — MusicBrainz, Discogs, and additional providers as pluggable trait impls.
- **Verify** rip quality against AccurateRip (and CUETools DB, eventually). Per-track confidence scores expose whether the rip is bit-identical to other submitters'.
- **Catalog** everything in SQLite with user-editable YAML overrides. Disagreements between providers are recorded, not silently resolved.
- **Export** selected discs as per-track FLAC with embedded Vorbis tags and cover art, organized into a standard music-library tree (`<AlbumArtist>/<Album> (<Year>)/NN - Title.flac`).
- **Scrape album art** from Cover Art Archive, Discogs, iTunes Search API, and Amazon (ASIN-direct or PA-API).

CLI and GUI stay in feature sync. GUI ships with a pan-script font bundle (NotoSans + NotoSansCJK + NotoSansThai + NotoSansArabic + NotoSansDevanagari) loaded unconditionally — foreign scripts are the whole point.

## Status

**Early development.** Workspace skeleton compiles; algorithms and providers are scaffolded with traits in place and most implementations pending.

- [CLAUDE.md](CLAUDE.md) — architecture, dependency graph, conventions
- [TODO.md](TODO.md) — ordered work queue for the MVP, deferred items, and open questions
- [`.claude/skills/phono-archive/`](.claude/skills/phono-archive/) — disc-identification and verification algorithm references with upstream citations

## Build

```bash
cargo build
cargo test
cargo run -p phono-junk-cli -- --help
```

On first build, Cargo fetches [junk-libs](https://github.com/AberrantWolf/junk-libs) over git. For faster iteration when you're also developing junk-libs, clone both repos side-by-side and uncomment the `[patch]` section at the bottom of the root `Cargo.toml`.

## CLI usage

```bash
# Scan a directory tree for rips and identify each against every provider.
phono-junk scan ~/rips

# Identify a single disc from its CUE/CHD.
phono-junk identify ~/rips/pinkerton.cue

# Verify a rip against AccurateRip. Accepts --disc-id or a path.
phono-junk verify --disc-id 17

# Export disc(s) as FLAC.
phono-junk export --disc-ids 17,18 --out ~/Music/library

# Filter the catalog.
phono-junk list --artist weezer --year 1990-1999
phono-junk --format json list --country JP
```

Global flags (valid on every subcommand):

- `--db <path>` — library database path. Default: `$PHONO_JUNK_DB`, else XDG `data_dir()/phono-junk/library.db`.
- `--user-agent <string>` — HTTP User-Agent for provider calls. Default identifies phono-junk and links to the repo. MusicBrainz *requires* a descriptive UA with contact info; override it with your own contact if you plan to run scans at volume.
- `--format <human|json>` — output shape.
- `-v` / `-vv` — log verbosity (INFO / DEBUG). Default is WARN.

## Architecture

See [CLAUDE.md](CLAUDE.md) for the full workspace architecture, dependency graph, crate responsibilities, and development conventions.

Short version: 14 crates organized into analysis foundation (`core`, `toc`, `accuraterip`), pluggable providers (`identify` + `musicbrainz`, `discogs`, `itunes`, `amazon`), catalog (`catalog`, `db`), cross-cutting (`extract`, `lib`), and presentation (`cli`, `gui`).

## Sibling projects

- **[retro-junk](https://github.com/AberrantWolf/retro-junk)** — same architectural patterns, applied to retro game ROMs and disc images.
- **[junk-libs](https://github.com/AberrantWolf/junk-libs)** — shared disc I/O and generic utilities consumed by both phono-junk and retro-junk.

## License

MIT — see [LICENSE](LICENSE).
