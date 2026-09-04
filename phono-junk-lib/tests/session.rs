use std::sync::Arc;
use std::time::{Duration, Instant};

use phono_junk_lib::{JobEvent, JobEventKind, LibrarySession, PhonoContext};

fn await_events(session: &LibrarySession) -> Vec<JobEvent> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut events = Vec::new();
    while Instant::now() < deadline {
        events.extend(session.supervisor().try_events());
        if events
            .iter()
            .any(|event| matches!(event.kind, JobEventKind::Finished))
        {
            break;
        }
        std::thread::yield_now();
    }
    events
}

#[test]
fn every_job_event_is_scoped_to_its_session_generation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.db");
    let session = LibrarySession::open(path, Arc::new(PhonoContext::new())).unwrap();
    let generation = session.generation();
    let job_id = session
        .supervisor()
        .queue_identification(Vec::new(), false)
        .unwrap();
    let events = await_events(&session);
    assert!(!events.is_empty());
    assert!(
        events
            .iter()
            .all(|event| { event.session_generation == generation && event.job_id == job_id })
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, JobEventKind::LibraryChanged))
    );
}

#[test]
fn reset_joins_old_supervisor_and_increments_generation() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.db");
    let mut session = LibrarySession::open(path, Arc::new(PhonoContext::new())).unwrap();
    let before = session.generation();
    session
        .connection()
        .execute("INSERT INTO albums (title) VALUES ('temporary')", [])
        .unwrap();

    session.reset_database().unwrap();

    assert_ne!(session.generation(), before);
    let count: i64 = session
        .connection()
        .query_row("SELECT COUNT(*) FROM albums", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn shutdown_is_joined_and_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.db");
    let mut session = LibrarySession::open(path, Arc::new(PhonoContext::new())).unwrap();
    session
        .supervisor()
        .queue_identification(Vec::new(), false)
        .unwrap();
    // Reset necessarily cancels, closes, joins, and replaces the supervisor.
    session.reset_database().unwrap();
    session.reset_database().unwrap();
}
