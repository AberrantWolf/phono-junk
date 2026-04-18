//! UI ↔ worker channel messages.
//!
//! Every long-running operation dispatched by the GUI sends progress and
//! completion events back via [`AppMessage`]. Mirrors retro-junk-gui's
//! pattern (see `retro-junk-gui/src/state.rs`).

use std::sync::{Arc, atomic::AtomicBool};

pub type OperationId = u64;

#[derive(Debug)]
pub enum AppMessage {
    OperationProgress {
        op_id: OperationId,
        current: u64,
        total: u64,
    },
    OperationComplete {
        op_id: OperationId,
    },
    DiscIdentified {
        disc_path: std::path::PathBuf,
        result: Result<String, String>,
    },
    DiscVerified {
        disc_path: std::path::PathBuf,
        result: Result<String, String>,
    },
    ExportProgress {
        op_id: OperationId,
        current: u64,
        total: u64,
    },
    ExportComplete {
        op_id: OperationId,
        written_paths: Vec<std::path::PathBuf>,
    },
    Error {
        message: String,
    },
}

pub struct BackgroundOperation {
    pub id: OperationId,
    pub description: String,
    pub progress_current: u64,
    pub progress_total: u64,
    pub progress_is_bytes: bool,
    pub cancel_token: Arc<AtomicBool>,
}
