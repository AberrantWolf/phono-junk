//! AccurateRip-specific errors. Folds into [`phono_junk_core::AudioError`]
//! at the crate boundary via [`From`].

use phono_junk_core::AudioError;
use phono_junk_identify::HttpError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AccurateRipError {
    #[error("AccurateRip lookup requires a populated `{0}` in DiscIds")]
    MissingId(&'static str),

    #[error("HTTP: {0}")]
    Http(#[from] HttpError),

    #[error("dBAR parse: {0}")]
    Parse(String),
}

impl From<AccurateRipError> for AudioError {
    fn from(e: AccurateRipError) -> Self {
        match e {
            AccurateRipError::Http(h) => AudioError::Network(h.to_string()),
            other => AudioError::Other(other.to_string()),
        }
    }
}
