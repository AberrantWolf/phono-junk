//! Append-only identification and verification evidence persistence.
//!
//! Storage-shaped records keep this crate independent of provider and
//! AccurateRip crates. `phono-junk-lib` is the one mapping layer.

use rusqlite::{Connection, OptionalExtension, params};

use crate::DbError;

pub type EvidenceId = i64;

#[derive(Debug, Clone)]
pub struct NewDbarResponse<'a> {
    pub disc_stable_key: &'a str,
    pub body_hash: &'a str,
    pub body: &'a [u8],
    pub acquired_at: &'a str,
}

#[derive(Debug, Clone)]
pub struct NewTrackVerification<'a> {
    pub track_position: u8,
    pub computed_v1: u32,
    pub computed_v2: u32,
    pub matched_checksum: Option<u32>,
    pub checksum_version: Option<&'a str>,
    pub sample_shift: Option<i32>,
    pub confidence: Option<u8>,
    pub response_index: Option<usize>,
    pub frame_450_support: bool,
    pub status: &'a str,
}

#[derive(Debug, Clone)]
pub struct NewProviderObservation<'a> {
    pub provider: &'a str,
    pub input_kind: &'a str,
    pub input_value: &'a str,
    pub stage: u8,
    pub raw_response_json: Option<&'a str>,
    pub error_text: Option<&'a str>,
}

pub fn start_identification_attempt(
    conn: &Connection,
    rip_file_id: Option<i64>,
    disc_stable_key: &str,
) -> Result<EvidenceId, DbError> {
    conn.execute(
        "INSERT INTO identification_attempts (rip_file_id, disc_stable_key)
         VALUES (?1, ?2)",
        params![rip_file_id, disc_stable_key],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_provider_observation(
    conn: &Connection,
    attempt_id: EvidenceId,
    observation: &NewProviderObservation<'_>,
) -> Result<EvidenceId, DbError> {
    conn.execute(
        "INSERT INTO provider_observations
            (attempt_id, provider, input_kind, input_value, stage,
             raw_response_json, error_text)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            attempt_id,
            observation.provider,
            observation.input_kind,
            observation.input_value,
            i64::from(observation.stage),
            observation.raw_response_json,
            observation.error_text
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_identification_candidate(
    conn: &Connection,
    attempt_id: EvidenceId,
    candidate_key: &str,
    candidate_json: &str,
    score_json: &str,
    selected: bool,
) -> Result<EvidenceId, DbError> {
    conn.execute(
        "INSERT INTO identification_candidates
            (attempt_id, candidate_key, candidate_json, score_json, selected)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            attempt_id,
            candidate_key,
            candidate_json,
            score_json,
            selected
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn link_candidate_observation(
    conn: &Connection,
    candidate_id: EvidenceId,
    observation_id: EvidenceId,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT OR IGNORE INTO candidate_observations (candidate_id, observation_id)
         VALUES (?1, ?2)",
        params![candidate_id, observation_id],
    )?;
    Ok(())
}

pub fn finish_identification_attempt(
    conn: &Connection,
    attempt_id: EvidenceId,
    status: &str,
    selected_candidate: Option<&str>,
    confidence: Option<&str>,
    ambiguity_json: Option<&str>,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE identification_attempts
         SET finished_at = datetime('now'), status = ?1, selected_candidate = ?2,
             confidence = ?3, ambiguity_json = ?4 WHERE id = ?5",
        params![
            status,
            selected_candidate,
            confidence,
            ambiguity_json,
            attempt_id
        ],
    )?;
    Ok(())
}

pub fn insert_dbar_response(
    conn: &Connection,
    response: &NewDbarResponse<'_>,
) -> Result<EvidenceId, DbError> {
    conn.execute(
        "INSERT INTO dbar_responses (disc_stable_key, body_hash, body, acquired_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(disc_stable_key, body_hash) DO NOTHING",
        params![
            response.disc_stable_key,
            response.body_hash,
            response.body,
            response.acquired_at
        ],
    )?;
    conn.query_row(
        "SELECT id FROM dbar_responses WHERE disc_stable_key = ?1 AND body_hash = ?2",
        params![response.disc_stable_key, response.body_hash],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn start_verification_run(
    conn: &Connection,
    rip_file_id: i64,
    dbar_response_id: Option<EvidenceId>,
    max_sample_shift: i32,
    ripper_read_offset: Option<i32>,
) -> Result<EvidenceId, DbError> {
    conn.execute(
        "INSERT INTO verification_runs
            (rip_file_id, dbar_response_id, max_sample_shift, ripper_read_offset, status)
         VALUES (?1, ?2, ?3, ?4, 'working')",
        params![
            rip_file_id,
            dbar_response_id,
            max_sample_shift,
            ripper_read_offset
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn finish_verification_run(
    conn: &Connection,
    run_id: EvidenceId,
    status: &str,
    chosen_sample_shift: Option<i32>,
    ambiguous_offsets_json: Option<&str>,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE verification_runs
         SET finished_at = datetime('now'), status = ?1,
             chosen_sample_shift = ?2, ambiguous_offsets_json = ?3
         WHERE id = ?4",
        params![status, chosen_sample_shift, ambiguous_offsets_json, run_id],
    )?;
    Ok(())
}

pub fn insert_track_verification(
    conn: &Connection,
    run_id: EvidenceId,
    track: &NewTrackVerification<'_>,
) -> Result<(), DbError> {
    conn.execute(
        "INSERT INTO track_verifications
            (run_id, track_position, computed_v1, computed_v2, matched_checksum,
             checksum_version, sample_shift, confidence, response_index,
             frame_450_support, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            run_id,
            i64::from(track.track_position),
            i64::from(track.computed_v1),
            i64::from(track.computed_v2),
            track.matched_checksum.map(i64::from),
            track.checksum_version,
            track.sample_shift,
            track.confidence.map(i64::from),
            track.response_index.map(|value| value as i64),
            track.frame_450_support,
            track.status
        ],
    )?;
    Ok(())
}

pub fn latest_verification_status(
    conn: &Connection,
    rip_file_id: i64,
) -> Result<Option<String>, DbError> {
    conn.query_row(
        "SELECT status FROM verification_runs
         WHERE rip_file_id = ?1 AND finished_at IS NOT NULL
         ORDER BY id DESC LIMIT 1",
        [rip_file_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Recover process-local work markers after an unclean shutdown.
pub fn recover_interrupted_work(conn: &Connection) -> Result<(), DbError> {
    conn.execute(
        "UPDATE rip_files SET identification_state = 'queued',
             last_state_change_at = datetime('now')
         WHERE identification_state = 'working'",
        [],
    )?;
    conn.execute(
        "UPDATE identification_attempts SET status = 'interrupted',
             finished_at = datetime('now') WHERE status = 'working'",
        [],
    )?;
    conn.execute(
        "UPDATE verification_runs SET status = 'interrupted',
             finished_at = datetime('now') WHERE status = 'working'",
        [],
    )?;
    Ok(())
}
