# Disc Identification Algorithms

Every audio CD's Table of Contents (TOC) yields multiple canonical identifiers, each used by a different database. All of them are computed from the same four inputs:

- **First track number** — conventionally `1`, but can be higher for multi-disc releases with hidden intro tracks.
- **Last track number** — the last *audio* track. A trailing data session (CD-Extra, mixed-mode) is excluded; see the multi-session caveat under each algorithm.
- **Lead-out sector offset** — the absolute sector position where the disc's audio data ends (start of the lead-out gap).
- **Per-track sector offsets** — absolute sector position where each track begins. Track 1 typically starts at sector 150 (the 2-second lead-in).

All sector offsets are in CD frames (= sectors). 75 frames per second; 588 stereo samples per frame at 44.1 kHz.

**Sources for every algorithm on this page are listed at the bottom.** Compute all IDs once in `phono-junk-toc` and pass them through `DiscIds` to every provider — never recompute inside a provider.

## MusicBrainz DiscID

The primary key for MusicBrainz lookups. A 28-character URL-safe Base64 string. Computed purely from the TOC; no network access needed.

### Algorithm

Concatenate the following as **uppercase hexadecimal ASCII**:

1. First track number — `%02X` (2 chars)
2. Last track number — `%02X` (2 chars)
3. Lead-out offset — `%08X` (8 chars)
4. Track offsets for tracks 1 through 99, padded with `00000000` for tracks that don't exist — `%08X` each (8 chars × 99 = 792 chars)

Total input string length: `2 + 2 + 8 + (8 × 99) = 804` ASCII characters.

Hash the string with **SHA-1** (20 bytes of output), then **Base64-encode** the digest using a URL-safe variant: replace `+` with `.`, `/` with `_`, and `=` with `-`. The result is exactly 28 characters.

### Example

A 6-track CD with track offsets `[150, 15363, 32314, 46592, 63414, 80489]` and lead-out at `95462`:

- Input string (concatenated): `01` + `06` + `000174E6` + `00000096` + `00003C03` + `00007E3A` + `0000B600` + `0000F7B6` + `000013A29` ... (last 93 offsets are `00000000` each)
- SHA-1 → 20 bytes → URL-safe base64: **`49HHV7Eb8UKF3aQiNmu1GR8vKTY-`**

### Multi-session discs (CD-Extra / Enhanced CD)

When a data session follows the audio session, the last entry reported in the TOC is the data track, not the true audio lead-out. Exclude the data track from the audio-track count, then **subtract 11,400 frames** from the data track's offset to recover the audio lead-out position used in the DiscID calculation.

### Reference implementation

`libdiscid` is the authoritative C implementation: `discid_get_id()` returns the 28-char string from a populated `DiscId` handle. Our Rust implementation in `phono-junk-toc` must produce byte-identical output for every test fixture.

Test fixtures should be extracted from `libdiscid`'s own test suite and from real discs where you can independently verify via the MusicBrainz website's "Lookup by Disc ID" page.

## FreeDB / CDDB ID

An 8-hex-digit legacy ID used by the original CDDB/FreeDB service, and still used today by AccurateRip (as part of the dBAR URL), Discogs (as a secondary lookup key), and most Japanese-focused databases that predate MusicBrainz.

### Algorithm

```
N = 0
for each track t = first_track .. last_track:
    seconds = track_offset[t] / 75           # absolute MSF seconds, lead-in included
    N += digit_sum(seconds)                  # single-pass digit sum

T = (leadout_offset - track_offset[first_track]) / 75

cddb_id = ((N mod 0xFF) << 24) | (T << 8) | num_tracks
```

Where:
- `digit_sum(n)` is a **single-pass** sum of the decimal digits of `n`, not iterative-to-single-digit. `digit_sum(1734) = 1 + 7 + 3 + 4 = 15` (we stop there — we do **not** continue with `1 + 5 = 6`). This matches the reference implementation in libdiscid (`src/base64.c` / `src/toc.c`) and cd-discid.
- `num_tracks = last_track - first_track + 1`
- `seconds` is derived from the **raw absolute sector offset** divided by 75 — the 2-second lead-in is included. Equivalently: `seconds = minutes * 60 + seconds` where `(minutes, seconds, frames)` is the MSF address of the track's start. **Do not subtract 150 before dividing** — despite the AccurateRip algorithms below using LSN, the FreeDB/CDDB algorithm predates the LSN convention and uses raw MSF seconds.
- `T` uses the first-track's raw offset too (same rationale as `seconds`).

Output is formatted as 8 lowercase hex digits (some sources uppercase — phono-junk uses lowercase to match `libdiscid::discid_get_freedb_id()`).

### Reference implementation

`libdiscid::discid_get_freedb_id()`. Cross-check against `cd-discid(1)` (Unix utility) on any CD.

## AccurateRip Disc IDs (id1, id2)

Two 32-bit IDs derived from track offsets. Combined with the FreeDB/CDDB ID, they form the key into the AccurateRip dBAR database.

### Algorithm

Convert every track offset and the lead-out to **LSN**: `lsn = offset - LEAD_IN_FRAMES` where `LEAD_IN_FRAMES = 150`.

```
id1 = (sum(lsn_track_offsets) + lsn_leadout) & 0xFFFFFFFF

id2 = sum((offset_or_1) * track_number
          for track_number, offset in enumerate(lsn_track_offsets, start=1))
      + lsn_leadout * (num_tracks + 1)
id2 = id2 & 0xFFFFFFFF
```

Where `offset_or_1` is the LSN offset, or `1` if that offset is `0`. (Track-1 LSN is usually `0` since track 1 starts at absolute sector 150 = LSN 0, so track 1's contribution to `id2` is always `1 × 1 = 1`.)

Both IDs are 32-bit unsigned integers. When embedded in the dBAR URL they are formatted as 8 lowercase hex digits.

### dBAR URL format

```
http://www.accuraterip.com/accuraterip/<id1_last_char>/<id1_2nd_last>/<id1_3rd_last>/dBAR-<ntracks_padded>-<id1_hex>-<id2_hex>-<cddb_id_hex>.bin
```

- `<id1_last_char>`, `<id1_2nd_last>`, `<id1_3rd_last>` are the last three characters of the 8-char hex string of `id1`, used in reverse order as the directory path.
- `<ntracks_padded>` is the track count formatted as `0NN` (always 3 digits with a leading `0`) — e.g. `012` for 12 tracks.
- All IDs are lowercase 8-hex-digit strings.

Example for a 12-track disc with `id1 = 0x001b0c2a`, `id2 = 0x02e8a1b7`, `cddb_id = 0xa40c4b0c`:
```
http://www.accuraterip.com/accuraterip/a/2/c/dBAR-012-001b0c2a-02e8a1b7-a40c4b0c.bin
```

## Implementation notes (for `phono-junk-toc`)

- Store the three identification IDs in `phono_junk_core::DiscIds` as optional strings. A `None` means that particular ID couldn't be computed (e.g., TOC was incomplete).
- Format MB DiscID as its natural URL-safe base64 output (mixed case, punctuation). FreeDB/CDDB ID and AccurateRip IDs should be stored lowercase hex (`format!("{:08x}", value)`) so URL construction and string comparison are trivial.
- A single `compute_disc_ids(&Toc) -> DiscIds` function is the canonical entry point. No provider should reach into the TOC to compute IDs itself — that's how divergent implementations happen.
- Test fixtures: stash known TOCs with their verified IDs in `phono-junk-toc/tests/fixtures/`. The MusicBrainz spec page gives one canonical example; `libdiscid`'s source tree has more. For AccurateRip, any dBAR URL scraped from a real lookup gives a verified id1/id2/cddb triple.

## Sources

- [MusicBrainz Disc ID Calculation](https://musicbrainz.org/doc/Disc_ID_Calculation) — authoritative specification of the MB DiscID algorithm, including the canonical 6-track example.
- [libdiscid on GitHub](https://github.com/metabrainz/libdiscid) — reference C implementation. Compute all three IDs (`discid_get_id`, `discid_get_freedb_id`) and compare byte-for-byte.
- [libdiscid API reference](https://metabrainz.github.io/libdiscid/discid_8h.html) — complete documented public API for the reference implementation.
- [MusicBrainz Web Service v2 — /ws/2/discid](https://musicbrainz.org/doc/MusicBrainz_API#discid) — how to look up a DiscID once computed.
- [cd-discid on Debian Sources](https://sources.debian.org/src/cd-discid/) — a minimal standalone C implementation of the FreeDB/CDDB ID algorithm. Useful cross-check.
- [arcctgx/ARver — arver/disc/fingerprint.py](https://github.com/arcctgx/ARver/blob/master/arver/disc/fingerprint.py) — concise Python implementation of AccurateRip id1 and id2. Well-commented, good for reading.
- [arcctgx/ARver — arver/disc/database.py](https://github.com/arcctgx/ARver/blob/master/arver/disc/database.py) — dBAR URL construction and binary response parsing.
- [AccurateRip — Hydrogenaudio Knowledgebase](https://wiki.hydrogenaudio.org/index.php?title=AccurateRip) — user-facing overview of the verification ecosystem; links to deeper technical posts.
