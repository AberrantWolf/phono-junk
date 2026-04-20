---
name: phono-archive
description: Knowledge on how audio CD rips are stored, identified, and verified
---

# Phono Archive

Audio CDs are backed up from physical media into container formats shared with game preservation (CUE/BIN, CHD), but they follow an entirely different identification and verification model than ROMs.

**IMPORTANT:** When you learn something about an audio-disc format, algorithm, or data source, add it to a named file in [formats/](formats/) and link to it from any database or provider doc that uses it. Always include the upstream sources where the knowledge came from — same rule as the sibling `retro-archive` skill.

## The Identification Model

Unlike retro-game ROMs (identified by file-hash matching against DAT databases), audio CDs are identified by their **Table of Contents (TOC)** — the per-track sector offsets and lead-out position burned into every CD. The TOC is stable across rips; two bit-identical rips of the same physical disc will always produce the same DiscID, even if their BIN files differ due to drive-offset differences.

This separation is a feature, not a bug:

- **Identification** = "which disc is this?" → TOC-based DiscIDs (MusicBrainz, FreeDB/CDDB, AccurateRip id1/id2)
- **Verification** = "is this rip bit-perfect?" → audio-sample CRC (AccurateRip v1/v2, CTDB)

A single disc has multiple canonical IDs because different databases derive different values from the same TOC:

| ID Kind | Used By | Where It Lives |
|---------|---------|----------------|
| MusicBrainz DiscID | MusicBrainz | [formats/DiscID.md](formats/DiscID.md) |
| FreeDB / CDDB ID | Historical FreeDB, many legacy rippers, some Japanese DBs | [formats/DiscID.md](formats/DiscID.md) |
| AccurateRip id1, id2 | AccurateRip, CUETools DB | [formats/DiscID.md](formats/DiscID.md) |
| Barcode / Catalog Number | Discogs, Amazon, Tower Records, etc. | printed on packaging — not derived from TOC |

All TOC-derived IDs are computed from the same four inputs: first track number, last track number, lead-out sector offset, and per-track sector offsets. Compute them once in `phono-junk-toc`; every provider consumes them.

## Verification Databases

Verification databases store community-submitted per-track checksums keyed by disc IDs. A positive match from multiple independent submitters is strong evidence that your rip is bit-perfect.

| Database | Checksum Algorithm | Where It Lives |
|----------|-------------------|----------------|
| AccurateRip | v1, v2 (offset-compensated weighted sum) | [formats/AccurateRip.md](formats/AccurateRip.md) |
| CUETools Database (CTDB) | Custom CRC | (not yet documented) |

## Containers

Audio CDs use the same containers as retro disc games:

- `.cue` + `.bin` — most common. The CUE sheet carries the TOC.
- `.chd` — MAME's compressed format. The TOC is in the CHD metadata.
- Standalone `.wav` / `.flac` per track — common after conversion, but DiscID-lossy if the TOC isn't preserved.

Container parsing lives in the shared `junk-libs-disc` crate (copied from retro-junk-disc). See that crate's README and the retro-junk [formats/CUE.md](../../../../retro-junk/.claude/skills/retro-archive/formats/CUE.md) skill for details — the parser is identical. Audio CDs use `TRACK <n> AUDIO` entries rather than `MODE1/2048` or `MODE2/2352`.

## Identification Data Sources

phono-junk is designed around pluggable providers (trait-based, see `phono-junk-identify`). Day-1 identification providers are MusicBrainz and Discogs; additional sources (VGMdb, Tower Records, Gracenote, CDJapan) land as additional trait impls.

Provider-specific endpoints, auth, rate-limit policy, and scraping selectors live in the sibling [music-scraping/](../music-scraping/SKILL.md) skill.

## Sources

- [MusicBrainz Disc ID Calculation](https://musicbrainz.org/doc/Disc_ID_Calculation) — authoritative MB DiscID algorithm spec
- [libdiscid](https://github.com/metabrainz/libdiscid) — reference C implementation of MB DiscID + FreeDB/CDDB ID
- [libdiscid API reference](https://metabrainz.github.io/libdiscid/discid_8h.html) — documented public API
- [AccurateRip — Hydrogenaudio wiki](https://wiki.hydrogenaudio.org/index.php?title=AccurateRip) — user-facing overview
- [leo-bogert/accuraterip-checksum](https://github.com/leo-bogert/accuraterip-checksum) — clean C reference implementation of CRC v1 + v2
- [arcctgx/ARver](https://github.com/arcctgx/ARver) — maintained Python implementation; good for cross-checking
