# Architecture

phono-junk identifies, catalogs, verifies, and exports audio-CD rips. It is
organized around four boundaries: shared disc I/O, analysis, durable catalog
state, and application use cases.

## Dependency direction

```text
junk-libs-core      junk-libs-disc
       \               /
        phono-junk-core
        /       |       \
      toc   accuraterip  identify <- provider crates
                              \
catalog -> db -> phono-junk-lib -> cli / gui
                 |
              extract
```

The product repositories may depend on `junk-libs`; `junk-libs` never depends
on a product. phono-junk and retro-junk never depend on each other.

## Invariants

- `junk-libs-disc` is the only implementation of CUE/CHD layout and raw PCM
  access. Product code does not reimplement container parsing.
- `phono-junk-toc` is the only implementation of TOC-derived IDs.
- `phono-junk-accuraterip` exclusively owns dBAR interpretation and audio
  verification. Identification and verification remain separate concepts.
- Provider crates own endpoint, authentication, response, and quota knowledge.
  `phono-junk-identify` owns provider-neutral observations and resolution.
- SQLite is the durable evidence and projection store. Provider evidence is
  append-only; projections can be refreshed; user overrides are authoritative.
- `phono-junk-lib::LibrarySession` is the application boundary. Presentation
  crates do not mutate the database or call providers directly.
- Network requests never run inside SQLite transactions.
- Background work is bounded, cancellable, joined, and scoped to one database
  generation. Events from an older generation are ignored.

## Crate responsibilities

- `phono-junk-core`: I/O-free TOC, ID, confidence, and error primitives.
- `phono-junk-toc`: TOC conversion and MusicBrainz/CDDB/AccurateRip IDs.
- `phono-junk-accuraterip`: checksum calculation, dBAR parsing, offset search.
- `phono-junk-identify`: provider contracts, observations, candidates, scoring.
- Provider crates: MusicBrainz/CAA, Discogs, iTunes, Tower, BarcodeLookup.
- `phono-junk-catalog`: persisted domain types, stable entity keys.
- `phono-junk-db`: schema, migrations, repositories, aggregate queries.
- `phono-junk-extract`: PCM-to-FLAC primitives and tagging.
- `phono-junk-lib`: sessions and catalog use cases shared by CLI and GUI.
- `phono-junk-cli`, `phono-junk-gui`: presentation only. GUI playback may read
  PCM through `junk-libs-disc`, but cannot mutate catalog state through it.

## Data model direction

Schema v7 is the first migration-supported baseline. It separates replaceable
catalog projections from provider observations, verification runs, and user
authority. Stable string keys—not transient SQLite row IDs—target overrides
and disagreements. See [TODO.md](TODO.md) for the active implementation queue
and [docs/knowledge](docs/knowledge) for cited external format/provider facts.
