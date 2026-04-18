//! YAML I/O for catalog entities (overrides, seed data).
//!
//! Mirrors the directory-of-YAML-files pattern from retro-junk-catalog.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OverridesFile {
    #[serde(default)]
    pub overrides: Vec<crate::Override>,
}
