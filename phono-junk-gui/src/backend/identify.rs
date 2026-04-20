//! Re-identify selected albums, and run first-time identify on selected
//! unidentified rip files.
//!
//! `spawn_reidentify` walks `album → release → disc → rip_file`, rebuilds
//! the disc's [`Toc`] + [`DiscIds`] from the persisted `Disc` row, and
//! calls `PhonoContext::identify_disc(..., force_refresh=true)` so
//! providers re-run and overwrite stale metadata.
//!
//! `spawn_identify_unidentified` loops over selected rip-file ids and
//! re-runs [`phono_junk_lib::scan::ingest_path`] on each — the same fast/
//! slow path the scan-folder flow uses, so identify-from-unidentified and
//! scan-folder stay bit-identical.

use std::sync::atomic::Ordering;

use phono_junk_catalog::Id;
use phono_junk_core::DiscIds;
use phono_junk_db::{crud, open_database};

use crate::app::PhonoApp;
use crate::backend::{resolve_disc_ids_for_albums, worker::spawn_background_op};
use crate::state::AppMessage;

pub fn spawn_reidentify(app: &mut PhonoApp, album_ids: Vec<Id>) {
    let Some(db_path) = app.db_path.clone() else {
        app.load_error = Some("re-identify: open a catalog database first".into());
        return;
    };
    let phono_ctx = app.phono_ctx.clone();
    let n = album_ids.len();
    let description = format!("Re-identifying {n} album{}", if n == 1 { "" } else { "s" });

    spawn_background_op(app, description, move |op_id, cancel, tx| {
        let conn = match open_database(&db_path) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(AppMessage::OperationFailed {
                    op_id,
                    error: format!("re-identify: open database: {e}"),
                });
                return;
            }
        };

        let disc_ids = match resolve_disc_ids_for_albums(&conn, &album_ids) {
            Ok(d) => d,
            Err(e) => {
                let _ = tx.send(AppMessage::OperationFailed {
                    op_id,
                    error: format!("re-identify: resolve discs: {e}"),
                });
                return;
            }
        };

        let total = disc_ids.len() as u64;
        let mut failures = 0usize;
        for (i, disc_id) in disc_ids.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let _ = tx.send(AppMessage::OperationProgress {
                op_id,
                current: i as u64,
                total,
                note: Some(format!("disc {disc_id}")),
            });

            if let Err(e) = reidentify_one(&conn, &phono_ctx, *disc_id) {
                failures += 1;
                log::warn!("re-identify disc {disc_id}: {e}");
            }
        }
        let _ = tx.send(AppMessage::OperationProgress {
            op_id,
            current: total,
            total,
            note: None,
        });

        let _ = tx.send(AppMessage::LibraryChanged);
        if failures > 0 {
            let _ = tx.send(AppMessage::OperationFailed {
                op_id,
                error: format!("re-identify finished with {failures} failure(s); see log"),
            });
        } else {
            let _ = tx.send(AppMessage::OperationComplete { op_id });
        }
    });
}

fn reidentify_one(
    conn: &rusqlite::Connection,
    ctx: &phono_junk_lib::PhonoContext,
    disc_id: Id,
) -> Result<(), Box<dyn std::error::Error>> {
    let disc = crud::get_disc(conn, disc_id)?.ok_or("disc vanished")?;
    let toc = disc.toc.clone().ok_or("disc has no persisted TOC")?;
    // Carry barcode + catalog_number into the re-identify pass so Discogs
    // (and any other identifier-keyed provider) actually gets dispatched.
    // MCN on `Disc` was populated by Sprint 22's sidecar enrichment; the
    // catalog_number lives on the release. Previously these were hardcoded
    // to `None` and every re-identify silently skipped Discogs.
    let barcode = disc.mcn.clone();
    let catalog_number = crud::get_release(conn, disc.release_id)?
        .and_then(|r| r.catalog_number);
    let ids = DiscIds {
        mb_discid: disc.mb_discid.clone(),
        cddb_id: disc.cddb_id.clone(),
        ar_discid1: disc.ar_discid1.clone(),
        ar_discid2: disc.ar_discid2.clone(),
        barcode,
        catalog_number,
    };
    let rip = crud::find_rip_file_for_disc(conn, disc_id)?;
    let rip_id = rip.map(|r| r.id);
    ctx.identify_disc(conn, &toc, &ids, rip_id, true)?;
    Ok(())
}

/// Push every selected rip-file id onto the shared identify queue so the
/// serialized worker drains them one at a time — respects provider rate
/// limits and keeps the "Identify (N)" button consistent with the scan
/// dispatcher's auto-queueing path.
pub fn spawn_identify_unidentified(app: &mut PhonoApp, rip_file_ids: Vec<Id>) {
    let Some(db_path) = app.db_path.clone() else {
        app.load_error = Some("identify: open a catalog database first".into());
        return;
    };
    if rip_file_ids.is_empty() {
        return;
    }
    let phono_ctx = app.phono_ctx.clone();
    for id in rip_file_ids {
        crate::backend::identify_queue::enqueue_for_identify(
            app.message_tx.clone(),
            phono_ctx.clone(),
            db_path.clone(),
            id,
        );
    }
    // No explicit activity-bar registration — the queue announces itself
    // via `AppMessage::OperationStarted` on its first idle→active
    // transition, and `handle_message` adds the entry.
}
