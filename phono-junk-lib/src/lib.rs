//! Glue facade shared by CLI and GUI.
//!
//! [`PhonoContext`] registers all identification + asset providers and exposes
//! the single entry-point API (`scan_library`, `identify_disc`, `verify_disc`,
//! `export_discs`) that both CLI and GUI call into.
//!
//! Credentials + rate-limited HTTP client live here for day 1; extracted to
//! `junk-libs` once retro-junk is ready to consume them.

pub mod context;
pub mod credentials;
pub mod extract;
pub mod http;
pub mod identify;

pub use context::PhonoContext;
pub use extract::{ExportError, ExportedDisc};
pub use identify::{IdentifiedDisc, IdentifyError};
