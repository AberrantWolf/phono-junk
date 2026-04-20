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
pub mod sidecar;
pub mod verify;

pub use context::PhonoContext;
pub use detail::{
    AlbumDetail, DetailError, DiscDetail, ReleaseDetail, UnidentifiedDetail, load_album_detail,
    load_unidentified_detail,
};
pub use extract::{ExportError, ExportedDisc, fetch_asset_bytes};
pub use identify::{IdentifiedDisc, IdentifyError};
pub use list::{
    ListEntry, ListFilters, ListRow, UnidentifiedRow, YearSpec, filter_entries, filter_rows,
    load_list_entries, load_list_rows,
};
pub use scan::{IngestOutcome, ScanError, ScanEvent, ScanKind, ScanOpts, ScanSummary, ingest_path};
pub use verify::{VerifiedTrack, VerifyError, VerifySummary, VerifyTarget};
