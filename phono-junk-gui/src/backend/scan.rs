//! Scan-a-folder background op.
//!
//! Walks `root` via `PhonoContext::scan_library`. The progress callback
//! translates each `ScanEvent<'_>` into an owned `note: Option<String>`
//! before sending it across the channel — sidesteps the
//! `&dyn std::error::Error` cross-thread constraint (TODO.md line 107)
//! without changing the lib's borrow-based progress enum.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use phono_junk_db::open_database;
use phono_junk_lib::scan::{ScanEvent, ScanOpts};

use crate::app::PhonoApp;
use crate::backend::worker::spawn_background_op;
use crate::state::AppMessage;

pub fn spawn_scan(app: &mut PhonoApp, root: PathBuf) {
    let Some(db_path) = app.db_path.clone() else {
        app.load_error = Some("scan: open a catalog database first".into());
        return;
    };
    let phono_ctx = app.phono_ctx.clone();
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

        let opts = ScanOpts::default();
        let mut seen: u64 = 0;
        let cancel_for_cb = cancel.clone();
        let tx_for_cb = tx.clone();
        let result = phono_ctx.scan_library(&conn, &root, opts, |event| {
            if cancel_for_cb.load(Ordering::Relaxed) {
                return;
            }
            // Count only terminal events (one per file) so the UI
            // counter matches "files processed", not raw event volume.
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
                let status = format!(
                    "scan: {} file(s) — {} identified, {} unidentified, {} cached, {} failed",
                    summary.total_files,
                    summary.identified,
                    summary.unidentified,
                    summary.cached,
                    summary.failed,
                );
                log::warn!("{status}");
                let _ = tx.send(AppMessage::Status(status));
                let _ = tx.send(AppMessage::OperationComplete { op_id });
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
        | ScanEvent::Identified { path, .. }
        | ScanEvent::ScannedOnly { path, .. }
        | ScanEvent::Failed { path, .. } => path,
    };
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .or_else(|| Some(path.display().to_string()))
}
