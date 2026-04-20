//! Scan-a-folder background op — metadata-only walk plus identify queue
//! dispatch.
//!
//! The scan worker runs [`PhonoContext::scan_library`] with
//! `identify = false` so every file's metadata lands fast and the album
//! list reflects the new rows as soon as they exist. Each `Ingested`
//! event produces a `LibraryChanged` so the table re-renders per row,
//! and the ids are pushed onto [`identify_queue`] for serialized
//! provider fan-out afterwards.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use phono_junk_core::IdentificationState;
use phono_junk_db::{crud, open_database};
use phono_junk_lib::scan::{ScanEvent, ScanOpts};

use crate::app::PhonoApp;
use crate::backend::identify_queue::enqueue_for_identify;
use crate::backend::worker::spawn_background_op;
use crate::state::AppMessage;

pub fn spawn_scan(app: &mut PhonoApp, root: PathBuf) {
    let Some(db_path) = app.db_path.clone() else {
        app.load_error = Some("scan: open a catalog database first".into());
        return;
    };

    // Register the folder as a tracked library root so subsequent app
    // opens auto-rescan it. Idempotent — re-adding a known path is a
    // no-op, so repeated "Add folder…" clicks on the same tree are safe.
    if let Some(conn) = app.db_conn.as_ref() {
        if let Err(e) = crud::insert_library_folder(conn, &root) {
            log::warn!("scan: failed to persist library folder {}: {e}", root.display());
        }
    }

    let phono_ctx = app.phono_ctx.clone();
    let app_tx = app.message_tx.clone();
    let description = format!("Scanning {}", root.display());

    spawn_background_op(app, description, move |op_id, cancel, tx| {
        let conn = match open_database(&db_path) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(AppMessage::OperationFailed {
                    op_id,
                    error: format!("scan: open database: {e}"),
                });
                return;
            }
        };

        // Metadata-only walk. Identification runs in a second pass
        // *after* the walk completes so the table has fully populated
        // before the slow (rate-limited) identify work kicks off —
        // matches the user's mental model of "finish the fast parts,
        // then automatically start the full scan of detected albums."
        let opts = ScanOpts {
            force_refresh: false,
            identify: false,
        };
        let mut queued_ids: Vec<i64> = Vec::new();
        let mut seen: u64 = 0;
        let cancel_for_cb = cancel.clone();
        let tx_for_cb = tx.clone();
        let result = phono_ctx.scan_library(&conn, &root, opts, |event| {
            if cancel_for_cb.load(Ordering::Relaxed) {
                return;
            }
            if matches!(event, ScanEvent::Found { .. }) {
                let note = describe_event(&event);
                let _ = tx_for_cb.send(AppMessage::OperationProgress {
                    op_id,
                    current: seen,
                    total: 0,
                    note,
                });
                return;
            }
            log_event(&event);

            // Streaming LibraryChanged per non-Found event so the album
            // list updates as rows land rather than only at the end.
            match &event {
                ScanEvent::Ingested {
                    rip_file_id,
                    state,
                    ..
                } => {
                    let _ = tx_for_cb.send(AppMessage::LibraryChanged);
                    if *state == IdentificationState::Queued {
                        queued_ids.push(*rip_file_id);
                    }
                }
                ScanEvent::CacheHit { .. }
                | ScanEvent::ScannedOnly { .. }
                | ScanEvent::Identified { .. }
                | ScanEvent::Failed { .. } => {
                    let _ = tx_for_cb.send(AppMessage::LibraryChanged);
                }
                ScanEvent::Found { .. } => {}
            }

            seen += 1;
            let note = describe_event(&event);
            let _ = tx_for_cb.send(AppMessage::OperationProgress {
                op_id,
                current: seen,
                total: 0,
                note,
            });
        });

        let _ = tx.send(AppMessage::LibraryChanged);
        match result {
            Ok(summary) => {
                let queued_count = queued_ids.len();
                let status = format!(
                    "scan: {} file(s) — {} cached, {} queued for identify, {} failed, {} sidecars refreshed",
                    summary.total_files,
                    summary.cached,
                    queued_count,
                    summary.failed,
                    summary.sidecars_refreshed,
                );
                log::warn!("{status}");
                let _ = tx.send(AppMessage::Status(status));
                let _ = tx.send(AppMessage::OperationComplete { op_id });

                // Metadata phase done, table is fully populated — kick
                // off the identify queue with every freshly-queued row
                // in insert order so the Status column animates
                // top-down as each provider call finishes.
                for rf_id in queued_ids {
                    enqueue_for_identify(
                        app_tx.clone(),
                        phono_ctx.clone(),
                        db_path.clone(),
                        rf_id,
                    );
                }
            }
            Err(e) => {
                let _ = tx.send(AppMessage::OperationFailed {
                    op_id,
                    error: format!("scan failed: {e}"),
                });
            }
        }
    });
}

fn log_event(event: &ScanEvent<'_>) {
    match event {
        ScanEvent::Found { path, .. } => {
            log::debug!("scan: found {}", path.display());
        }
        ScanEvent::CacheHit { path, rip_file_id } => {
            log::debug!("scan: cache hit {} (rip_file_id={rip_file_id})", path.display());
        }
        ScanEvent::Ingested {
            path,
            rip_file_id,
            state,
        } => {
            log::info!(
                "scan: metadata {} (rip_file_id={rip_file_id}, state={})",
                path.display(),
                state.as_str(),
            );
        }
        ScanEvent::ScannedOnly { path, rip_file_id } => {
            log::info!(
                "scan: scanned-only {} (rip_file_id={rip_file_id})",
                path.display()
            );
        }
        ScanEvent::Identified { path, result } => {
            if result.identified {
                log::info!(
                    "scan: identified {} → album_id={:?} disc_id={:?} asset_count={} disagreements={}",
                    path.display(),
                    result.album_id,
                    result.disc_id,
                    result.asset_count,
                    result.any_disagreements,
                );
            } else {
                log::warn!(
                    "scan: unidentified {} — no provider returned a match ({} provider error(s))",
                    path.display(),
                    result.provider_errors.len(),
                );
                for (name, err) in &result.provider_errors {
                    log::warn!("  provider {name}: {err}");
                }
            }
        }
        ScanEvent::Failed { path, error } => {
            log::warn!("scan: failed {} — {error}", path.display());
        }
    }
}

fn describe_event(event: &ScanEvent<'_>) -> Option<String> {
    let path = match event {
        ScanEvent::Found { path, .. }
        | ScanEvent::CacheHit { path, .. }
        | ScanEvent::Ingested { path, .. }
        | ScanEvent::Identified { path, .. }
        | ScanEvent::ScannedOnly { path, .. }
        | ScanEvent::Failed { path, .. } => path,
    };
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .or_else(|| Some(path.display().to_string()))
}
