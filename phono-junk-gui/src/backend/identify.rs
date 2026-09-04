//! Queue identification on the session-owned supervisor.

use phono_junk_lib::Id;

use crate::app::PhonoApp;

pub fn spawn_reidentify(app: &mut PhonoApp, album_ids: Vec<Id>) {
    let Some(session) = app.session.as_ref() else {
        app.load_error = Some("re-identify: open a catalog database first".into());
        return;
    };
    let rip_file_ids = match session.rip_ids_for_albums(&album_ids) {
        Ok(ids) => ids,
        Err(error) => {
            app.load_error = Some(format!("re-identify: {error}"));
            return;
        }
    };
    match session
        .supervisor()
        .queue_identification(rip_file_ids, true)
    {
        Ok(job_id) => app.status_message = Some(format!("job {}: re-identifying", job_id.0)),
        Err(error) => app.load_error = Some(format!("re-identify: {error}")),
    }
}

pub fn spawn_identify_unidentified(app: &mut PhonoApp, rip_file_ids: Vec<Id>) {
    let Some(session) = app.session.as_ref() else {
        app.load_error = Some("identify: open a catalog database first".into());
        return;
    };
    if rip_file_ids.is_empty() {
        return;
    }
    match session
        .supervisor()
        .queue_identification(rip_file_ids, false)
    {
        Ok(job_id) => app.status_message = Some(format!("job {}: identifying", job_id.0)),
        Err(error) => app.load_error = Some(format!("identify: {error}")),
    }
}
