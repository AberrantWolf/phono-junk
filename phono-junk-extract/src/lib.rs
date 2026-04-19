//! BIN/CHD → per-track FLAC transcode + tag pipeline.
//!
//! Primitive layer: pure FLAC encoding, Vorbis comments, picture-block
//! attachment, and path planning. No DB reads, no HTTP fetch. The
//! `phono-junk-lib::extract` facade composes catalog reads and asset
//! fetching on top of this crate.
//!
//! Output layout: `<library>/<AlbumArtist>/<Album> (<Year>)/NN - Title.flac`
//! plus `cover.jpg` (single-disc), or `.../<Album> (<Year>)/Disc N/NN - …`
//! (multi-disc).
//!
//! Vorbis tag spec (12 canonical + ISRC): `ALBUM`, `ALBUMARTIST`, `ARTIST`,
//! `TITLE`, `TRACKNUMBER`, `TOTALTRACKS`, `DISCNUMBER`, `TOTALDISCS`,
//! `DATE`, `GENRE`, `MUSICBRAINZ_ALBUMID`, `MUSICBRAINZ_RELEASETRACKID`,
//! `ISRC`. Front cover embedded as `METADATA_BLOCK_PICTURE` (picture
//! type 3, `image/jpeg`).

pub mod encode;
pub mod error;
pub mod paths;
pub mod tags;

pub use encode::encode_flac_track;
pub use error::ExtractError;
pub use paths::{
    album_artist_component, album_folder_name, plan_disc_directory, plan_output_paths,
    sanitize_path_component, track_file_name,
};
pub use tags::TrackTags;
