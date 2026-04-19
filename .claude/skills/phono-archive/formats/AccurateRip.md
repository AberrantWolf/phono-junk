# AccurateRip CRC Verification

AccurateRip is a community-maintained database of per-track CD audio checksums. Its purpose is **verification** — confirming that a local rip is bit-identical to the same track ripped by other people with correctly-offset drives. It does not identify discs (that's what [DiscID](DiscID.md) is for); given a disc ID, AccurateRip returns N expected checksums per track and a "confidence" count of how many submitters produced that checksum.

A track with confidence `≥ 2` from independent submitters is strong evidence that your rip is bit-perfect.

## Two versions: CRC v1 and v2

AccurateRip originally used a single checksum (v1). The v1 algorithm has a known flaw — about 3% of the right-channel data is effectively ignored due to a 32-bit multiplication overflow truncation. v2 fixes this by accumulating both halves of the 64-bit product.

The database stores both. Modern rippers compute and submit both. Verification checks both independently; a match in either is acceptable, a match in v2 is preferred.

## Sample layout

The algorithm operates on stereo 16-bit PCM, packed as one **u32 per stereo sample**: left channel in the low 16 bits, right in the high 16 bits (little-endian). Every CDDA frame is 588 stereo samples = 588 u32s = 2352 bytes.

```
┌─ u32 ─┐  ┌─ u32 ─┐  ┌─ u32 ─┐   ...
LL LL RR RR LL LL RR RR LL LL RR RR
└ sample 1┘ └ sample 2┘ └ sample 3┘
```

When reading from a CUE/BIN or CHD, extract the raw 2352-byte audio-track sectors and reinterpret them as `u32` little-endian values.

## CRC v1

```text
AR_CRC = 0
position = 1          # 1-indexed, runs across the whole track
for each u32 sample in the track:
    if check_start <= position <= check_end:
        AR_CRC = (AR_CRC + (position * sample)) & 0xFFFFFFFF
    position += 1
```

The multiplication is a 32-bit truncated product (v1's known flaw). `AR_CRC` is an accumulating 32-bit unsigned integer with wrap-around.

## CRC v2

Identical iteration, but preserves the full 64-bit product:

```text
AR_CRC_v2 = 0
position = 1
for each u32 sample in the track:
    if check_start <= position <= check_end:
        product = (position as u64) * (sample as u64)
        hi = (product >> 32) as u32
        lo = (product & 0xFFFFFFFF) as u32
        AR_CRC_v2 = (AR_CRC_v2 + hi + lo) & 0xFFFFFFFF
    position += 1
```

The accumulator stays 32-bit with wrap-around; what changes is that the high half of each multiplication is folded in instead of being discarded.

## First- and last-track frame skipping

CD audio has slightly different alignment near disc boundaries. Both CRC versions apply the same skip logic:

- **First track of the disc:** skip the first **5 CDDA frames** (= `5 × 588 = 2940` stereo samples). Positions `1..=2939` do not contribute; position `2940` is the first included position (the reference C implementations use `multiplier >= skip_frames`, so `multiplier == 2940` passes the gate).
- **Last track of the disc:** skip the last **5 CDDA frames** (= 2940 samples). The last included position is `(track_sample_count - 2940)`.
- **All other tracks:** no skip. Every position `1..=track_sample_count` contributes.

Expressed as `check_start` / `check_end`:

| Track position    | check_start | check_end                           |
|-------------------|-------------|-------------------------------------|
| First track       | `2940`      | `track_sample_count`                |
| Middle tracks     | `1`         | `track_sample_count`                |
| Last track        | `1`         | `track_sample_count - 2940`         |
| Single-track disc | `2940`      | `track_sample_count - 2940`         |

These bounds match both reference implementations (leo-bogert's
[`accuraterip-checksum.c`](https://github.com/leo-bogert/accuraterip-checksum/blob/master/accuraterip-checksum.c)
and ARver's [`_audio.c`](https://github.com/arcctgx/ARver/blob/master/arver/audio/_audio.c))
and are cross-verified via ARver's `tests/checksums_test.py` fixture CRCs.

**Why the skip:** historical robustness — CD drives vary in how they handle the very first and very last samples of the disc's audio region, so AccurateRip ignores those zones to reduce false mismatches.

## dBAR file: the database response

Given the three [disc IDs](DiscID.md), fetch:

```
http://www.accuraterip.com/accuraterip/<id1_last>/<id1_2nd_last>/<id1_3rd_last>/dBAR-<NNN>-<id1>-<id2>-<cddbid>.bin
```

The response is a binary `.bin` file containing one or more **Responses** concatenated. Each Response represents one "pressing" (one submitter's rip of a disc that claimed this same triple of IDs):

```
Response {
    Header (13 bytes, little-endian):
        u8   track_count                # should equal num_tracks on the queried disc
        u32  ar_id1
        u32  ar_id2
        u32  cddb_id
    TrackEntry[track_count]:
        u8   confidence                 # how many submitters agreed with this checksum
        u32  v1_checksum
        u32  v2_checksum                # (may be 0 for older submissions)
        # total 9 bytes per track
}
```

A single disc typically has 2–10 Responses stacked in one `.bin` — different pressings, different submitters, or different drive offsets. Match your computed `v1` or `v2` checksum against any TrackEntry across all Responses; a hit at any confidence level is a positive match.

### Interpreting "confidence"

The confidence byte is the number of submitters whose rips produced this exact checksum. Common rubric:

- `1–2`: weak — could be coincidence or correlated errors
- `3–9`: good match
- `10+`: very high confidence, effectively canonical
- `200+`: saturated at max value (popular CDs)

Reporting this raw number to the user is more useful than thresholding it; they can decide for themselves.

## Implementation notes (for `phono-junk-accuraterip`)

- **Streaming is fine**: compute both v1 and v2 in a single pass over the PCM. No need to buffer a whole track.
- **Handle the triple-skip case**: single-track discs apply both the start-skip (2940) and end-skip (2940) simultaneously. Most rips won't hit this, but test for it.
- **Offset compensation** (advanced, v2+): the multiplication-by-position structure means if you compute `SA = Σ(sample × i)` and `SB = Σ(sample)` once, you can derive the CRC at any integer sample-offset shift `Δ` via `CRC(Δ) = SA + Δ × SB`. This lets you test thousands of drive-offset candidates in one pass — useful for "find my drive's offset" features. Out of scope for MVP; skip for day 1.
- **Failure modes**: drive offset mismatch, non-audio track fed in, data-track-misidentified-as-audio, and silence-padding differences all produce wrong CRCs. The correct response is "no match found" — don't guess; show the user what was computed vs. expected.
- **Verification is independent of identification**: you can compute AccurateRip CRCs without knowing MusicBrainz DiscID, and vice versa. They answer different questions.

## Sources

- [AccurateRip — Hydrogenaudio Knowledgebase](https://wiki.hydrogenaudio.org/index.php?title=AccurateRip) — user-facing overview of the database and its history.
- [leo-bogert/accuraterip-checksum](https://github.com/leo-bogert/accuraterip-checksum) — the cleanest public C reference implementation of CRC v1 and v2. The entire calculation is in [accuraterip-checksum.c](https://github.com/leo-bogert/accuraterip-checksum/blob/master/accuraterip-checksum.c).
- [arcctgx/ARver](https://github.com/arcctgx/ARver) — a maintained Python implementation. [`arver/audio/checksums.py`](https://github.com/arcctgx/ARver/blob/master/arver/audio/checksums.py) mirrors the C source closely and is easier to read.
- [arcctgx/ARver — database.py](https://github.com/arcctgx/ARver/blob/master/arver/disc/database.py) — dBAR URL construction and binary response parsing.
- [sbooth's AccurateRip gist](https://gist.github.com/sbooth/331559) — Objective-C port with the offset-compensated-sum variant (`SA`, `SB` structure) explicitly written out.
- [Jonas Lundqvist — Calculating AccurateRip checksums (2009)](https://jonls.dk/2009/10/calculating-accuraterip-checksums/) — a blog-length derivation of the v1 algorithm and its overflow bug. Sometimes rate-limited; archive copies exist.
- [dBpoweramp developer forum — AccurateRip CRC Calculation](https://forum.dbpoweramp.com/forum/other-topics/developers-corner/20117-accuraterip-crc-calculation) — original reverse-engineering discussion, referenced by most later implementations.
- [CUETools wiki](http://cue.tools/wiki/Main_Page) — documents CTDB, AccurateRip's more-accurate sibling database. Relevant when adding a secondary verification provider.
