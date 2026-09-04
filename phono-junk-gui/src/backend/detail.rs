//! Queue cover-art acquisition on the session-owned supervisor.

use std::path::PathBuf;

use phono_junk_lib::Asset;

use crate::app::PhonoApp;

pub fn queue_cover_fetch(app: &mut PhonoApp, asset: Asset, cache_dir: PathBuf) {
    let Some(session) = app.session.as_ref() else {
        app.load_error = Some("cover art: no database open".into());
        return;
    };
    if let Some(cache) = app.detail_cache.as_mut() {
        cache.art_loading = true;
    }
    if let Err(error) = session.supervisor().queue_asset_cache(asset, cache_dir)
        && let Some(cache) = app.detail_cache.as_mut()
    {
        cache.art_loading = false;
        cache.art_error = Some(error.to_string());
    }
}
