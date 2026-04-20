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
    pub primary_type: Option<String>,
    #[serde(default)]
    pub secondary_types: Vec<String>,
    pub first_release_date: Option<String>,
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
    pub status: Option<String>,
    /// ISO 639-3 language code from MB `text-representation.language`
    /// (e.g. `jpn`, `kor`, `zho`, `eng`). Drives CJK font region selection.
    pub language: Option<String>,
    /// ISO 15924 script code from MB `text-representation.script`
    /// (e.g. `Jpan`, `Hans`, `Hant`, `Hang`, `Latn`).
    pub script: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Disc {
    pub id: Id,
    pub release_id: Id,
    pub disc_number: u8,
    pub format: String,
    pub toc: Option<Toc>,
    pub mb_discid: Option<String>,
    pub cddb_id: Option<String>,
    pub ar_discid1: Option<String>,
    pub ar_discid2: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dbar_raw: Option<Vec<u8>>,
}

impl Default for Disc {
    fn default() -> Self {
        Self {
            id: 0,
            release_id: 0,
            disc_number: 1,
            format: "CD".to_string(),
            toc: None,
            mb_discid: None,
            cddb_id: None,
            ar_discid1: None,
            ar_discid2: None,
            dbar_raw: None,
        }
    }
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
    pub recording_mbid: Option<String>,
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
    /// Per-provider error log from the most recent identify attempt.
    /// Persisted so the GUI can explain *why* an unidentified rip didn't match
    /// without forcing the user to re-run identify just to read errors.
    /// Messages are humanized at the boundary (see
    /// `phono_junk_lib::identify::humanize_provider_error`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_identify_errors: Option<Vec<IdentifyAttemptError>>,
    pub last_identify_at: Option<String>,
}

/// One row of "what each provider said" from the most recent identify attempt.
/// Stored as JSON in `rip_files.last_identify_errors`.
///
/// Has no `kind` discriminant — the human message carries enough context for
/// the UI, and keeping this catalog-level type free of trait-crate dependencies
/// preserves the layering (catalog never depends on `phono-junk-identify`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentifyAttemptError {
    pub provider: String,
    pub message: String,
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

impl AssetType {
    /// Canonical string form used when persisting to SQLite. `Other(tag)` is
    /// encoded as `other:<tag>`; the `other:` prefix is reserved so round-trip
    /// parsing is unambiguous.
    pub fn as_db_str(&self) -> String {
        match self {
            AssetType::FrontCover => "front_cover".into(),
            AssetType::BackCover => "back_cover".into(),
            AssetType::CdLabel => "cd_label".into(),
            AssetType::Booklet => "booklet".into(),
            AssetType::ObiStrip => "obi_strip".into(),
            AssetType::TrayInsert => "tray_insert".into(),
            AssetType::Other(tag) => format!("other:{tag}"),
        }
    }

    pub fn from_db_str(s: &str) -> Self {
        match s {
            "front_cover" => AssetType::FrontCover,
            "back_cover" => AssetType::BackCover,
            "cd_label" => AssetType::CdLabel,
            "booklet" => AssetType::Booklet,
            "obi_strip" => AssetType::ObiStrip,
            "tray_insert" => AssetType::TrayInsert,
            other => AssetType::Other(other.strip_prefix("other:").unwrap_or(other).to_string()),
        }
    }
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

/// Pick the canonical front-cover from a release's assets.
///
/// Filters to `AssetType::FrontCover` then picks the lowest
/// `(group_id, sequence, id)` so multi-page front-cover groups (rare, but
/// some MB releases tag both a digipak and a sleeve) yield a stable choice.
/// `None` group_ids sort last via `i64::MAX` so explicit groupings beat
/// implicit ones.
///
/// Single canonical implementation per CLAUDE.md "one implementation per
/// algorithm" — consumed by both `phono-junk-extract` (FLAC art embed) and
/// `phono-junk-lib::detail` (GUI cover-art block).
pub fn pick_front_cover(assets: &[Asset]) -> Option<&Asset> {
    assets
        .iter()
        .filter(|a| a.asset_type == AssetType::FrontCover)
        .min_by_key(|a| (a.group_id.unwrap_or(i64::MAX), a.sequence, a.id))
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

/// A user-curated override. `sub_path` targets nested fields like
/// `"track[6].title"`. Grammar + application live in `phono-junk-db::overrides`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Override {
    pub id: Id,
    pub entity_type: String,
    pub entity_id: Id,
    pub sub_path: Option<String>,
    pub field: String,
    pub override_value: String,
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}
