//! Errors produced by the extract pipeline.

use std::path::PathBuf;

use junk_libs_core::AnalysisError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExtractError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Analysis(#[from] AnalysisError),

    #[error("FLAC encoder init failed: {0}")]
    FlacInit(String),

    #[error("FLAC encode failed in state: {0}")]
    FlacEncode(String),

    #[error("FLAC metadata (tag/picture) write failed: {0}")]
    FlacMetadata(String),

    #[error("missing rip source: disc has neither a CUE nor a CHD path")]
    MissingRipSource,

    #[error("invalid track data: {0}")]
    InvalidTrack(String),
}

impl ExtractError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
