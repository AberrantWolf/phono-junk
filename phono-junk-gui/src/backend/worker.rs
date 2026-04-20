//! Standard boilerplate for spawning cancellable background operations.
//!
//! Mirrors `retro-junk-gui/src/backend/worker.rs`: allocates an
//! [`OperationId`], creates an `Arc<AtomicBool>` cancellation token,
//! registers the operation on [`PhonoApp::operations`], clones the
//! channel sender, and spawns a thread that receives
//! `(op_id, cancel, tx)`.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::thread;

use crate::app::PhonoApp;
use crate::state::{AppMessage, BackgroundOperation, OperationId, next_operation_id};

/// Spawn a background operation and register it on the app's activity
/// bar. Returns the allocated [`OperationId`] so callers can correlate
/// later messages.
///
/// The closure receives `(op_id, cancel_token, tx)`. It owns `tx` and is
/// expected to send at least an [`AppMessage::OperationComplete`] (or
/// [`AppMessage::OperationFailed`]) before dropping it, so the main
/// thread can retire the activity-bar entry.
pub fn spawn_background_op<F>(app: &mut PhonoApp, description: String, work: F) -> OperationId
where
    F: FnOnce(OperationId, Arc<AtomicBool>, mpsc::Sender<AppMessage>) + Send + 'static,
{
    let op_id = next_operation_id();
    let cancel = Arc::new(AtomicBool::new(false));
    let tx = app.message_tx.clone();

    app.operations
        .push(BackgroundOperation::new(op_id, description, Arc::clone(&cancel)));

    thread::spawn(move || {
        work(op_id, cancel, tx);
    });

    op_id
}
