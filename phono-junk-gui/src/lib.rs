//! phono-junk GUI (egui/eframe).
//!
//! Follows retro-junk-gui's patterns verbatim: std::mpsc channel + background
//! threads via `spawn_background_op`, `AppMessage` enum for worker→UI
//! communication, `BackgroundOperation` tracking for the activity bar.
//!
//! Diverges in two ways by design:
//! 1. Pan-script fonts are loaded unconditionally — no `cjk-full` feature
//!    gate. Foreign discs are the whole point.
//! 2. Structured search/filter bar (artist / year / genre / language) ships
//!    in v1, not bolted on.

pub mod app;
pub mod backend;
pub mod fonts;
pub mod state;
pub mod views;

pub use app::PhonoApp;
