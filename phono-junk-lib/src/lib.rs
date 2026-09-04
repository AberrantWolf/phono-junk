//! Glue facade shared by CLI and GUI.
//!
//! [`PhonoContext`] registers all identification + asset providers and exposes
//! the single entry-point API (`scan_library`, `identify_disc`, `verify_disc`,
//! `export_disc`) that both CLI and GUI call into.
//!
//! Credentials + rate-limited HTTP client live here for day 1; extracted to
//! `junk-libs` once retro-junk is ready to consume them.

pub mod audit;
pub mod context;
pub mod credentials;
pub mod detail;
pub mod env;
pub mod extract;
pub mod http;
pub mod identify;
pub mod list;
pub mod scan;
pub mod session;
pub mod sidecar;
pub mod verify;

pub use context::PhonoContext;
pub use detail::{
    AlbumDetail, DetailError, DiscDetail, ReleaseDetail, UnidentifiedDetail, load_album_detail,
    load_unidentified_detail,
};
pub use extract::{
    ExportError, ExportedDisc, cache_asset_bytes, find_layout_for_track, load_track_layouts,
    open_pcm_reader,
};
pub use identify::{IdentifiedDisc, IdentifyError};
pub use list::{
    ListEntry, ListFilters, ListRow, UnidentifiedRow, YearSpec, filter_entries, filter_rows,
    load_list_entries, load_list_rows,
};
pub use phono_junk_catalog::{
    Asset, Disagreement, Id, IdentifyAttemptError, RipFile, RipperProvenance,
};
pub use phono_junk_core::{IdentificationConfidence, IdentificationState, Toc};
pub use phono_junk_db::CURRENT_VERSION as CATALOG_SCHEMA_VERSION;
pub use phono_junk_identify::HttpError;
pub use scan::{
    IdentificationDisposition, IngestOutcome, RefreshPolicy, ScanError, ScanEvent, ScanKind,
    ScanRequest, ScanSummary, ingest_path,
};
pub use session::{
    AlbumSummary, JobEvent, JobEventKind, JobId, JobSupervisor, LibrarySession, SessionError,
    SessionGeneration,
};
pub use verify::{VerifiedTrack, VerifyError, VerifySummary, VerifyTarget};
