//! Background workers — disc scan, identification, verification, export.
//!
//! Every long-running operation dispatched from the UI lives here. Each
//! submodule exposes one `spawn_*` function matching its op shape; all
//! four share the same worker contract:
//!
//! 1. Re-open the catalog DB in the worker thread (rusqlite `Connection`
//!    is `!Send`; WAL mode permits concurrent readers).
//! 2. Hold an `Arc<PhonoContext>` so provider rate limiters coordinate
//!    across workers.
//! 3. Loop over work items, checking `cancel.load(Ordering::Relaxed)`
//!    between items (never inside lib calls).
//! 4. Send `AppMessage::OperationProgress` per item; conclude with
//!    `LibraryChanged` + `OperationComplete`, or `OperationFailed`.

pub mod detail;
pub mod export;
pub mod identify;
pub mod scan;
pub mod verify;
pub mod worker;

use phono_junk_catalog::Id;
use phono_junk_db::{DbError, crud};
use rusqlite::Connection;

/// Resolve a set of album ids to the concrete disc ids that live under
/// them (via releases). Order preserved per album; deduped across
/// releases sharing a disc should not happen (disc belongs to one
/// release) but the helper tolerates empty releases gracefully.
pub(crate) fn resolve_disc_ids_for_albums(
    conn: &Connection,
    album_ids: &[Id],
) -> Result<Vec<Id>, DbError> {
    let mut out = Vec::new();
    for &album_id in album_ids {
        for release in crud::list_releases_for_album(conn, album_id)? {
            for disc in crud::list_discs_for_release(conn, release.id)? {
                out.push(disc.id);
            }
        }
    }
    Ok(out)
}
