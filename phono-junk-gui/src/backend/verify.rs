//! Queue AccurateRip verification on the session-owned supervisor.

use phono_junk_lib::Id;

use crate::app::PhonoApp;

pub fn spawn_reverify(app: &mut PhonoApp, album_ids: Vec<Id>) {
    let Some(session) = app.session.as_ref() else {
        app.load_error = Some("re-verify: open a catalog database first".into());
        return;
    };
    let disc_ids = match session.disc_ids_for_albums(&album_ids) {
        Ok(ids) => ids,
        Err(error) => {
            app.load_error = Some(format!("re-verify: {error}"));
            return;
        }
    };
    match session.supervisor().queue_verification(disc_ids) {
        Ok(job_id) => app.status_message = Some(format!("job {}: re-verifying", job_id.0)),
        Err(error) => app.load_error = Some(format!("re-verify: {error}")),
    }
}
