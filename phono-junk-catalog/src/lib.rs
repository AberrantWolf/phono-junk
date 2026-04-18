//! Catalog data model: Album / Release / Disc / Track / RipFile / Asset /
//! Disagreement / Override.
//!
//! Audio-native shape — per-track entities, ordered asset groups (booklet
//! pages), identification-confidence and source on [`RipFile`] (so
//! "unidentified" is a first-class state), sub-path targeting on [`Override`]
//! (e.g. `track[6].title`).

pub mod types;
pub mod yaml;

pub use types::*;
