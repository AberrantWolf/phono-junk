//! Serialized identify-queue worker.
//!
//! Provider rate limits (MB 1/s, Discogs 60/min, etc.) forbid parallel
//! fan-out across rip files — the scan dispatcher streams rip-file ids
//! onto this queue and a single background worker drains them one at a
//! time via [`phono_junk_lib::scan::identify_one`]. State transitions
//! (Queued → Working → Identified / Unidentified / Failed) happen inside
//! `identify_one`; this module just orchestrates the worker lifecycle
//! and surfaces progress to the activity bar.
//!
//! The worker thread + channel are lazily created on first enqueue and
//! stay alive for the rest of the process. The *activity-bar entry*,
//! on the other hand, cycles per burst: a new [`OperationId`] is
//! allocated on every idle→active transition (announced via
//! [`AppMessage::OperationStarted`]), and `OperationComplete` is sent
//! the moment the in-flight count returns to zero. That way a long-
//! idle queue doesn't leave a stuck 100%-full progress bar, and a
//! scan that enqueues N rips sees exactly one "Identifying queued
//! rips — N/N" entry appear and disappear.

use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Sender, SyncSender},
};
use std::thread;

use phono_junk_catalog::Id;
use phono_junk_db::open_database;
use phono_junk_lib::PhonoContext;

use crate::state::{AppMessage, OperationId, next_operation_id};

#[derive(Debug)]
struct QueueItem {
    rip_file_id: Id,
}

/// Per-burst lifecycle state. `active_op` flips from `None` (idle) to
/// `Some` on the first enqueue after idle, and back to `None` when the
/// worker finishes the last item. Guarded by a `Mutex` so enqueue and
/// worker don't race on the transitions.
#[derive(Default)]
struct QueueState {
    in_flight: u64,
    total_seen: u64,
    active_op: Option<(OperationId, Arc<AtomicBool>)>,
}

struct QueueHandle {
    sender: SyncSender<QueueItem>,
    state: Arc<Mutex<QueueState>>,
    ui_tx: Sender<AppMessage>,
}

static QUEUE: OnceLock<Mutex<Option<QueueHandle>>> = OnceLock::new();

fn handle_slot() -> &'static Mutex<Option<QueueHandle>> {
    QUEUE.get_or_init(|| Mutex::new(None))
}

/// Push a rip-file id onto the identify queue. Starts the worker thread
/// on first call. A bursty batch of enqueues produces exactly one
/// activity-bar entry, announced via `OperationStarted` on the first
/// idle→active transition.
pub fn enqueue_for_identify(
    ui_tx: Sender<AppMessage>,
    phono_ctx: Arc<PhonoContext>,
    db_path: PathBuf,
    rip_file_id: Id,
) {
    let slot = handle_slot();
    let mut guard = match slot.lock() {
        Ok(g) => g,
        Err(_) => return, // poisoned — nothing we can do cleanly
    };
    if guard.is_none() {
        *guard = Some(start_queue(ui_tx.clone(), phono_ctx, db_path));
    }
    let handle = guard.as_ref().unwrap();

    // Idle→active transition + counter bump inside a single locked
    // section so a scan-burst enqueue and a rare-concurrent bulk-Identify
    // can't both try to allocate op_ids at once.
    let mut s = match handle.state.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if s.active_op.is_none() {
        let op_id = next_operation_id();
        let cancel = Arc::new(AtomicBool::new(false));
        s.active_op = Some((op_id, cancel.clone()));
        // Reset per-burst totals so the activity-bar counter restarts
        // at 0/N rather than accumulating across bursts.
        s.in_flight = 0;
        s.total_seen = 0;
        let _ = handle.ui_tx.send(AppMessage::OperationStarted {
            op_id,
            description: "Identifying queued rips".into(),
            cancel_token: cancel,
        });
    }
    s.in_flight += 1;
    s.total_seen += 1;
    let op_id = s.active_op.as_ref().map(|(id, _)| *id).unwrap_or(0);
    let total = s.total_seen;
    let current = s.total_seen.saturating_sub(s.in_flight);
    drop(s);

    let _ = handle.ui_tx.send(AppMessage::OperationProgress {
        op_id,
        current,
        total,
        note: None,
    });
    if handle.sender.send(QueueItem { rip_file_id }).is_err() {
        // Channel closed — worker crashed. Reset the slot so the next
        // enqueue starts fresh.
        *guard = None;
    }
}

fn start_queue(
    ui_tx: Sender<AppMessage>,
    phono_ctx: Arc<PhonoContext>,
    db_path: PathBuf,
) -> QueueHandle {
    // Bounded channel to apply a little back-pressure when a huge scan
    // fires N thousand ingest-metadata events in a burst. 256 is enough
    // that scan isn't blocked under normal workloads.
    let (tx, rx) = mpsc::sync_channel::<QueueItem>(256);
    let state = Arc::new(Mutex::new(QueueState::default()));

    let ui_tx_for_worker = ui_tx.clone();
    let state_for_worker = state.clone();

    thread::spawn(move || {
        let conn = match open_database(&db_path) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("identify queue: open db: {e}");
                return;
            }
        };

        while let Ok(item) = rx.recv() {
            // Check cooperative cancel on the current burst (if any).
            let cancel = {
                let s = state_for_worker.lock().ok();
                s.as_ref().and_then(|s| s.active_op.as_ref().map(|(_, c)| c.clone()))
            };
            if cancel.as_ref().map_or(false, |c| c.load(Ordering::Relaxed)) {
                finish_item(&state_for_worker, &ui_tx_for_worker, item.rip_file_id);
                continue;
            }

            let rf_id = item.rip_file_id;
            match phono_junk_lib::scan::identify_one(&phono_ctx, &conn, rf_id, false) {
                Ok(disc) => {
                    log::info!(
                        "identify queue: rip_file={rf_id} → identified={} disc_id={:?}",
                        disc.identified,
                        disc.disc_id,
                    );
                }
                Err(e) => {
                    log::warn!("identify queue: rip_file={rf_id} failed: {e}");
                }
            }
            finish_item(&state_for_worker, &ui_tx_for_worker, rf_id);
        }
    });

    QueueHandle {
        sender: tx,
        state,
        ui_tx,
    }
}

/// Per-item bookkeeping after `identify_one` returns — decrement
/// `in_flight`, send a progress tick, and if the burst just drained
/// complete the activity-bar entry. Next enqueue starts a fresh burst.
fn finish_item(state: &Arc<Mutex<QueueState>>, ui_tx: &Sender<AppMessage>, rf_id: Id) {
    let (op_id_opt, current, total, burst_done) = {
        let Ok(mut s) = state.lock() else {
            return;
        };
        s.in_flight = s.in_flight.saturating_sub(1);
        let op_id = s.active_op.as_ref().map(|(id, _)| *id);
        let current = s.total_seen.saturating_sub(s.in_flight);
        let total = s.total_seen;
        let burst_done = s.in_flight == 0;
        if burst_done {
            s.active_op = None;
            s.total_seen = 0;
        }
        (op_id, current, total, burst_done)
    };

    if let Some(op_id) = op_id_opt {
        let _ = ui_tx.send(AppMessage::OperationProgress {
            op_id,
            current,
            total,
            note: Some(format!("rip_file {rf_id}")),
        });
    }
    let _ = ui_tx.send(AppMessage::LibraryChanged);
    if burst_done {
        if let Some(op_id) = op_id_opt {
            let _ = ui_tx.send(AppMessage::OperationComplete { op_id });
        }
    }
}
