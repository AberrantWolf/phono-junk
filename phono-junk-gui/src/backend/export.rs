//! Queue FLAC export on the session-owned supervisor.

use std::path::PathBuf;

use phono_junk_lib::Id;

use crate::app::PhonoApp;

pub fn spawn_export(app: &mut PhonoApp, album_ids: Vec<Id>, library_root: PathBuf) {
    let Some(session) = app.session.as_ref() else {
        app.load_error = Some("export: open a catalog database first".into());
        return;
    };
    let disc_ids = match session.disc_ids_for_albums(&album_ids) {
        Ok(ids) => ids,
        Err(error) => {
            app.load_error = Some(format!("export: {error}"));
            return;
        }
    };
    match session
        .supervisor()
        .queue_export(disc_ids, library_root.clone())
    {
        Ok(job_id) => {
            app.status_message = Some(format!(
                "job {}: exporting to {}",
                job_id.0,
                library_root.display()
            ));
        }
        Err(error) => app.load_error = Some(format!("export: {error}")),
    }
}
