//! Private deserialization types for the iTunes Search API
//! (`https://itunes.apple.com/search?...&entity=album`).
//!
//! Only the fields we map into [`phono_junk_identify::AssetCandidate`] are
//! modeled. Reference (captured 2026-04-18):
//! <https://developer.apple.com/library/archive/documentation/AudioVideo/Conceptual/iTuneSearchAPI/>

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    #[serde(default)]
    pub results: Vec<SearchHit>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// iTunes' 100-pixel artwork URL; rewrite to 1000x1000 before use.
    #[serde(default)]
    pub artwork_url100: Option<String>,
}
