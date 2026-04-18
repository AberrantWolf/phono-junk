//! Convert a generic `TrackLayout` sequence (from `junk-libs-disc`) into
//! an audio-CD `Toc` suitable for `compute_disc_ids`.
//!
//! This is the only audio-CD-specific piece of Sprint 2. The sector-layout
//! math lives in `junk-libs-disc::{cue,chd}::compute_*_layout`; what lives
//! here is the CD-Extra multi-session correction defined by the MusicBrainz
//! DiscID specification:
//!
//! > When a data session follows the audio session, the last entry
//! > reported in the TOC is the data track, not the true audio lead-out.
//! > Exclude the data track from the audio-track count, then subtract
//! > 11,400 frames from the data track's offset to recover the audio
//! > lead-out position used in the DiscID calculation.
//!
//! Source: <https://musicbrainz.org/doc/Disc_ID_Calculation> and
//! `.claude/skills/phono-archive/formats/DiscID.md`.

use junk_libs_disc::{TrackKind, TrackLayout};
use phono_junk_core::{AudioError, Toc};

/// Frames to subtract from a trailing data track's absolute offset to
/// recover the effective audio lead-out on a CD-Extra / Enhanced CD.
const CD_EXTRA_DATA_OFFSET: u32 = 11_400;

/// Convert a sequence of `TrackLayout` entries to an audio-CD `Toc`.
///
/// Rules:
/// 1. Empty input → `InvalidToc`.
/// 2. Partition into a leading audio run and a trailing data run.
///    `TrackKind::Unknown` is treated as audio (conservative).
/// 3. If there are no audio tracks → `InvalidToc("no audio tracks")`.
/// 4. If any data track precedes an audio track → `Unsupported`
///    (leading-data mixed-mode CD; not an audio-CD identification target).
/// 5. No trailing data → `leadout = last_audio.absolute_offset +
///    last_audio.length_sectors`.
/// 6. One or more trailing data tracks → `leadout = first_data.absolute_offset
///    - 11_400`. Data tracks are excluded from `track_offsets`.
/// 7. Validates that track offsets are strictly monotonic and that the
///    leadout lies beyond the last audio-track offset.
pub(crate) fn layout_to_toc(layout: &[TrackLayout]) -> Result<Toc, AudioError> {
    if layout.is_empty() {
        return Err(AudioError::InvalidToc("no tracks in layout".into()));
    }

    let is_audiolike = |t: &TrackLayout| matches!(t.kind, TrackKind::Audio | TrackKind::Unknown);

    let first_data_idx = layout.iter().position(|t| t.kind == TrackKind::Data);

    let (audio, trailing_data): (&[TrackLayout], &[TrackLayout]) = match first_data_idx {
        Some(idx) => {
            // Any audio after this data track → leading-data mixed-mode.
            if layout[idx..].iter().any(is_audiolike) {
                return Err(AudioError::Unsupported(
                    "leading-data mixed-mode CD (data before audio)".into(),
                ));
            }
            (&layout[..idx], &layout[idx..])
        }
        None => (layout, &[][..]),
    };

    if audio.is_empty() {
        return Err(AudioError::InvalidToc("no audio tracks".into()));
    }

    let leadout_sector = if let Some(first_data) = trailing_data.first() {
        first_data
            .absolute_offset
            .checked_sub(CD_EXTRA_DATA_OFFSET)
            .ok_or_else(|| {
                AudioError::InvalidToc(
                    "CD-Extra data-track offset below lead-in threshold".into(),
                )
            })?
    } else {
        let last = audio.last().unwrap();
        last.absolute_offset + last.length_sectors
    };

    let first_track = audio.first().unwrap().number;
    let last_track = audio.last().unwrap().number;
    let track_offsets: Vec<u32> = audio.iter().map(|t| t.absolute_offset).collect();

    let toc = Toc {
        first_track,
        last_track,
        leadout_sector,
        track_offsets,
    };

    validate_toc(&toc)?;
    Ok(toc)
}

fn validate_toc(toc: &Toc) -> Result<(), AudioError> {
    if toc.track_offsets.is_empty() {
        return Err(AudioError::InvalidToc("empty track offsets".into()));
    }
    if toc.first_track > toc.last_track {
        return Err(AudioError::InvalidToc(
            "first_track > last_track".into(),
        ));
    }
    for pair in toc.track_offsets.windows(2) {
        if pair[0] >= pair[1] {
            return Err(AudioError::InvalidToc(
                "track offsets are not strictly monotonic".into(),
            ));
        }
    }
    if toc.leadout_sector <= *toc.track_offsets.last().unwrap() {
        return Err(AudioError::InvalidToc(
            "leadout must be past the last track offset".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/toc_from_layout_tests.rs"]
mod tests;
