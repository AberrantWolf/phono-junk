use phono_junk_catalog::RipFile;
use phono_junk_core::{IdentificationConfidence, IdentificationState};
use phono_junk_db::{
    crud,
    evidence::{
        NewDbarResponse, NewTrackVerification, finish_verification_run, insert_dbar_response,
        insert_track_verification, latest_verification_status, recover_interrupted_work,
        start_verification_run,
    },
    open_memory,
};

fn rip_file() -> RipFile {
    RipFile {
        id: 0,
        disc_id: None,
        cue_path: None,
        chd_path: None,
        bin_paths: Vec::new(),
        mtime: None,
        size: None,
        identification_confidence: IdentificationConfidence::Unidentified,
        identification_source: None,
        accuraterip_status: None,
        last_verified_at: None,
        inferred_sample_shift: None,
        last_identify_errors: None,
        last_identify_at: None,
        provenance: None,
        identification_state: IdentificationState::Working,
        last_state_change_at: None,
    }
}

#[test]
fn verification_evidence_is_append_only_and_drives_latest_status() {
    let conn = open_memory().unwrap();
    let rip_id = crud::insert_rip_file(&conn, &rip_file()).unwrap();
    let dbar_id = insert_dbar_response(
        &conn,
        &NewDbarResponse {
            disc_stable_key: "disc:ar:1-2-3",
            body_hash: "abc",
            body: &[1, 2, 3],
            acquired_at: "2026-09-03T00:00:00Z",
        },
    )
    .unwrap();
    let duplicate_id = insert_dbar_response(
        &conn,
        &NewDbarResponse {
            disc_stable_key: "disc:ar:1-2-3",
            body_hash: "abc",
            body: &[1, 2, 3],
            acquired_at: "2026-09-03T00:00:00Z",
        },
    )
    .unwrap();
    assert_eq!(duplicate_id, dbar_id);

    let run_id = start_verification_run(&conn, rip_id, Some(dbar_id), 2939, Some(6)).unwrap();
    insert_track_verification(
        &conn,
        run_id,
        &NewTrackVerification {
            track_position: 1,
            computed_v1: 10,
            computed_v2: 20,
            matched_checksum: Some(20),
            checksum_version: Some("v2"),
            sample_shift: Some(-6),
            confidence: Some(8),
            response_index: Some(2),
            frame_450_support: true,
            status: "verified",
        },
    )
    .unwrap();
    finish_verification_run(&conn, run_id, "verified", Some(-6), None).unwrap();

    assert_eq!(
        latest_verification_status(&conn, rip_id)
            .unwrap()
            .as_deref(),
        Some("verified")
    );
    let loaded = crud::get_rip_file(&conn, rip_id).unwrap().unwrap();
    assert_eq!(loaded.accuraterip_status.as_deref(), Some("verified"));
    assert!(loaded.last_verified_at.is_some());
    assert_eq!(loaded.inferred_sample_shift, Some(-6));
}

#[test]
fn recovery_requeues_identification_and_interrupts_runs() {
    let conn = open_memory().unwrap();
    let rip_id = crud::insert_rip_file(&conn, &rip_file()).unwrap();
    conn.execute(
        "INSERT INTO identification_attempts (rip_file_id, disc_stable_key)
         VALUES (?1, 'disc:local:test')",
        [rip_id],
    )
    .unwrap();
    let run_id = start_verification_run(&conn, rip_id, None, 2939, None).unwrap();

    recover_interrupted_work(&conn).unwrap();

    let loaded = crud::get_rip_file(&conn, rip_id).unwrap().unwrap();
    assert_eq!(loaded.identification_state, IdentificationState::Queued);
    let attempt_status: String = conn
        .query_row("SELECT status FROM identification_attempts", [], |row| {
            row.get(0)
        })
        .unwrap();
    let run_status: String = conn
        .query_row(
            "SELECT status FROM verification_runs WHERE id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(attempt_status, "interrupted");
    assert_eq!(run_status, "interrupted");
}
