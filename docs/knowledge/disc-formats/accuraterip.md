# AccurateRip CRC Verification

AccurateRip is a community-maintained database of per-track CD audio checksums. Its purpose is **verification** — confirming that a local rip is bit-identical to the same track ripped by other people with correctly-offset drives. It does not identify discs (that's what [disc IDs](disc-ids.md) are for); given a disc ID, AccurateRip returns N expected checksums per track and a "confidence" count of how many submitters produced that checksum.

A track with confidence `≥ 2` from independent submitters is strong evidence that your rip is bit-perfect.

## Two versions: CRC v1 and v2

AccurateRip originally used a single checksum (v1). The v1 algorithm has a known flaw — about 3% of the right-channel data is effectively ignored due to a 32-bit multiplication overflow truncation. v2 fixes this by accumulating both halves of the 64-bit product.

Modern tools compute both variants, but a dBAR track entry stores one unlabeled
primary checksum. It may be either ARv1 or ARv2. Verification therefore compares
the locally computed ARv2 first and ARv1 second against that same primary field.
This is the behavior implemented independently by CUETools and ARver.

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

Given the three [disc IDs](disc-ids.md), fetch:

```
https://www.accuraterip.com/accuraterip/<id1_last>/<id1_2nd_last>/<id1_3rd_last>/dBAR-<NNN>-<id1>-<id2>-<cddbid>.bin
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
        u32  checksum                    # primary: ARv1 or ARv2, unlabeled
        u32  checksum_450                # frame-450 offset evidence
        # total 9 bytes per track
}
```

A single disc may have multiple Responses stacked in one `.bin`. Compare both
local algorithms with `checksum`; `checksum_450` is a separate checksum around
frame 450 used to help find offsets. Equality with `checksum_450` alone never
verifies a full track.

### Frame-450 checksum

CUETools computes the partial value from exactly one CDDA frame beginning at
sample `450 * 588` of the offset-adjusted track. The 588 packed stereo samples
are weighted `1..=588` with the ARv1 32-bit wrapping multiply-and-sum. Moving
the source window through the reconstructed disc stream makes this a cheap
offset locator. It narrows the candidates that need a full ARv2 fold, but it is
not an alternate full-track checksum and cannot establish accuracy by itself.
This interpretation comes from the `CRC450` accumulation and offset lookup in
[CUETools' `AccurateRip.cs`](https://github.com/gchudov/cuetools.net/blob/master/CUETools.AccurateRip/AccurateRip.cs), and is consistent with ARver's separate
primary/partial database fields.

### Interpreting "confidence"

The confidence byte is the number of submitters whose rips produced this exact checksum. Common rubric:

- `1–2`: weak — could be coincidence or correlated errors
- `3–9`: good match
- `10+`: very high confidence, effectively canonical
- `200+`: saturated at max value (popular CDs)

Reporting this raw number to the user is more useful than thresholding it; they can decide for themselves.

## Implementation notes (for `phono-junk-accuraterip`)

- **Streaming is fine for zero offset**: compute both v1 and v2 in a single pass over the PCM. Exhaustive offset search reconstructs the disc stream so internal tracks can borrow adjacent samples.
- **Handle the triple-skip case**: single-track discs apply both the start-skip (2940) and end-skip (2940) simultaneously. Most rips won't hit this, but test for it.
- **Offset search**: CUETools searches `-2939..=2939` stereo samples (`5*588-1`). In phono-junk, a positive shift means selecting later PCM samples from the reconstructed disc stream. Internal-track checks must borrow samples from the adjacent tracks while preserving the first/last-disc five-frame exclusion.
- **Offset acceleration**: weighted sums can be updated with cumulative sums, keeping the ARv1/frame-450 search linear in PCM size plus tracks × offset range rather than rereading every track for every shift. Only shifts with primary or frame-450 evidence need the nonlinear ARv2 high-word fold.
- **Failure modes**: drive offset mismatch, non-audio track fed in, data-track-misidentified-as-audio, and silence-padding differences all produce wrong CRCs. The correct response is "no match found" — don't guess; show the user what was computed vs. expected.
- **Verification is independent of identification**: you can compute AccurateRip CRCs without knowing MusicBrainz DiscID, and vice versa. They answer different questions.

## Sources

- [AccurateRip — Hydrogenaudio Knowledgebase](https://wiki.hydrogenaudio.org/index.php?title=AccurateRip) — user-facing overview of the database and its history.
- [leo-bogert/accuraterip-checksum](https://github.com/leo-bogert/accuraterip-checksum) — the cleanest public C reference implementation of CRC v1 and v2. The entire calculation is in [accuraterip-checksum.c](https://github.com/leo-bogert/accuraterip-checksum/blob/master/accuraterip-checksum.c).
- [arcctgx/ARver](https://github.com/arcctgx/ARver) — a maintained Python implementation. [`arver/audio/checksums.py`](https://github.com/arcctgx/ARver/blob/master/arver/audio/checksums.py) mirrors the C source closely and is easier to read.
- [arcctgx/ARver — database.py](https://github.com/arcctgx/ARver/blob/master/arver/disc/database.py) — dBAR URL construction and binary response parsing.
- [CUETools AccurateRip.cs](https://github.com/gchudov/cuetools.net/blob/master/CUETools.AccurateRip/AccurateRip.cs) — primary checksum matching and the distinct frame-450 field.
- [sbooth's AccurateRip gist](https://gist.github.com/sbooth/331559) — Objective-C port with the offset-compensated-sum variant (`SA`, `SB` structure) explicitly written out.
- [Jonas Lundqvist — Calculating AccurateRip checksums (2009)](https://jonls.dk/2009/10/calculating-accuraterip-checksums/) — a blog-length derivation of the v1 algorithm and its overflow bug. Sometimes rate-limited; archive copies exist.
- [dBpoweramp developer forum — AccurateRip CRC Calculation](https://forum.dbpoweramp.com/forum/other-topics/developers-corner/20117-accuraterip-crc-calculation) — original reverse-engineering discussion, referenced by most later implementations.
- [CUETools wiki](http://cue.tools/wiki/Main_Page) — documents CTDB, AccurateRip's more-accurate sibling database. Relevant when adding a secondary verification provider.
