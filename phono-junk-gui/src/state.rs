//! UI ↔ worker channel messages + `BackgroundOperation` registry + the
//! main-thread message-drain handler.
//!
//! Mirrors retro-junk-gui's pattern (see `retro-junk-gui/src/state.rs`).
//! Four orthogonal message variants cover every dispatch shape the GUI
//! currently spawns; domain-specific variants are deliberately absent —
//! add one only when the main thread genuinely needs to branch on the
//! payload (e.g. "open this identified album's detail panel").

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, atomic::AtomicBool};

use phono_junk_catalog::Id;

use crate::app::PhonoApp;

pub type OperationId = u64;

/// Identifies one row in the unified album / unidentified-rip list for
/// selection tracking. Keying by kind lets bulk actions dispatch to the
/// right backend (identify-unidentified vs re-identify/re-verify/export).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKey {
    Album(Id),
    RipFile(Id),
}

impl EntryKey {
    pub fn album_id(self) -> Option<Id> {
        match self {
            EntryKey::Album(id) => Some(id),
            EntryKey::RipFile(_) => None,
        }
    }

    pub fn rip_file_id(self) -> Option<Id> {
        match self {
            EntryKey::RipFile(id) => Some(id),
            EntryKey::Album(_) => None,
        }
    }
}

#[derive(Debug)]
pub enum AppMessage {
    /// Worker pushed a progress tick for an in-flight operation.
    ///
    /// `total == 0` means "indeterminate" — the activity bar draws a
    /// spinner + counter but no progress bar.
    OperationProgress {
        op_id: OperationId,
        current: u64,
        total: u64,
        note: Option<String>,
    },
    /// Worker finished cleanly. The main thread drops the matching
    /// [`BackgroundOperation`] from [`PhonoApp::operations`].
    OperationComplete { op_id: OperationId },
    /// Worker bailed with an error. Behaves like `OperationComplete`
    /// except the error surfaces on `app.load_error`.
    OperationFailed {
        op_id: OperationId,
        error: String,
    },
    /// DB state changed in a way that the album list should observe.
    /// Triggers a `reload_rows` on the main thread.
    LibraryChanged,
    /// Free-form status line for the toolbar — e.g. the summary of the
    /// most recent scan. Overwrites any previous status.
    Status(String),
}

/// A background operation as tracked on `PhonoApp.operations` and drawn
/// by the activity bar.
pub struct BackgroundOperation {
    pub id: OperationId,
    pub description: String,
    pub progress_current: u64,
    pub progress_total: u64,
    /// Optional per-item label (e.g. "Exporting Weezer — Pinkerton").
    pub note: Option<String>,
    pub cancel_token: Arc<AtomicBool>,
}

impl BackgroundOperation {
    pub fn new(id: OperationId, description: String, cancel_token: Arc<AtomicBool>) -> Self {
        Self {
            id,
            description,
            progress_current: 0,
            progress_total: 0,
            note: None,
            cancel_token,
        }
    }

    /// 0.0 when `progress_total == 0`, otherwise clamped to `[0.0, 1.0]`.
    pub fn progress_fraction(&self) -> f32 {
        if self.progress_total == 0 {
            return 0.0;
        }
        let raw = self.progress_current as f32 / self.progress_total as f32;
        raw.clamp(0.0, 1.0)
    }
}

static NEXT_OP_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_operation_id() -> OperationId {
    NEXT_OP_ID.fetch_add(1, Ordering::Relaxed)
}

/// Apply one incoming [`AppMessage`] to `app`. Called by the update loop
/// after draining `message_rx`; requests a repaint so workers that post
/// while the window is idle still cause a redraw.
pub fn handle_message(app: &mut PhonoApp, msg: AppMessage, ctx: &egui::Context) {
    match msg {
        AppMessage::OperationProgress {
            op_id,
            current,
            total,
            note,
        } => {
            if let Some(op) = app.operations.iter_mut().find(|o| o.id == op_id) {
                op.progress_current = current;
                op.progress_total = total;
                op.note = note;
            }
        }
        AppMessage::OperationComplete { op_id } => {
            app.operations.retain(|o| o.id != op_id);
        }
        AppMessage::OperationFailed { op_id, error } => {
            app.operations.retain(|o| o.id != op_id);
            app.load_error = Some(error);
        }
        AppMessage::LibraryChanged => {
            app.reload_rows();
        }
        AppMessage::Status(s) => {
            app.status_message = Some(s);
        }
    }
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn mk_op(id: u64) -> BackgroundOperation {
        BackgroundOperation::new(id, format!("op {id}"), Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn progress_fraction_zero_total_is_zero() {
        let op = mk_op(1);
        assert_eq!(op.progress_fraction(), 0.0);
    }

    #[test]
    fn progress_fraction_halfway() {
        let mut op = mk_op(1);
        op.progress_current = 5;
        op.progress_total = 10;
        assert!((op.progress_fraction() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn progress_fraction_clamps_over_unity() {
        let mut op = mk_op(1);
        op.progress_current = 20;
        op.progress_total = 10;
        assert_eq!(op.progress_fraction(), 1.0);
    }

    #[test]
    fn next_operation_id_is_monotonic() {
        let a = next_operation_id();
        let b = next_operation_id();
        assert!(b > a);
    }

    #[test]
    fn handle_message_progress_mutates_existing_entry() {
        let mut app = PhonoApp::new();
        app.operations.push(mk_op(42));
        let ctx = egui::Context::default();
        handle_message(
            &mut app,
            AppMessage::OperationProgress {
                op_id: 42,
                current: 3,
                total: 10,
                note: Some("hello".into()),
            },
            &ctx,
        );
        let op = &app.operations[0];
        assert_eq!(op.progress_current, 3);
        assert_eq!(op.progress_total, 10);
        assert_eq!(op.note.as_deref(), Some("hello"));
    }

    #[test]
    fn handle_message_complete_drops_entry() {
        let mut app = PhonoApp::new();
        app.operations.push(mk_op(1));
        app.operations.push(mk_op(2));
        let ctx = egui::Context::default();
        handle_message(&mut app, AppMessage::OperationComplete { op_id: 1 }, &ctx);
        assert_eq!(app.operations.len(), 1);
        assert_eq!(app.operations[0].id, 2);
    }

    #[test]
    fn handle_message_failed_drops_entry_and_sets_error() {
        let mut app = PhonoApp::new();
        app.operations.push(mk_op(1));
        let ctx = egui::Context::default();
        handle_message(
            &mut app,
            AppMessage::OperationFailed {
                op_id: 1,
                error: "boom".into(),
            },
            &ctx,
        );
        assert!(app.operations.is_empty());
        assert_eq!(app.load_error.as_deref(), Some("boom"));
    }

    #[test]
    fn handle_message_library_changed_is_safe_without_db() {
        let mut app = PhonoApp::new();
        let ctx = egui::Context::default();
        handle_message(&mut app, AppMessage::LibraryChanged, &ctx);
        // No DB open — reload_rows short-circuits; no panic, no error.
        assert!(app.load_error.is_none());
    }

    #[test]
    fn entry_key_projects_to_kind_specific_ids() {
        assert_eq!(EntryKey::Album(3).album_id(), Some(3));
        assert_eq!(EntryKey::Album(3).rip_file_id(), None);
        assert_eq!(EntryKey::RipFile(9).album_id(), None);
        assert_eq!(EntryKey::RipFile(9).rip_file_id(), Some(9));
    }
}
