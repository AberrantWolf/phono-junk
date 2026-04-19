use std::path::PathBuf;

use phono_junk_lib::{ExportError, IdentifyError, ScanError, VerifyError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)]
    Db(#[from] phono_junk_db::DbError),
    #[error(transparent)]
    DbSchema(#[from] phono_junk_db::SchemaError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Identify(#[from] IdentifyError),
    #[error(transparent)]
    Scan(#[from] ScanError),
    #[error(transparent)]
    Verify(#[from] VerifyError),
    #[error(transparent)]
    Export(#[from] ExportError),
    #[error(transparent)]
    Http(#[from] phono_junk_identify::HttpError),
    #[error("path does not exist: {0}")]
    MissingPath(PathBuf),
    #[error("invalid argument: {0}")]
    InvalidArg(String),
    #[error("could not resolve default database path — pass --db explicitly")]
    NoDbPath,
    #[error("JSON serialisation: {0}")]
    Json(#[from] serde_json::Error),
}
