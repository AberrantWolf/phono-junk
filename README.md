# phono-junk

Audio CD rip identification, cataloging, verification, and export. Designed around identifying **uncommon and foreign-language discs** (Thai, Korean, Chinese, Japanese, etc.) where the user may not even know how to type the album title.

Sibling of [retro-junk](https://github.com/AberrantWolf/retro-junk); shares disc-parsing infrastructure via [junk-libs](https://github.com/AberrantWolf/junk-libs).

## What it does

- **Identify** CD rips (CUE/BIN, CHD) by computing canonical disc IDs from the TOC and querying pluggable metadata providers.
- **Verify** rip quality against AccurateRip (and CUETools DB, eventually). Per-track confidence scores expose whether the rip is bit-identical to other submitters'.
- **Catalog** everything in SQLite with user-editable YAML overrides. Disagreements between providers are recorded, not silently resolved.
- **Export** selected discs as per-track FLAC with embedded Vorbis tags and cover art, organized into a standard music-library tree (`<AlbumArtist>/<Album> (<Year>)/NN - Title.flac`).
- **Find album art** through Cover Art Archive, Discogs, iTunes Search, Tower Records MDB, and BarcodeLookup where configured.

CLI and GUI stay in feature sync. GUI ships with a pan-script font bundle (NotoSans + NotoSansCJK + NotoSansThai + NotoSansArabic + NotoSansDevanagari) loaded unconditionally — foreign scripts are the whole point.

## Status

**Alpha.** The core workflows are implemented and tested, but the catalog,
background-job, verification, and provider-resolution foundations are being
hardened before more product features land. Schema v7 intentionally rebuilds
older alpha catalogs.

- [ARCHITECTURE.md](ARCHITECTURE.md) — boundaries and invariants
- [TODO.md](TODO.md) — current Now/Next/Later queue
- [`docs/knowledge/`](docs/knowledge/) — source-cited format and provider notes
- [`docs/archive/development-history.md`](docs/archive/development-history.md) — historical sprint diary

## Build

```bash
cargo build
cargo test
cargo run -p phono-junk-cli -- --help
```

On first build, Cargo fetches the exact pinned [junk-libs](https://github.com/AberrantWolf/junk-libs) revision over git. A clean checkout does not require a sibling repository.

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

See [ARCHITECTURE.md](ARCHITECTURE.md) for the current dependency graph,
crate responsibilities, and invariants. `CLAUDE.md` and `AGENTS.md` contain
tooling instructions rather than project status.

Short version: the workspace separates analysis (`core`, `toc`,
`accuraterip`), provider-neutral identification plus provider crates, durable
catalog state (`catalog`, `db`), application use cases (`lib`, `extract`), and
presentation (`cli`, `gui`).

## Sibling projects

- **[retro-junk](https://github.com/AberrantWolf/retro-junk)** — same architectural patterns, applied to retro game ROMs and disc images.
- **[junk-libs](https://github.com/AberrantWolf/junk-libs)** — shared disc I/O and generic utilities consumed by both phono-junk and retro-junk.

## License

MIT — see [LICENSE](LICENSE).
