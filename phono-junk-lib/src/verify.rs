//! Verify pipeline: [`PhonoContext::verify_disc`].
//!
//! Computes per-track AccurateRip CRCs from the rip's BIN/CHD, fetches
//! the matching dBAR file, compares, and persists the summary on the
//! `RipFile` row. Identification is *not* re-run — see CLAUDE.md's
//! identification-vs-verification split.

use std::path::PathBuf;

use chrono::Utc;
use junk_libs_disc::{TrackKind, TrackLayout};
use phono_junk_accuraterip::{
    AccurateRipError, TrackCrc, TrackPosition, TrackVerification, track_crc_from_chd,
    track_crc_from_cue, verify_disc as verify_disc_against_dbar,
};
use phono_junk_catalog::{Disc, Id, RipFile};
use phono_junk_core::DiscIds;
use phono_junk_db::{DbError, crud};
use phono_junk_identify::HttpError;
use rusqlite::Connection;
use serde::Serialize;

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
    /// Verify one disc against AccurateRip. Persists a compact status
    /// string + timestamp to the linked `RipFile` row on success.
    ///
    /// Returns a [`VerifySummary`] whose `not_in_db == true` case is *not*
    /// an error — it's a legitimate "we looked, AccurateRip has no
    /// submissions for this TOC triple" outcome.
    pub fn verify_disc(
        &self,
        conn: &Connection,
        target: VerifyTarget,
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

        let dbar = client.fetch_dbar(&ids, track_count)?;
        let Some(dbar) = dbar else {
            persist_verification(conn, &rip_file, "not in accuraterip db", &[])?;
            return Ok(VerifySummary {
                disc_id: disc.id,
                rip_file_id: rip_file.id,
                per_track: Vec::new(),
                accurate: 0,
                mismatched: 0,
                not_in_db: true,
                max_confidence: 0,
            });
        };

        let mut computed: Vec<(u8, TrackCrc)> = Vec::with_capacity(audio_layouts.len());
        for (i, layout) in audio_layouts.iter().enumerate() {
            let position = track_position(i, audio_layouts.len());
            let crc = compute_track_crc(&rip_file, layout, position)?;
            computed.push((layout.number, crc));
        }

        let verifications = verify_disc_against_dbar(&dbar, &computed);

        let mut accurate = 0;
        let mut mismatched = 0;
        let mut max_confidence: u8 = 0;
        for v in &verifications {
            if v.is_verified() {
                accurate += 1;
                if let Some(c) = v.best_confidence() {
                    max_confidence = max_confidence.max(c);
                }
            } else {
                mismatched += 1;
            }
        }

        let status_str = format_status(&verifications);
        persist_verification(conn, &rip_file, &status_str, &verifications)?;

        Ok(VerifySummary {
            disc_id: disc.id,
            rip_file_id: rip_file.id,
            per_track: verifications.iter().map(VerifiedTrack::from).collect(),
            accurate,
            mismatched,
            not_in_db: false,
            max_confidence,
        })
    }
}

fn resolve_target(
    conn: &Connection,
    target: VerifyTarget,
) -> Result<(Disc, RipFile), VerifyError> {
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

fn track_position(i: usize, n: usize) -> TrackPosition {
    match (i, n) {
        (_, 1) => TrackPosition::Only,
        (0, _) => TrackPosition::First,
        (idx, len) if idx + 1 == len => TrackPosition::Last,
        _ => TrackPosition::Middle,
    }
}

fn compute_track_crc(
    rip: &RipFile,
    layout: &TrackLayout,
    position: TrackPosition,
) -> Result<TrackCrc, VerifyError> {
    if let Some(chd) = rip.chd_path.as_ref() {
        return Ok(track_crc_from_chd(chd, layout.number, position)?);
    }
    if let Some(cue) = rip.cue_path.as_ref() {
        return Ok(track_crc_from_cue(cue, layout.number, position)?);
    }
    Err(VerifyError::NoRipSource(rip.id))
}

fn format_status(vs: &[TrackVerification]) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(vs.len());
    for v in vs {
        parts.push(format!("{}:{}", v.position, v.status_string()));
    }
    parts.join(", ")
}

fn persist_verification(
    conn: &Connection,
    rip: &RipFile,
    status: &str,
    _verifications: &[TrackVerification],
) -> Result<(), DbError> {
    let mut updated = rip.clone();
    updated.accuraterip_status = Some(status.to_string());
    updated.last_verified_at = Some(Utc::now().to_rfc3339());
    crud::update_rip_file(conn, &updated)
}
