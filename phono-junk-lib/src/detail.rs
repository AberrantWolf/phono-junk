//! Detail-load helpers for Sprint 18's GUI album detail panel.
//!
//! Builds the typed detail tree from the repository's fixed-query album
//! aggregate. Catalog size changes returned rows, never query count. Export
//! resolves the same aggregate rather than rebuilding the hierarchy.
//!
//! For unidentified rips, [`load_unidentified_detail`] re-parses the TOC from
//! the on-disk CUE/CHD on demand so the panel can show a track-count + length
//! preview. TOC isn't persisted on `RipFile` (only on `Disc`, which doesn't
//! exist for unidentified rips), so re-parse is the only option.

use std::path::Path;

use phono_junk_catalog::{
    Album, Asset, Disagreement, Disc, Id, Release, RipFile, Track, pick_front_cover,
};
use phono_junk_core::Toc;
use phono_junk_db::{DbError, aggregate};
use phono_junk_toc::{read_toc_from_chd, read_toc_from_cue};
use rusqlite::Connection;

use crate::sidecar::{self, SidecarData};

/// Entire album subtree as the detail panel needs to render it.
#[derive(Debug, Clone)]
pub struct AlbumDetail {
    pub album: Album,
    pub releases: Vec<ReleaseDetail>,
    /// Unresolved + resolved disagreements scoped to the `Album` entity.
    pub disagreements: Vec<Disagreement>,
}

#[derive(Debug, Clone)]
pub struct ReleaseDetail {
    pub release: Release,
    pub discs: Vec<DiscDetail>,
    pub assets: Vec<Asset>,
    /// Pre-resolved front-cover (via `pick_front_cover`) so the view never
    /// re-runs the heuristic and both the detail panel and export agree on
    /// which asset is "the cover".
    pub cover_asset: Option<Asset>,
    pub disagreements: Vec<Disagreement>,
}

#[derive(Debug, Clone)]
pub struct DiscDetail {
    pub disc: Disc,
    pub tracks: Vec<Track>,
    /// First `RipFile` linked to this disc (none if the catalog row was
    /// imported without a backing file). Carries the latest derived
    /// verification status, timestamp, and inferred shift for the AR badge.
    pub rip_file: Option<RipFile>,
    pub disagreements: Vec<Disagreement>,
}

/// Errors from [`load_album_detail`] / [`load_unidentified_detail`].
#[derive(Debug, thiserror::Error)]
pub enum DetailError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("album {0} not found")]
    AlbumMissing(Id),
}

/// Compose the full album subtree from one repository aggregate.
pub fn load_album_detail(conn: &Connection, album_id: Id) -> Result<AlbumDetail, DetailError> {
    let aggregate =
        aggregate::load_album(conn, album_id)?.ok_or(DetailError::AlbumMissing(album_id))?;
    let mut releases = Vec::with_capacity(aggregate.releases.len());
    for release in aggregate.releases {
        let mut discs = Vec::new();
        for disc in aggregate
            .discs
            .iter()
            .filter(|disc| disc.release_id == release.id)
            .cloned()
        {
            let tracks = aggregate
                .tracks
                .iter()
                .filter(|track| track.disc_id == disc.id)
                .cloned()
                .collect();
            let rip_file = aggregate
                .rip_files
                .iter()
                .find(|rip| rip.disc_id == Some(disc.id))
                .cloned();
            let disagreements = disagreements_for(&aggregate.disagreements, "Disc", disc.id);
            discs.push(DiscDetail {
                disc,
                tracks,
                rip_file,
                disagreements,
            });
        }
        let assets: Vec<_> = aggregate
            .assets
            .iter()
            .filter(|asset| asset.release_id == release.id)
            .cloned()
            .collect();
        let cover_asset = pick_front_cover(&assets).cloned();
        let release_disagreements =
            disagreements_for(&aggregate.disagreements, "Release", release.id);
        releases.push(ReleaseDetail {
            release,
            discs,
            assets,
            cover_asset,
            disagreements: release_disagreements,
        });
    }
    let disagreements = disagreements_for(&aggregate.disagreements, "Album", album_id);
    Ok(AlbumDetail {
        album: aggregate.album,
        releases,
        disagreements,
    })
}

fn disagreements_for(
    disagreements: &[Disagreement],
    entity_type: &str,
    entity_id: Id,
) -> Vec<Disagreement> {
    disagreements
        .iter()
        .filter(|item| {
            item.entity_id == entity_id && item.entity_type.eq_ignore_ascii_case(entity_type)
        })
        .cloned()
        .collect()
}

/// Detail payload for an unidentified rip — the rip file row (which carries
/// the persisted `last_identify_errors` + `last_identify_at` fields) plus an
/// on-the-fly TOC re-parse and a fresh sidecar collection.
///
/// Sidecar data (MCN, ISRCs, CD-TEXT titles/performers) is transient for
/// unidentified rips — only `RipFile.provenance` persists; the rest lives on
/// `Disc.mcn` / `Track.isrc` which don't exist until identify succeeds.
/// Re-collecting here lets the panel surface it anyway, especially useful for
/// foreign-language discs where CD-TEXT titles are the only readable metadata.
#[derive(Debug, Clone)]
pub struct UnidentifiedDetail {
    pub rip_file: RipFile,
    pub toc: Option<Toc>,
    /// Populated when `toc` is `None` — typically because the CUE/CHD file
    /// was moved or deleted after the scan. Renders inline in the panel so
    /// the user can act on it instead of seeing a silent blank.
    pub toc_error: Option<String>,
    /// Sidecar artefacts re-collected from the CUE's neighbouring files
    /// (redumper `.log`, `.cdtext`). Empty for CHD-only rips and when no
    /// sidecars exist next to the CUE.
    pub sidecar: SidecarData,
}

/// Re-parse the on-disk TOC from `rip_file`'s CUE or CHD so the panel can
/// show track count / lengths even though no `Disc` row exists yet.
///
/// CUE re-parse is microseconds; CHD reads one hunk via Sprint 16's
/// `ChdHunkCache`. A missing file becomes a `toc_error` string rather than a
/// hard failure — the rip-file row is still useful (path + last identify
/// errors render even without a TOC).
pub fn load_unidentified_detail(rip_file: RipFile) -> UnidentifiedDetail {
    let (toc, toc_error) = match read_toc_for(&rip_file) {
        Ok(Some(t)) => (Some(t), None),
        Ok(None) => (
            None,
            Some("rip file has neither cue_path nor chd_path".to_string()),
        ),
        Err(e) => (None, Some(e)),
    };
    // Sidecars only attach to CUE-based rips (CHD has no sibling log/cdtext
    // in its container today). Mirrors the scan pipeline's policy.
    let sidecar = match rip_file.cue_path.as_deref() {
        Some(cue) => sidecar::collect_redumper_sidecars(cue),
        None => SidecarData::default(),
    };
    UnidentifiedDetail {
        rip_file,
        toc,
        toc_error,
        sidecar,
    }
}

fn read_toc_for(rip_file: &RipFile) -> Result<Option<Toc>, String> {
    if let Some(cue) = rip_file.cue_path.as_ref() {
        return read_toc_or_msg(cue, read_toc_from_cue).map(Some);
    }
    if let Some(chd) = rip_file.chd_path.as_ref() {
        return read_toc_or_msg(chd, read_toc_from_chd).map(Some);
    }
    Ok(None)
}

fn read_toc_or_msg<F, E>(path: &Path, f: F) -> Result<Toc, String>
where
    F: FnOnce(&Path) -> Result<Toc, E>,
    E: std::fmt::Display,
{
    f(path).map_err(|e| format!("{}: {}", path.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use phono_junk_catalog::IdentifyAttemptError;
    use phono_junk_core::{IdentificationConfidence, IdentificationState};
    use std::path::PathBuf;

    fn rip(cue: Option<&str>, chd: Option<&str>) -> RipFile {
        RipFile {
            id: 1,
            disc_id: None,
            cue_path: cue.map(PathBuf::from),
            chd_path: chd.map(PathBuf::from),
            bin_paths: Vec::new(),
            mtime: None,
            size: None,
            identification_confidence: IdentificationConfidence::Unidentified,
            identification_source: None,
            accuraterip_status: None,
            last_verified_at: None,
            inferred_sample_shift: None,
            last_identify_errors: Some(vec![IdentifyAttemptError {
                provider: "MusicBrainz".into(),
                message: "no match found".into(),
            }]),
            last_identify_at: Some("2026-04-20T12:00:00Z".into()),
            provenance: None,
            identification_state: IdentificationState::Unidentified,
            last_state_change_at: Some("2026-04-20T12:00:00Z".into()),
        }
    }

    #[test]
    fn unidentified_detail_missing_paths_yields_toc_error() {
        let d = load_unidentified_detail(rip(None, None));
        assert!(d.toc.is_none());
        assert!(d.toc_error.as_deref().unwrap().contains("neither"));
        // Persisted error log survives intact.
        assert_eq!(d.rip_file.last_identify_errors.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn unidentified_detail_missing_cue_file_returns_named_error() {
        let d = load_unidentified_detail(rip(Some("/no/such/path.cue"), None));
        assert!(d.toc.is_none());
        let msg = d.toc_error.unwrap();
        assert!(msg.contains("/no/such/path.cue"));
    }

    #[test]
    fn unidentified_detail_collects_sibling_redumper_log() {
        // Smoke test: dropping a minimally-shaped redumper log next to a CUE
        // surfaces provenance on `UnidentifiedDetail.sidecar` without any
        // scan pipeline involvement. Verifies Bug 1 — the detail panel no
        // longer needs persistence to show sidecar-derived facts.
        let tmp = tempfile::tempdir().unwrap();
        let cue_path = tmp.path().join("foo.cue");
        std::fs::write(
            &cue_path,
            b"FILE \"foo.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n",
        )
        .unwrap();
        let log_path = tmp.path().join("foo.log");
        std::fs::write(
            &log_path,
            b"redumper v2024.03.01 build_1\n\nMCN: 0123456789012\n",
        )
        .unwrap();

        let mut r = rip(Some(cue_path.to_str().unwrap()), None);
        r.cue_path = Some(cue_path.clone());
        let d = load_unidentified_detail(r);

        assert!(d.sidecar.provenance.is_some());
        assert_eq!(
            d.sidecar.mcn.as_deref(),
            Some("0123456789012"),
            "MCN line should be parsed from the log",
        );
    }

    #[test]
    fn unidentified_detail_no_sidecar_yields_empty_sidecar_data() {
        let tmp = tempfile::tempdir().unwrap();
        let cue_path = tmp.path().join("bare.cue");
        std::fs::write(
            &cue_path,
            b"FILE \"bare.bin\" BINARY\n  TRACK 01 AUDIO\n    INDEX 01 00:00:00\n",
        )
        .unwrap();
        let mut r = rip(Some(cue_path.to_str().unwrap()), None);
        r.cue_path = Some(cue_path.clone());
        let d = load_unidentified_detail(r);
        assert!(d.sidecar.is_empty());
    }
}
