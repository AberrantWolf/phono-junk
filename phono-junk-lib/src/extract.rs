//! Extract pipeline: [`PhonoContext::export_disc`].
//!
//! Sprint 12's high-level facade. Given a persisted `Disc`, walks the
//! catalog (Album/Release/Disc/Tracks/Assets), resolves BIN/CHD source
//! via the rip_files row, lazily caches cover bytes to disk, then hands
//! off to `phono-junk-extract` for per-track FLAC encoding + tagging.
//!
//! All DB and HTTP orchestration lives here so `phono-junk-extract` can
//! stay pure and reusable — the same primitive `encode_flac_track` is
//! callable from a CLI dry-run, a GUI progress-driven loop, or a future
//! batch-export policy.

use std::fs;
use std::path::{Path, PathBuf};

use junk_libs_disc::{TrackLayout, TrackPcmReader};
use phono_junk_catalog::{Album, Asset, AssetType, Disc, Id, Release, RipFile, Track};
use phono_junk_db::{DbError, crud};
use phono_junk_extract::{
    ExtractError as ExtractPrimitiveError, TrackTags, encode_flac_track, plan_disc_directory,
    plan_output_paths,
};
use phono_junk_identify::HttpError;
use rusqlite::Connection;

use crate::PhonoContext;

/// Output summary — every file that was written to disk.
#[derive(Debug, Clone, Default)]
pub struct ExportedDisc {
    pub disc_id: Id,
    pub written: Vec<PathBuf>,
    /// Whether a cover file was produced (`cover.jpg` alongside the FLACs).
    pub cover_written: bool,
}

/// Errors from [`PhonoContext::export_disc`].
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Extract(#[from] ExtractPrimitiveError),
    #[error(transparent)]
    Analysis(#[from] junk_libs_core::AnalysisError),
    #[error("HTTP error fetching asset: {0}")]
    Http(#[from] HttpError),
    #[error("catalog row missing: {0}")]
    MissingRow(&'static str),
    #[error("disc {0} has no linked rip_files row")]
    MissingRipFile(Id),
    #[error("disc {0} has no usable source: cue_path and chd_path both empty")]
    NoRipSource(Id),
    #[error("no HttpClient registered on PhonoContext; use with_default_providers() or set ctx.http")]
    NoHttpClient,
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl ExportError {
    fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

impl PhonoContext {
    /// Encode every track of `disc_id` into FLAC files under `library_root`,
    /// embed Vorbis tags + front-cover art, and drop a `cover.jpg` sidecar.
    ///
    /// Cover bytes are fetched on first export and cached under
    /// `<library_root>/.cache/assets/<asset_id>.<ext>`; the `Asset.file_path`
    /// column is updated to that location so subsequent exports (or
    /// re-exports across a moved library) skip the fetch.
    pub fn export_disc(
        &self,
        conn: &Connection,
        disc_id: Id,
        library_root: &Path,
    ) -> Result<ExportedDisc, ExportError> {
        let disc = crud::get_disc(conn, disc_id)?
            .ok_or(ExportError::MissingRow("disc"))?;
        let release = crud::get_release(conn, disc.release_id)?
            .ok_or(ExportError::MissingRow("release"))?;
        let album = crud::get_album(conn, release.album_id)?
            .ok_or(ExportError::MissingRow("album"))?;
        let tracks = crud::list_tracks_for_disc(conn, disc_id)?;
        let assets = crud::list_assets_for_release(conn, release.id)?;
        let sibling_discs = crud::list_discs_for_release(conn, release.id)?;
        let total_discs = sibling_discs.len().max(1) as u8;

        let rip_file = crud::find_rip_file_for_disc(conn, disc_id)?
            .ok_or(ExportError::MissingRipFile(disc_id))?;

        let album_artist = resolve_album_artist(&album, &tracks);
        let out_paths = plan_output_paths(
            library_root,
            &album,
            disc.disc_number,
            total_discs,
            &tracks,
            Some(&album_artist),
        );
        let disc_dir = plan_disc_directory(
            library_root,
            &album,
            disc.disc_number,
            total_discs,
            Some(&album_artist),
        );

        let cover_bytes = resolve_cover_bytes(self, conn, &assets, library_root)?;

        let layouts = load_track_layouts(&rip_file, disc_id)?;
        verify_layouts_match_tracks(&layouts, &tracks, disc_id)?;

        let mut written: Vec<PathBuf> = Vec::with_capacity(out_paths.len());
        for (track, out_path) in tracks.iter().zip(out_paths.iter()) {
            let layout = find_layout_for_track(&layouts, track.position)
                .ok_or_else(|| {
                    ExtractPrimitiveError::InvalidTrack(format!(
                        "no layout entry for track position {}",
                        track.position
                    ))
                })?;
            let pcm = open_pcm_reader(&rip_file, layout)?;
            let total_samples = pcm.total_samples();
            let tags = build_track_tags(
                &album,
                &release,
                &disc,
                track,
                &tracks,
                total_discs,
                &album_artist,
            );
            encode_flac_track(
                pcm,
                total_samples,
                &tags,
                cover_bytes.as_deref(),
                out_path,
            )?;
            written.push(out_path.clone());
        }

        let cover_written = if let Some(bytes) = cover_bytes.as_deref() {
            fs::create_dir_all(&disc_dir).map_err(|e| ExportError::io(&disc_dir, e))?;
            let cover_path = disc_dir.join("cover.jpg");
            fs::write(&cover_path, bytes).map_err(|e| ExportError::io(&cover_path, e))?;
            written.push(cover_path);
            true
        } else {
            false
        };

        Ok(ExportedDisc {
            disc_id,
            written,
            cover_written,
        })
    }
}

fn resolve_album_artist(album: &Album, tracks: &[Track]) -> String {
    // Explicit "Various Artists" on the album row wins.
    if album.artist_credit.as_deref() == Some("Various Artists") {
        return "Various Artists".into();
    }
    // Heuristic: every track has a credit, they differ between tracks, and
    // none match the album-level credit → treat as VA.
    if !tracks.is_empty() && tracks.iter().all(|t| t.artist_credit.is_some()) {
        let distinct: std::collections::HashSet<&str> = tracks
            .iter()
            .filter_map(|t| t.artist_credit.as_deref())
            .collect();
        let mismatch_album = match &album.artist_credit {
            Some(a) => !distinct.contains(a.as_str()),
            None => true,
        };
        if distinct.len() > 1 && mismatch_album {
            return "Various Artists".into();
        }
    }
    album
        .artist_credit
        .clone()
        .unwrap_or_else(|| "Unknown Artist".into())
}

fn build_track_tags(
    album: &Album,
    release: &Release,
    _disc: &Disc,
    track: &Track,
    all_tracks: &[Track],
    total_discs: u8,
    album_artist: &str,
) -> TrackTags {
    let artist = track
        .artist_credit
        .clone()
        .or_else(|| album.artist_credit.clone())
        .unwrap_or_else(|| album_artist.to_string());
    let title = track
        .title
        .clone()
        .unwrap_or_else(|| format!("Track {:02}", track.position));
    let date = album
        .first_release_date
        .clone()
        .or_else(|| release.date.clone())
        .or_else(|| album.year.map(|y| y.to_string()));
    TrackTags {
        album: album.title.clone(),
        album_artist: album_artist.to_string(),
        artist,
        title,
        track_number: track.position,
        total_tracks: all_tracks.len() as u8,
        // Disc number comes from the Disc row via caller; total_discs from sibling count.
        disc_number: _disc.disc_number,
        total_discs,
        date,
        genre: None,
        musicbrainz_album_id: album.mbid.clone(),
        musicbrainz_release_track_id: track.mbid.clone(),
        isrc: track.isrc.clone(),
    }
}

fn load_track_layouts(rip: &RipFile, disc_id: Id) -> Result<Vec<TrackLayout>, ExportError> {
    if let Some(cue) = rip.cue_path.as_ref() {
        let layout = junk_libs_disc::read_cue_layout(cue)?;
        return Ok(layout);
    }
    if let Some(chd) = rip.chd_path.as_ref() {
        let layout = junk_libs_disc::read_chd_layout(chd)?;
        return Ok(layout);
    }
    Err(ExportError::NoRipSource(disc_id))
}

fn find_layout_for_track(layouts: &[TrackLayout], position: u8) -> Option<&TrackLayout> {
    layouts.iter().find(|l| l.number == position)
}

fn verify_layouts_match_tracks(
    layouts: &[TrackLayout],
    tracks: &[Track],
    disc_id: Id,
) -> Result<(), ExportError> {
    for t in tracks {
        if find_layout_for_track(layouts, t.position).is_none() {
            return Err(ExtractPrimitiveError::InvalidTrack(format!(
                "disc {disc_id}: catalog track position {} absent from rip TOC",
                t.position
            ))
            .into());
        }
    }
    Ok(())
}

fn open_pcm_reader(
    rip: &RipFile,
    layout: &TrackLayout,
) -> Result<TrackPcmReader, junk_libs_core::AnalysisError> {
    if let Some(chd) = rip.chd_path.as_ref() {
        return TrackPcmReader::from_chd(chd, layout);
    }
    if let Some(cue) = rip.cue_path.as_ref() {
        // When backed by a single-BIN CUE, the first bin_path is the
        // whole-disc image. If bin_paths is empty (unusual but tolerated
        // by catalog seeds), fall back to deriving a `.bin` path next to
        // the CUE — junk_libs_disc will error loudly if that doesn't exist.
        let bin = rip
            .bin_paths
            .first()
            .cloned()
            .unwrap_or_else(|| cue.with_extension("bin"));
        return TrackPcmReader::from_bin(&bin, layout);
    }
    Err(junk_libs_core::AnalysisError::invalid_format(
        "rip_file has neither cue_path nor chd_path",
    ))
}

/// Pick the front-cover asset, ensure its bytes are locally cached, and
/// return those bytes. Returns `Ok(None)` if the release has no front
/// cover at all — export proceeds without embedded art.
fn resolve_cover_bytes(
    ctx: &PhonoContext,
    conn: &Connection,
    assets: &[Asset],
    library_root: &Path,
) -> Result<Option<Vec<u8>>, ExportError> {
    let Some(asset) = pick_front_cover(assets) else {
        return Ok(None);
    };
    let bytes = ensure_asset_cached(ctx, conn, asset, library_root)?;
    Ok(Some(bytes))
}

fn pick_front_cover(assets: &[Asset]) -> Option<&Asset> {
    assets
        .iter()
        .filter(|a| a.asset_type == AssetType::FrontCover)
        .min_by_key(|a| (a.group_id.unwrap_or(i64::MAX), a.sequence, a.id))
}

/// Ensure an Asset's bytes exist on disk under `library_root/.cache/assets/`.
/// If `asset.file_path` already points at a readable file, use it. Otherwise
/// download via `ctx.http`, persist to a path keyed by asset id, and write
/// back the relative path to the DB row.
fn ensure_asset_cached(
    ctx: &PhonoContext,
    conn: &Connection,
    asset: &Asset,
    library_root: &Path,
) -> Result<Vec<u8>, ExportError> {
    if let Some(rel_or_abs) = asset.file_path.as_ref() {
        let candidate = if rel_or_abs.is_absolute() {
            rel_or_abs.clone()
        } else {
            library_root.join(rel_or_abs)
        };
        if candidate.exists() {
            return fs::read(&candidate).map_err(|e| ExportError::io(&candidate, e));
        }
    }
    let http = ctx.http.as_ref().ok_or(ExportError::NoHttpClient)?;
    let url = asset
        .source_url
        .as_deref()
        .ok_or(ExtractPrimitiveError::InvalidTrack(
            "front-cover asset has neither file_path nor source_url".into(),
        ))?;
    let resp = http.get(url)?;
    let ext = cover_extension(&resp.content_type, url);
    let cache_dir = library_root.join(".cache").join("assets");
    fs::create_dir_all(&cache_dir).map_err(|e| ExportError::io(&cache_dir, e))?;
    let filename = format!("{}.{}", asset.id, ext);
    let abs_path = cache_dir.join(&filename);
    fs::write(&abs_path, &resp.body).map_err(|e| ExportError::io(&abs_path, e))?;

    // Persist a library-root-relative path so the library stays portable.
    let relative = PathBuf::from(".cache").join("assets").join(&filename);
    let mut updated = asset.clone();
    updated.file_path = Some(relative);
    crud::update_asset(conn, &updated)?;
    Ok(resp.body)
}

/// Decide a file extension for the cached cover. Prefers Content-Type;
/// falls back to URL suffix; defaults to `jpg`.
fn cover_extension(content_type: &Option<String>, url: &str) -> String {
    if let Some(ct) = content_type.as_deref() {
        let ct = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        match ct.as_str() {
            "image/jpeg" | "image/jpg" => return "jpg".into(),
            "image/png" => return "png".into(),
            "image/webp" => return "webp".into(),
            _ => {}
        }
    }
    let lower = url.to_ascii_lowercase();
    for ext in ["jpg", "jpeg", "png", "webp"] {
        if lower.ends_with(&format!(".{ext}")) {
            return if ext == "jpeg" { "jpg".into() } else { ext.into() };
        }
    }
    "jpg".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_album(artist: Option<&str>) -> Album {
        Album {
            id: 0,
            title: "t".into(),
            sort_title: None,
            artist_credit: artist.map(String::from),
            year: None,
            mbid: None,
            primary_type: None,
            secondary_types: Vec::new(),
            first_release_date: None,
        }
    }

    fn mk_track(position: u8, artist: Option<&str>) -> Track {
        Track {
            id: 0,
            disc_id: 0,
            position,
            title: Some(format!("t{position}")),
            artist_credit: artist.map(String::from),
            length_frames: None,
            isrc: None,
            mbid: None,
            recording_mbid: None,
        }
    }

    #[test]
    fn va_when_album_credit_says_so() {
        let album = mk_album(Some("Various Artists"));
        let tracks = vec![mk_track(1, Some("A")), mk_track(2, Some("B"))];
        assert_eq!(resolve_album_artist(&album, &tracks), "Various Artists");
    }

    #[test]
    fn va_heuristic_when_tracks_all_differ() {
        let album = mk_album(None);
        let tracks = vec![mk_track(1, Some("A")), mk_track(2, Some("B"))];
        assert_eq!(resolve_album_artist(&album, &tracks), "Various Artists");
    }

    #[test]
    fn non_va_when_all_tracks_match_album_credit() {
        let album = mk_album(Some("Weezer"));
        let tracks = vec![mk_track(1, Some("Weezer")), mk_track(2, Some("Weezer"))];
        assert_eq!(resolve_album_artist(&album, &tracks), "Weezer");
    }

    #[test]
    fn fallback_to_unknown_when_album_and_tracks_empty() {
        let album = mk_album(None);
        let tracks: Vec<Track> = Vec::new();
        assert_eq!(resolve_album_artist(&album, &tracks), "Unknown Artist");
    }

    #[test]
    fn cover_extension_prefers_content_type() {
        assert_eq!(
            cover_extension(&Some("image/png".into()), "http://x/y.jpg"),
            "png"
        );
        assert_eq!(
            cover_extension(&Some("image/jpeg; charset=binary".into()), "http://x/y"),
            "jpg"
        );
    }

    #[test]
    fn cover_extension_falls_back_to_url() {
        assert_eq!(cover_extension(&None, "http://x/y.PNG"), "png");
        assert_eq!(cover_extension(&None, "http://x/y.jpeg"), "jpg");
        assert_eq!(cover_extension(&None, "http://x/y"), "jpg");
    }
}
