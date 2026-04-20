//! Re-verify selected albums against AccurateRip.
//!
//! Iterates albums → discs → `PhonoContext::verify_disc(DiscId)`. The lib
//! call persists `accuraterip_status` + `last_verified_at` on the linked
//! `RipFile`.

use std::sync::atomic::Ordering;

use phono_junk_catalog::Id;
use phono_junk_db::open_database;
use phono_junk_lib::verify::VerifyTarget;

use crate::app::PhonoApp;
use crate::backend::{resolve_disc_ids_for_albums, worker::spawn_background_op};
use crate::state::AppMessage;

pub fn spawn_reverify(app: &mut PhonoApp, album_ids: Vec<Id>) {
    let Some(db_path) = app.db_path.clone() else {
        app.load_error = Some("re-verify: open a catalog database first".into());
        return;
    };
    let phono_ctx = app.phono_ctx.clone();
    let n = album_ids.len();
    let description = format!("Re-verifying {n} album{}", if n == 1 { "" } else { "s" });

    spawn_background_op(app, description, move |op_id, cancel, tx| {
        let conn = match open_database(&db_path) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(AppMessage::OperationFailed {
                    op_id,
                    error: format!("re-verify: open database: {e}"),
                });
                return;
            }
        };

        let disc_ids = match resolve_disc_ids_for_albums(&conn, &album_ids) {
            Ok(d) => d,
            Err(e) => {
                let _ = tx.send(AppMessage::OperationFailed {
                    op_id,
                    error: format!("re-verify: resolve discs: {e}"),
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

            if let Err(e) = phono_ctx.verify_disc(&conn, VerifyTarget::DiscId(*disc_id)) {
                failures += 1;
                log::warn!("verify disc {disc_id}: {e}");
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
                error: format!("re-verify finished with {failures} failure(s); see log"),
            });
        } else {
            let _ = tx.send(AppMessage::OperationComplete { op_id });
        }
    });
}
