//! BIN/CHD → per-track FLAC transcode + tag pipeline.
//!
//! Output layout: `<library>/<AlbumArtist>/<Album> (<Year>)/NN - Title.flac`
//! plus `cover.jpg`. Vorbis comments include `MUSICBRAINZ_ALBUMID`,
//! `MUSICBRAINZ_RELEASETRACKID`, `ARTIST`, `ALBUMARTIST`, `TITLE`,
//! `TRACKNUMBER`, `TOTALTRACKS`, `DISCNUMBER`, `TOTALDISCS`, `DATE`, `GENRE`.
//! Front cover embedded as `METADATA_BLOCK_PICTURE`.

use phono_junk_core::AudioError;
use std::path::Path;

/// Extract a disc to per-track FLAC files at `target`.
pub fn extract_disc_to_flac(
    _rip_cue_or_chd: &Path,
    _target_dir: &Path,
) -> Result<Vec<std::path::PathBuf>, AudioError> {
    // TODO: read PCM via junk-libs-disc, encode to FLAC, embed tags + cover
    Err(AudioError::Unsupported(
        "extract_disc_to_flac not implemented".into(),
    ))
}
