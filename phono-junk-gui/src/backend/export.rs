//! Export selected albums to FLAC under a user-picked library root.
//!
//! Iterates albums → discs → `PhonoContext::export_disc`. The note
//! attached to each progress tick is the album's title when available,
//! falling back to the disc id.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use phono_junk_catalog::Id;
use phono_junk_db::{crud, open_database};

use crate::app::PhonoApp;
use crate::backend::{resolve_disc_ids_for_albums, worker::spawn_background_op};
use crate::state::AppMessage;

pub fn spawn_export(app: &mut PhonoApp, album_ids: Vec<Id>, library_root: PathBuf) {
    let Some(db_path) = app.db_path.clone() else {
        app.load_error = Some("export: open a catalog database first".into());
        return;
    };
    let phono_ctx = app.phono_ctx.clone();
    let n = album_ids.len();
    let description = format!(
        "Exporting {n} album{} → {}",
        if n == 1 { "" } else { "s" },
        library_root.display()
    );

    spawn_background_op(app, description, move |op_id, cancel, tx| {
        let conn = match open_database(&db_path) {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(AppMessage::OperationFailed {
                    op_id,
                    error: format!("export: open database: {e}"),
                });
                return;
            }
        };

        let disc_ids = match resolve_disc_ids_for_albums(&conn, &album_ids) {
            Ok(d) => d,
            Err(e) => {
                let _ = tx.send(AppMessage::OperationFailed {
                    op_id,
                    error: format!("export: resolve discs: {e}"),
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
            let note = disc_label(&conn, *disc_id).unwrap_or_else(|| format!("disc {disc_id}"));
            let _ = tx.send(AppMessage::OperationProgress {
                op_id,
                current: i as u64,
                total,
                note: Some(note),
            });

            if let Err(e) = phono_ctx.export_disc(&conn, *disc_id, &library_root) {
                failures += 1;
                log::warn!("export disc {disc_id}: {e}");
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
                error: format!("export finished with {failures} failure(s); see log"),
            });
        } else {
            let _ = tx.send(AppMessage::OperationComplete { op_id });
        }
    });
}

fn disc_label(conn: &rusqlite::Connection, disc_id: Id) -> Option<String> {
    let disc = crud::get_disc(conn, disc_id).ok().flatten()?;
    let release = crud::get_release(conn, disc.release_id).ok().flatten()?;
    let album = crud::get_album(conn, release.album_id).ok().flatten()?;
    match (album.artist_credit.as_deref(), album.title.as_str()) {
        (Some(a), t) if !t.is_empty() => Some(format!("{a} — {t}")),
        (None, t) if !t.is_empty() => Some(t.to_string()),
        _ => None,
    }
}
