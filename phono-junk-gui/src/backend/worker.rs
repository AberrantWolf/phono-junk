//! Standard boilerplate for spawning cancellable background operations.
//!
//! Mirrors `retro-junk-gui/src/backend/worker.rs`: allocates an [`OperationId`],
//! creates an `Arc<AtomicBool>` cancellation token, clones the channel sender,
//! and spawns a thread that receives `(op_id, cancel, tx)`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use crate::state::{AppMessage, OperationId};

static NEXT_OP_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_operation_id() -> OperationId {
    NEXT_OP_ID.fetch_add(1, Ordering::Relaxed)
}

pub fn spawn_background_op<F>(tx: mpsc::Sender<AppMessage>, f: F) -> (OperationId, Arc<AtomicBool>)
where
    F: FnOnce(OperationId, Arc<AtomicBool>, mpsc::Sender<AppMessage>) + Send + 'static,
{
    let op_id = next_operation_id();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = Arc::clone(&cancel);
    thread::spawn(move || f(op_id, cancel_clone, tx));
    (op_id, cancel)
}
