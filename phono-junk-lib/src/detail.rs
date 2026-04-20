//! Detail-load helpers for Sprint 18's GUI album detail panel.
//!
//! Composes the disc-tree CRUD chain (`get_album` → `list_releases_for_album`
//! → `list_discs_for_release` → `list_tracks_for_disc` + `list_assets_for_release`
//! + `list_disagreements_for` + `find_rip_file_for_disc`) into a single
//! call so the panel reads from a typed `AlbumDetail` value rather than
//! reissuing the same join shape inline.
//!
//! Second consumer of this composition (`extract::export_disc` is the first).
//! A follow-up sprint should refactor `export_disc` onto this module so
//! disc-tree assembly has exactly one implementation. See TODO.md.
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
use phono_junk_db::{DbError, crud};
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
    /// imported without a backing file). Carries `accuraterip_status` /
    /// `last_verified_at` for the AR badge.
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

/// Compose the full album subtree in one call. ~O(releases × discs) DB
/// queries — fine at MVP catalog sizes; lift to a worker only if a profile
/// proves it hot.
pub fn load_album_detail(conn: &Connection, album_id: Id) -> Result<AlbumDetail, DetailError> {
    let album = crud::get_album(conn, album_id)?
        .ok_or(DetailError::AlbumMissing(album_id))?;
    let releases_raw = crud::list_releases_for_album(conn, album_id)?;
    let mut releases = Vec::with_capacity(releases_raw.len());
    for release in releases_raw {
        let discs_raw = crud::list_discs_for_release(conn, release.id)?;
        let mut discs = Vec::with_capacity(discs_raw.len());
        for disc in discs_raw {
            let tracks = crud::list_tracks_for_disc(conn, disc.id)?;
            let rip_file = crud::find_rip_file_for_disc(conn, disc.id)?;
            let disagreements = crud::list_disagreements_for(conn, "Disc", disc.id)?;
            discs.push(DiscDetail {
                disc,
                tracks,
                rip_file,
                disagreements,
            });
        }
        let assets = crud::list_assets_for_release(conn, release.id)?;
        let cover_asset = pick_front_cover(&assets).cloned();
        let release_disagreements = crud::list_disagreements_for(conn, "Release", release.id)?;
        releases.push(ReleaseDetail {
            release,
            discs,
            assets,
            cover_asset,
            disagreements: release_disagreements,
        });
    }
    let disagreements = crud::list_disagreements_for(conn, "Album", album_id)?;
    Ok(AlbumDetail {
        album,
        releases,
        disagreements,
    })
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
        return read_toc_or_msg(cue, |p| read_toc_from_cue(p)).map(Some);
    }
    if let Some(chd) = rip_file.chd_path.as_ref() {
        return read_toc_or_msg(chd, |p| read_toc_from_chd(p)).map(Some);
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
