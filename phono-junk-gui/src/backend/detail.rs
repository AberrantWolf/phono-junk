//! Detail-panel side workers — currently just lazy cover-art fetch.
//!
//! Distinct from the album-list bulk workers: these one-shot tasks don't
//! register on the activity bar (a thumbnail load isn't user-visible work
//! the way an export is) and have no cancel token (worst case we drop the
//! result on `DetailArtLoaded`/`DetailArtFailed` if focus has moved).

use std::sync::mpsc;
use std::thread;

use phono_junk_catalog::Asset;
use phono_junk_lib::{PhonoContext, fetch_asset_bytes};

use crate::state::{AppMessage, EntryKey};

/// Spawn a one-shot fetch for `asset`'s bytes. On success posts
/// `AppMessage::DetailArtLoaded { key, bytes }`; on failure posts
/// `AppMessage::DetailArtFailed { key, error }` so the detail panel can
/// render the reason inline instead of egui's anonymous broken-image
/// placeholder.
///
/// Caller is responsible for setting `detail_cache.art_loading = true`
/// before calling so repaints don't stack duplicate fetches; this fn
/// itself can't touch app state (runs on a worker thread).
pub fn spawn_cover_fetch(
    ctx: std::sync::Arc<PhonoContext>,
    tx: mpsc::Sender<AppMessage>,
    key: EntryKey,
    asset: Asset,
) {
    thread::spawn(move || match fetch_asset_bytes(&ctx, &asset) {
        Ok(bytes) => {
            let _ = tx.send(AppMessage::DetailArtLoaded { key, bytes });
        }
        Err(e) => {
            let error = e.to_string();
            log::warn!(
                "detail: fetch cover bytes for asset {} failed: {error}",
                asset.id,
            );
            let _ = tx.send(AppMessage::DetailArtFailed { key, error });
        }
    });
}
