use phono_junk_core::{IdentificationConfidence, IdentificationSource, Toc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub type Id = i64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub id: Id,
    pub title: String,
    pub sort_title: Option<String>,
    pub artist_credit: Option<String>,
    pub year: Option<u16>,
    pub mbid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub id: Id,
    pub album_id: Id,
    pub country: Option<String>,
    pub date: Option<String>,
    pub label: Option<String>,
    pub catalog_number: Option<String>,
    pub barcode: Option<String>,
    pub mbid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Disc {
    pub id: Id,
    pub release_id: Id,
    pub disc_number: u8,
    pub toc: Option<Toc>,
    pub mb_discid: Option<String>,
    pub cddb_id: Option<String>,
    pub ar_discid1: Option<String>,
    pub ar_discid2: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: Id,
    pub disc_id: Id,
    pub position: u8,
    pub title: Option<String>,
    pub artist_credit: Option<String>,
    pub length_frames: Option<u64>,
    pub isrc: Option<String>,
    pub mbid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RipFile {
    pub id: Id,
    pub disc_id: Option<Id>,
    pub cue_path: Option<PathBuf>,
    pub chd_path: Option<PathBuf>,
    pub bin_paths: Vec<PathBuf>,
    pub mtime: Option<i64>,
    pub size: Option<u64>,
    pub identification_confidence: IdentificationConfidence,
    pub identification_source: Option<IdentificationSource>,
    pub accuraterip_status: Option<String>,
    pub last_verified_at: Option<String>,
}

/// Asset types mirror those in `phono-junk-identify::AssetType`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssetType {
    FrontCover,
    BackCover,
    CdLabel,
    Booklet,
    ObiStrip,
    TrayInsert,
    Other(String),
}

/// An asset — image/scan associated with a release. Ordered assets (booklet
/// pages, multi-disc obi strips) share a `group_id` and are sequenced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub id: Id,
    pub release_id: Id,
    pub asset_type: AssetType,
    pub group_id: Option<Id>,
    pub sequence: u16,
    pub source_url: Option<String>,
    pub file_path: Option<PathBuf>,
    pub scraped_at: Option<String>,
}

/// A field-level conflict between two sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Disagreement {
    pub id: Id,
    pub entity_type: String,
    pub entity_id: Id,
    pub field: String,
    pub source_a: String,
    pub value_a: String,
    pub source_b: String,
    pub value_b: String,
    pub resolved: bool,
}

/// A user-curated override. `sub_path` targets nested fields like
/// `"track[6].title"` or `"tracks[3].artist"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Override {
    pub id: Id,
    pub entity_type: String,
    pub entity_id: Id,
    pub sub_path: Option<String>,
    pub field: String,
    pub override_value: String,
    pub reason: Option<String>,
}
