//! Error type for the inline playback path.
//!
//! Wraps the three distinct failure modes: opening the audio device +
//! mixer track (`kira`), reading PCM from disk (`junk_libs_core`), and
//! handing a streaming sound to kira for playback. `PlayerError` is the
//! sole surface type the GUI layer has to know about — the click handler
//! pipes it straight onto `app.load_error`.

use junk_libs_core::AnalysisError;
use kira::PlaySoundError;
use kira::backend::cpal::Error as CpalError;

#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    #[error("open audio device: {0}")]
    BackendInit(#[from] CpalError),

    #[error("create mixer track: {0}")]
    TrackBuild(String),

    /// PCM source failure — opening the CUE/CHD, reading a sector, or
    /// rejecting a non-audio track. `AnalysisError` already reports the
    /// offending file path inside its message, so no wrapping is needed.
    #[error("{0}")]
    Pcm(#[from] AnalysisError),

    #[error("play sound: {0}")]
    PlaySound(String),
}

impl<T> From<PlaySoundError<T>> for PlayerError
where
    T: std::fmt::Debug,
{
    fn from(err: PlaySoundError<T>) -> Self {
        Self::PlaySound(format!("{err:?}"))
    }
}
