# Test fixtures for phono-junk-toc

These fixtures exercise the full CUE → `Toc` → `compute_disc_ids` pipeline
against values from authoritative reference implementations.

## CUE files (committed)

- `arver_3track.cue` — single-FILE audio CUE reproducing ARver's 3-track
  test fixture. Expected absolute offsets `[150, 75408, 130223]`, leadout
  `336103`. DiscIDs: MB `dUmct3Sk4dAt1a98qUKYKC0ZjYU-`, CDDB `19117f03`,
  AR1 `00084264`, AR2 `001cc184`.
  Source:
  <https://github.com/arcctgx/ARver/blob/master/tests/discinfo_test.py>.

- `cd_extra_synth.cue` — same three audio tracks plus a trailing
  `MODE2/2352` data track at `audio_leadout + 11,400` absolute sectors.
  After CD-Extra correction the resulting `Toc` is identical to
  `arver_3track.cue`'s. Source: the MusicBrainz DiscID spec's
  multi-session section, <https://musicbrainz.org/doc/Disc_ID_Calculation>.

## BIN files (generated at test time, not committed)

The integration test creates these lazily via `std::fs::File::set_len`.
On macOS and Linux the files are *sparse*: they occupy only a few KB on
disk but `fs::metadata().len()` reports the full logical size. The TOC
extraction code only needs the file size — it never reads the bytes —
so this is safe and keeps the repository small.

| BIN                    | Logical size (bytes)          |
| ---------------------- | ----------------------------- |
| `arver_3track.bin`     | `335953 × 2352 = 790,161,456` |
| `cd_extra_synth.bin`   | `347953 × 2352 = 818,385,456` |

Both are sized to match their respective CUE sheets: `arver_3track.bin`
holds exactly `335953` sectors (the total audio program), while
`cd_extra_synth.bin` holds the audio program plus the 600-sector data
track and the 11,400-sector session gap implied by CD-Extra.
