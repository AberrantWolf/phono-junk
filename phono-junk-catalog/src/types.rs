use chrono::{DateTime, Utc};
use junk_libs_disc::redumper::{DriveInfo, Ripper};
use phono_junk_core::{IdentificationConfidence, IdentificationSource, IdentificationState, Toc};
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
    /// Media Catalog Number as encoded in the disc's subchannel Q data —
    /// a *physical-disc fact*, distinct from [`Release::barcode`] which
    /// reflects what metadata databases report. Usually equal; a
    /// mismatch is real information (bootleg, mispressing, regional
    /// variant) and is surfaced via the [`Disagreement`] machinery.
    /// Populated on ingest when a redumper log sidecar carries an MCN.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcn: Option<String>,
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
            mcn: None,
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
    /// Provenance recorded from a ripper-specific sidecar (e.g. redumper's
    /// `.log`): which ripper produced the rip, on what drive, at what
    /// read offset, on what date. `None` means "we haven't detected any
    /// ripper-specific marker" — distinct from
    /// [`Ripper::Unknown`](junk_libs_disc::redumper::Ripper::Unknown)
    /// which means "a log was present but its format didn't match any
    /// known ripper." Unknown rippers still get a `Some` with empty
    /// fields so the audit query can flag them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<RipperProvenance>,
    /// Explicit lifecycle state: queued / working / identified / unidentified
    /// / failed. Distinct from [`IdentificationConfidence`] — state is about
    /// the pipeline ("has identify run yet?"), confidence about the match
    /// ("how trustworthy is it?"). Drives the Status column in the GUI and
    /// separates "no provider matched" from "we haven't tried yet."
    #[serde(default)]
    pub identification_state: IdentificationState,
    /// RFC3339 timestamp of the last `identification_state` transition.
    /// Used by the GUI to sort "freshly queued" rows first and to show
    /// "working since …" labels; not load-bearing for correctness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_state_change_at: Option<String>,
}

/// How a rip was produced — metadata that *only* the ripper knows
/// (not derivable from the PCM / CUE / CHD itself).
///
/// Drives the library-audit view ("which rips lack redumper provenance?")
/// and informs future re-rip suggestions.
///
/// Constructed from a parsed [`junk_libs_disc::redumper::RedumperLog`]
/// by the scan pipeline; other rippers (EAC, XLD, …) will populate the
/// same struct once their log formats get parsers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RipperProvenance {
    pub ripper: Ripper,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive: Option<DriveInfo>,
    /// Combined read offset applied during ripping, in audio samples.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_offset: Option<i32>,
    pub log_path: PathBuf,
    /// When the rip was produced. Parsed from the log's timestamp by the
    /// scan pipeline; `None` when the log doesn't record one or the value
    /// failed to parse as a datetime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rip_date: Option<DateTime<Utc>>,
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

/// A folder the user has told phono-junk to treat as a library root —
/// scanned automatically whenever the catalog DB is opened. Storing
/// these in the DB (rather than in a settings file) keeps the association
/// "this library tracks these folders" so a user with multiple catalogs
/// doesn't accidentally rescan the wrong tree. Sprint 27.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryFolder {
    pub id: Id,
    pub path: PathBuf,
    /// RFC3339 timestamp from `datetime('now')` when the row was first
    /// inserted. Re-adding an existing path leaves the original value.
    pub added_at: Option<String>,
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
