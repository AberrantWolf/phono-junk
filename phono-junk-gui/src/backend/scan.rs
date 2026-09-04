//! Queue a tracked-folder scan on the session-owned supervisor.

use std::path::PathBuf;

use phono_junk_lib::{IdentificationDisposition, RefreshPolicy, ScanRequest};

use crate::app::PhonoApp;

pub fn spawn_scan(app: &mut PhonoApp, root: PathBuf) {
    let Some(session) = app.session.as_ref() else {
        app.load_error = Some("scan: open a catalog database first".into());
        return;
    };
    if let Err(error) = session.track_folder(&root) {
        app.load_error = Some(format!("scan: track {}: {error}", root.display()));
        return;
    }
    let request = ScanRequest {
        refresh: RefreshPolicy::UseCache,
        identification: IdentificationDisposition::Queue,
    };
    match session.supervisor().queue_scan(root.clone(), request) {
        Ok(job_id) => {
            app.status_message = Some(format!("job {}: scanning {}", job_id.0, root.display()));
        }
        Err(error) => app.load_error = Some(format!("scan: {error}")),
    }
}
