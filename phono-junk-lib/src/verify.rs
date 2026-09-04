//! Verify pipeline: [`PhonoContext::verify_disc`].
//!
//! Computes per-track AccurateRip CRCs from the rip's BIN/CHD, fetches
//! the matching dBAR file, compares, and persists an append-only evidence
//! run. Identification is *not* re-run — see CLAUDE.md's
//! identification-vs-verification split.

use std::path::PathBuf;

use chrono::Utc;
use junk_libs_disc::{TrackKind, TrackLayout, TrackPcmReader};
use phono_junk_accuraterip::{
    AccurateRipError, ChecksumVersion, DiscTrackSamples, TrackPosition, TrackVerification,
    TrackVerificationStatus, VerificationOptions, VerificationStatus, track_crc_samples,
    verify_with_offsets,
};
use phono_junk_catalog::{Disc, Id, RipFile};
use phono_junk_core::DiscIds;
use phono_junk_db::{
    DbError, crud,
    evidence::{self, NewDbarResponse, NewTrackVerification},
};
use phono_junk_identify::HttpError;
use rusqlite::Connection;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::PhonoContext;

/// User-selectable entry point.
#[derive(Debug, Clone)]
pub enum VerifyTarget {
    Path(PathBuf),
    DiscId(Id),
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifiedTrack {
    pub position: u8,
    pub status: String,
    pub v1: u32,
    pub v2: u32,
    pub best_confidence: Option<u8>,
    pub verified: bool,
}

impl From<&TrackVerification> for VerifiedTrack {
    fn from(t: &TrackVerification) -> Self {
        Self {
            position: t.position,
            status: t.status_string(),
            v1: t.computed.v1,
            v2: t.computed.v2,
            best_confidence: t.best_confidence(),
            verified: t.is_verified(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifySummary {
    pub disc_id: Id,
    pub rip_file_id: Id,
    pub per_track: Vec<VerifiedTrack>,
    pub accurate: usize,
    pub mismatched: usize,
    pub not_in_db: bool,
    pub max_confidence: u8,
    pub inferred_sample_shift: Option<i32>,
    pub ripper_read_offset: Option<i32>,
    pub ambiguous_offsets: Vec<i32>,
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("no AccurateRip client registered on PhonoContext")]
    NoAccurateRipClient,
    #[error("disc {0} not found in catalog")]
    MissingDisc(Id),
    #[error("disc {0} has no linked rip_files row")]
    MissingRipFile(Id),
    #[error("rip file not found for path: {0}")]
    NoRipForPath(PathBuf),
    #[error("rip file for path {0} is not linked to any disc yet — run `identify` first")]
    RipNotIdentified(PathBuf),
    #[error("rip file has neither cue_path nor chd_path: rip_file {0}")]
    NoRipSource(Id),
    #[error("disc {0} is missing required AccurateRip IDs")]
    MissingDiscIds(Id),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Analysis(#[from] junk_libs_core::AnalysisError),
    #[error(transparent)]
    Audio(#[from] phono_junk_core::AudioError),
    #[error(transparent)]
    AccurateRip(#[from] AccurateRipError),
    #[error(transparent)]
    Http(#[from] HttpError),
}

impl PhonoContext {
    /// Verify one disc against AccurateRip. Persists structured run and
    /// per-track evidence; display summaries are derived on read.
    ///
    /// Returns a [`VerifySummary`] whose `not_in_db == true` case is *not*
    /// an error — it's a legitimate "we looked, AccurateRip has no
    /// submissions for this TOC triple" outcome.
    pub fn verify_disc(
        &self,
        conn: &Connection,
        target: VerifyTarget,
    ) -> Result<VerifySummary, VerifyError> {
        self.verify_disc_with_options(conn, target, VerificationOptions::default())
    }

    pub fn verify_disc_with_options(
        &self,
        conn: &Connection,
        target: VerifyTarget,
        options: VerificationOptions,
    ) -> Result<VerifySummary, VerifyError> {
        let client = self
            .accuraterip
            .as_ref()
            .ok_or(VerifyError::NoAccurateRipClient)?;
        let (disc, rip_file) = resolve_target(conn, target)?;
        let ids = ids_from_disc(&disc)?;

        let layouts = load_layouts(&rip_file)?;
        let audio_layouts: Vec<&TrackLayout> = layouts
            .iter()
            .filter(|l| matches!(l.kind, TrackKind::Audio | TrackKind::Unknown))
            .collect();
        if audio_layouts.is_empty() {
            return Err(VerifyError::NoRipSource(rip_file.id));
        }
        let track_count = audio_layouts.len() as u8;
        let pcm = load_disc_pcm(&rip_file, &audio_layouts)?;

        // Network activity is complete before any evidence write begins.
        let fetched = client.fetch_dbar_evidence(&ids, track_count)?;
        let ripper_read_offset = rip_file
            .provenance
            .as_ref()
            .and_then(|provenance| provenance.read_offset);
        let disc_stable_key = crud::catalog_entity_key(conn, "disc", disc.id)?;
        let Some(fetched) = fetched else {
            let run_id = evidence::start_verification_run(
                conn,
                rip_file.id,
                None,
                options.max_sample_shift,
                ripper_read_offset,
            )?;
            let tracks = no_data_tracks(&pcm);
            for track in &tracks {
                persist_track_verification(conn, run_id, track)?;
            }
            evidence::finish_verification_run(conn, run_id, "no_data", None, None)?;
            return Ok(VerifySummary {
                disc_id: disc.id,
                rip_file_id: rip_file.id,
                per_track: tracks.iter().map(VerifiedTrack::from).collect(),
                accurate: 0,
                mismatched: 0,
                not_in_db: true,
                max_confidence: 0,
                inferred_sample_shift: None,
                ripper_read_offset,
                ambiguous_offsets: Vec::new(),
            });
        };

        let acquired_at = Utc::now().to_rfc3339();
        let body_hash = format!("{:x}", Sha256::digest(&fetched.body));
        let dbar_id = evidence::insert_dbar_response(
            conn,
            &NewDbarResponse {
                disc_stable_key: &disc_stable_key,
                body_hash: &body_hash,
                body: &fetched.body,
                acquired_at: &acquired_at,
            },
        )?;
        let run_id = evidence::start_verification_run(
            conn,
            rip_file.id,
            Some(dbar_id),
            options.max_sample_shift,
            ripper_read_offset,
        )?;

        let verification = verify_with_offsets(&fetched.dbar, &pcm, options);
        let verifications = &verification.tracks;

        let mut accurate = 0;
        let mut mismatched = 0;
        let mut max_confidence: u8 = 0;
        for v in verifications {
            if v.is_verified() {
                accurate += 1;
                if let Some(c) = v.best_confidence() {
                    max_confidence = max_confidence.max(c);
                }
            } else {
                mismatched += 1;
            }
        }

        for track in verifications {
            persist_track_verification(conn, run_id, track)?;
        }
        let ambiguous_offsets: Vec<i32> = verification
            .ambiguous_offsets
            .iter()
            .map(|candidate| candidate.sample_shift)
            .collect();
        let ambiguous_json = (!ambiguous_offsets.is_empty())
            .then(|| {
                serde_json::to_string(
                    &verification
                        .ambiguous_offsets
                        .iter()
                        .map(|candidate| {
                            serde_json::json!({
                                "sample_shift": candidate.sample_shift,
                                "full_matches": candidate.full_matches,
                                "minimum_confidence": candidate.minimum_confidence,
                                "total_confidence": candidate.total_confidence,
                                "frame_450_matches": candidate.frame_450_matches,
                            })
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .transpose()
            .map_err(|error| DbError::Migration(error.to_string()))?;
        evidence::finish_verification_run(
            conn,
            run_id,
            verification_status(verification.status),
            verification.chosen_sample_shift,
            ambiguous_json.as_deref(),
        )?;

        Ok(VerifySummary {
            disc_id: disc.id,
            rip_file_id: rip_file.id,
            per_track: verifications.iter().map(VerifiedTrack::from).collect(),
            accurate,
            mismatched,
            not_in_db: false,
            max_confidence,
            inferred_sample_shift: verification.chosen_sample_shift,
            ripper_read_offset,
            ambiguous_offsets,
        })
    }
}

fn no_data_tracks(tracks: &[DiscTrackSamples]) -> Vec<TrackVerification> {
    tracks
        .iter()
        .enumerate()
        .map(|(index, track)| TrackVerification {
            position: track.position,
            computed: track_crc_samples(&track.samples, track_position(index, tracks.len())),
            v1_matches: Vec::new(),
            v2_matches: Vec::new(),
            sample_shift: None,
            frame_450_support: false,
            status: TrackVerificationStatus::NoData,
        })
        .collect()
}

fn track_position(index: usize, count: usize) -> TrackPosition {
    match (index, count) {
        (_, 1) => TrackPosition::Only,
        (0, _) => TrackPosition::First,
        (index, count) if index + 1 == count => TrackPosition::Last,
        _ => TrackPosition::Middle,
    }
}

fn resolve_target(conn: &Connection, target: VerifyTarget) -> Result<(Disc, RipFile), VerifyError> {
    match target {
        VerifyTarget::DiscId(id) => {
            let disc = crud::get_disc(conn, id)?.ok_or(VerifyError::MissingDisc(id))?;
            let rip_file =
                crud::find_rip_file_for_disc(conn, id)?.ok_or(VerifyError::MissingRipFile(id))?;
            Ok((disc, rip_file))
        }
        VerifyTarget::Path(path) => {
            let rip_file = crud::find_rip_file_by_cue_path(conn, &path)?
                .or(crud::find_rip_file_by_chd_path(conn, &path)?)
                .ok_or_else(|| VerifyError::NoRipForPath(path.clone()))?;
            let disc_id = rip_file
                .disc_id
                .ok_or_else(|| VerifyError::RipNotIdentified(path.clone()))?;
            let disc = crud::get_disc(conn, disc_id)?.ok_or(VerifyError::MissingDisc(disc_id))?;
            Ok((disc, rip_file))
        }
    }
}

fn ids_from_disc(disc: &Disc) -> Result<DiscIds, VerifyError> {
    if disc.ar_discid1.is_none() || disc.ar_discid2.is_none() || disc.cddb_id.is_none() {
        return Err(VerifyError::MissingDiscIds(disc.id));
    }
    Ok(DiscIds {
        mb_discid: disc.mb_discid.clone(),
        cddb_id: disc.cddb_id.clone(),
        ar_discid1: disc.ar_discid1.clone(),
        ar_discid2: disc.ar_discid2.clone(),
        barcode: None,
        catalog_number: None,
    })
}

fn load_layouts(rip: &RipFile) -> Result<Vec<TrackLayout>, VerifyError> {
    if let Some(chd) = rip.chd_path.as_ref() {
        return Ok(junk_libs_disc::read_chd_layout(chd)?);
    }
    if let Some(cue) = rip.cue_path.as_ref() {
        return Ok(junk_libs_disc::read_cue_layout(cue)?);
    }
    Err(VerifyError::NoRipSource(rip.id))
}

fn load_disc_pcm(
    rip: &RipFile,
    layouts: &[&TrackLayout],
) -> Result<Vec<DiscTrackSamples>, VerifyError> {
    let mut tracks = Vec::with_capacity(layouts.len());
    for layout in layouts {
        let reader = if let Some(chd) = rip.chd_path.as_ref() {
            TrackPcmReader::from_chd(chd, layout.number)?
        } else if let Some(cue) = rip.cue_path.as_ref() {
            TrackPcmReader::from_cue(cue, layout.number)?
        } else {
            return Err(VerifyError::NoRipSource(rip.id));
        };
        let mut samples = Vec::new();
        for sector in reader {
            samples.extend_from_slice(&sector?);
        }
        tracks.push(DiscTrackSamples {
            position: layout.number,
            samples,
        });
    }
    Ok(tracks)
}

fn persist_track_verification(
    conn: &Connection,
    run_id: i64,
    track: &TrackVerification,
) -> Result<(), DbError> {
    let best = track.best_match();
    evidence::insert_track_verification(
        conn,
        run_id,
        &NewTrackVerification {
            track_position: track.position,
            computed_v1: track.computed.v1,
            computed_v2: track.computed.v2,
            matched_checksum: best.map(|matched| matched.checksum),
            checksum_version: best.map(|matched| checksum_version(matched.version)),
            sample_shift: track.sample_shift,
            confidence: best.map(|matched| matched.confidence),
            response_index: best.map(|matched| matched.pressing),
            frame_450_support: track.frame_450_support,
            status: track_status(track.status),
        },
    )
}

fn checksum_version(version: ChecksumVersion) -> &'static str {
    match version {
        ChecksumVersion::V1 => "v1",
        ChecksumVersion::V2 => "v2",
        ChecksumVersion::Both => "both",
    }
}

fn track_status(status: TrackVerificationStatus) -> &'static str {
    match status {
        TrackVerificationStatus::Verified => "verified",
        TrackVerificationStatus::Mismatched => "mismatched",
        TrackVerificationStatus::NoData => "no_data",
        TrackVerificationStatus::Ambiguous => "ambiguous",
    }
}

fn verification_status(status: VerificationStatus) -> &'static str {
    match status {
        VerificationStatus::Verified => "verified",
        VerificationStatus::Mismatched => "mismatched",
        VerificationStatus::NoData => "no_data",
        VerificationStatus::AmbiguousOffsets => "ambiguous_offsets",
    }
}
