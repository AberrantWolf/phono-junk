use std::path::PathBuf;

use phono_junk_lib::{HttpError, SessionError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error("path does not exist: {0}")]
    MissingPath(PathBuf),
    #[error("invalid argument: {0}")]
    InvalidArg(String),
    #[error("could not resolve default database path — pass --db explicitly")]
    NoDbPath,
    #[error("JSON serialisation: {0}")]
    Json(#[from] serde_json::Error),
}
