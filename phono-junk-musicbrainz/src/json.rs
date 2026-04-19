//! Private deserialization types for MusicBrainz `/ws/2/discid` + Cover Art
//! Archive `/release/<mbid>` JSON responses.
//!
//! Only the fields we actually map into [`phono_junk_identify::ProviderResult`]
//! / [`phono_junk_identify::AssetCandidate`] are modeled here — no speculative
//! coverage. `#[serde(default)]` lets us accept responses where optional
//! fields are omitted entirely (common for `release-group`, `label-info`,
//! `barcode`, etc. depending on which `inc=` flags fired).
//!
//! Schema references (captured 2026-04-18):
//! - MusicBrainz Web Service v2: <https://musicbrainz.org/doc/MusicBrainz_API>
//! - `/ws/2/discid/<id>` inc flags: <https://musicbrainz.org/doc/MusicBrainz_API#Lookups>
//! - Cover Art Archive API: <https://musicbrainz.org/doc/Cover_Art_Archive/API>

use serde::Deserialize;

// ---------- MusicBrainz /ws/2/discid/<id> ----------

#[derive(Debug, Deserialize)]
pub struct DiscidResponse {
    #[serde(default)]
    pub releases: Vec<Release>,
}

#[derive(Debug, Deserialize)]
pub struct Release {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub barcode: Option<String>,
    #[serde(rename = "artist-credit", default)]
    pub artist_credit: Vec<ArtistCredit>,
    #[serde(rename = "label-info", default)]
    pub label_info: Vec<LabelInfo>,
    #[serde(default)]
    pub media: Vec<Medium>,
    #[serde(rename = "release-group", default)]
    pub release_group: Option<ReleaseGroup>,
}

#[derive(Debug, Deserialize)]
pub struct ArtistCredit {
    pub name: String,
    #[serde(default)]
    pub joinphrase: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LabelInfo {
    #[serde(rename = "catalog-number", default)]
    pub catalog_number: Option<String>,
    #[serde(default)]
    pub label: Option<Label>,
}

#[derive(Debug, Deserialize)]
pub struct Label {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct Medium {
    #[serde(default)]
    pub tracks: Vec<Track>,
}

#[derive(Debug, Deserialize)]
pub struct Track {
    pub position: u8,
    pub title: String,
    /// Track length in milliseconds (MB convention).
    #[serde(default)]
    pub length: Option<u64>,
    #[serde(default)]
    pub recording: Option<Recording>,
}

#[derive(Debug, Deserialize)]
pub struct Recording {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct ReleaseGroup {
    pub id: String,
}

// ---------- Cover Art Archive /release/<mbid> ----------

#[derive(Debug, Deserialize)]
pub struct CaaResponse {
    #[serde(default)]
    pub images: Vec<CaaImage>,
}

#[derive(Debug, Deserialize)]
pub struct CaaImage {
    /// Full-size image URL. Thumbnails exist but are ignored in MVP.
    pub image: String,
    #[serde(default)]
    pub front: bool,
    #[serde(default)]
    pub back: bool,
    #[serde(default)]
    pub types: Vec<String>,
}
