//! Background workers — disc scan, identification, verification, export.
//!
//! Long-running catalog work is queued on `LibrarySession`'s single owned
//! supervisor. The only local exception is audio playback, which reads PCM
//! through `junk-libs-disc` and never mutates catalog state.

pub mod detail;
pub mod export;
pub mod identify;
pub mod player;
pub mod scan;
pub mod verify;
