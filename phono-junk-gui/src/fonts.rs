//! Pan-script font configuration.
//!
//! Unlike retro-junk-gui's feature-gated CJK, phono-junk ships the full
//! multi-script Noto bundle as the default, baseline font layer. Foreign
//! discs (Thai, Korean, Chinese, Japanese, Arabic, Hindi) are the whole
//! point — requiring a rebuild to see them is wrong.
//!
//! TODO: embed actual font bytes via `include_bytes!` once assets land:
//! - NotoSans (Latin + extended)
//! - NotoSansCJK (Japanese, Simplified & Traditional Chinese, Korean)
//! - NotoSansThai
//! - NotoSansArabic
//! - NotoSansDevanagari

pub fn configure_fonts(_ctx: &egui::Context) {
    // TODO: load fonts via egui::FontDefinitions and install as fallbacks
    // in both Proportional and Monospace families.
}
