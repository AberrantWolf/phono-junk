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
use phono_junk_catalog::{Album, Asset, Disc, Id, Release, RipFile, Track};
use phono_junk_db::{DbError, aggregate, crud};
use phono_junk_extract::{
    EmbeddedPicture, ExtractError as ExtractPrimitiveError, TrackTags, encode_flac_track,
    plan_disc_directory, plan_output_paths,
};
use phono_junk_identify::HttpError;
use rusqlite::Connection;

use crate::PhonoContext;
use crate::env;

/// Output summary — every file that was written to disk.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ExportedDisc {
    pub disc_id: Id,
    pub written: Vec<PathBuf>,
    /// Whether a detected-format cover file was produced alongside the FLACs.
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
    #[error("asset {asset_id} fetch {url} returned HTTP {status}")]
    AssetFetchStatus {
        asset_id: Id,
        url: String,
        status: u16,
    },
    #[error("catalog row missing: {0}")]
    MissingRow(&'static str),
    #[error("disc {0} has no linked rip_files row")]
    MissingRipFile(Id),
    #[error("disc {0} has no usable source: cue_path and chd_path both empty")]
    NoRipSource(Id),
    #[error("asset {asset_id} is not a supported JPEG, PNG, or WebP image")]
    UnsupportedArtwork { asset_id: Id },
    #[error(
        "no HttpClient registered on PhonoContext; use with_default_providers() or set ctx.http"
    )]
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
    /// Resolve the same catalog aggregate as export and return the paths that
    /// would be produced, without reading PCM, fetching artwork, or writing.
    pub fn plan_export_disc(
        &self,
        conn: &Connection,
        disc_id: Id,
        library_root: &Path,
    ) -> Result<ExportedDisc, ExportError> {
        let aggregate =
            aggregate::load_for_disc(conn, disc_id)?.ok_or(ExportError::MissingRow("disc"))?;
        let disc = aggregate
            .discs
            .iter()
            .find(|disc| disc.id == disc_id)
            .ok_or(ExportError::MissingRow("disc"))?;
        let release = aggregate
            .releases
            .iter()
            .find(|release| release.id == disc.release_id)
            .ok_or(ExportError::MissingRow("release"))?;
        let tracks: Vec<_> = aggregate
            .tracks
            .iter()
            .filter(|track| track.disc_id == disc_id)
            .cloned()
            .collect();
        let total_discs = aggregate
            .discs
            .iter()
            .filter(|item| item.release_id == release.id)
            .count()
            .max(1) as u8;
        let album_artist = resolve_album_artist(&aggregate.album, &tracks);
        let written = plan_output_paths(
            library_root,
            &aggregate.album,
            disc.disc_number,
            total_discs,
            &tracks,
            Some(&album_artist),
        );
        Ok(ExportedDisc {
            disc_id,
            written,
            cover_written: false,
        })
    }

    /// Encode every track of `disc_id` into FLAC files under `library_root`,
    /// embed Vorbis tags + front-cover art, and drop a `cover.<detected-ext>` sidecar.
    ///
    /// Cover bytes are fetched on first export via [`cache_asset_bytes`] into
    /// the OS cache dir ([`env::default_asset_cache_dir`]); the `Asset.file_path`
    /// column is updated to that absolute path so subsequent exports and GUI
    /// detail-panel views skip the fetch. The cache is process-wide, not
    /// library-specific — the exported cover sidecar and embedded
    /// FLAC art are what travel with the library.
    pub fn export_disc(
        &self,
        conn: &Connection,
        disc_id: Id,
        library_root: &Path,
    ) -> Result<ExportedDisc, ExportError> {
        let aggregate =
            aggregate::load_for_disc(conn, disc_id)?.ok_or(ExportError::MissingRow("disc"))?;
        let disc = aggregate
            .discs
            .iter()
            .find(|disc| disc.id == disc_id)
            .cloned()
            .ok_or(ExportError::MissingRow("disc"))?;
        let release = aggregate
            .releases
            .iter()
            .find(|release| release.id == disc.release_id)
            .cloned()
            .ok_or(ExportError::MissingRow("release"))?;
        let album = aggregate.album;
        let tracks: Vec<_> = aggregate
            .tracks
            .iter()
            .filter(|track| track.disc_id == disc_id)
            .cloned()
            .collect();
        let assets: Vec<_> = aggregate
            .assets
            .iter()
            .filter(|asset| asset.release_id == release.id)
            .cloned()
            .collect();
        let total_discs = aggregate
            .discs
            .iter()
            .filter(|item| item.release_id == release.id)
            .count()
            .max(1) as u8;

        let rip_file = aggregate
            .rip_files
            .iter()
            .find(|rip| rip.disc_id == Some(disc_id))
            .cloned()
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

        let cache_dir = resolve_asset_cache_dir(library_root);
        let cover = resolve_cover(self, conn, &assets, &cache_dir)?;

        let layouts = load_track_layouts(&rip_file, disc_id)?;
        verify_layouts_match_tracks(&layouts, &tracks, disc_id)?;

        let mut written: Vec<PathBuf> = Vec::with_capacity(out_paths.len());
        for (track, out_path) in tracks.iter().zip(out_paths.iter()) {
            let pcm = open_pcm_reader(&rip_file, track.position)?;
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
            let picture = cover.as_ref().map(|art| EmbeddedPicture {
                mime_type: art.kind.mime_type(),
                bytes: &art.bytes,
            });
            encode_flac_track(pcm, total_samples, &tags, picture, out_path)?;
            written.push(out_path.clone());
        }

        let cover_written = if let Some(art) = cover.as_ref() {
            fs::create_dir_all(&disc_dir).map_err(|e| ExportError::io(&disc_dir, e))?;
            let cover_path = disc_dir.join(format!("cover.{}", art.kind.extension()));
            fs::write(&cover_path, &art.bytes).map_err(|e| ExportError::io(&cover_path, e))?;
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

/// Reconstruct per-track disc layout from a rip's on-disk source. Exposed
/// as `pub` so consumers outside the export pipeline — the GUI's inline
/// playback path, any future CLI `play` — don't re-derive the CUE / CHD
/// parsing glue.
pub fn load_track_layouts(rip: &RipFile, disc_id: Id) -> Result<Vec<TrackLayout>, ExportError> {
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

pub fn find_layout_for_track(layouts: &[TrackLayout], position: u8) -> Option<&TrackLayout> {
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

/// Open a `TrackPcmReader` over the rip's CUE or CHD source, positioned at
/// the start of the track identified by `track_number`. Exposed as `pub`
/// so the GUI playback path (and any future consumer that wants raw PCM
/// from a catalogued rip) reuses exactly the export pipeline's source
/// selection. Delegates to `TrackPcmReader::from_cue` / `from_chd`, which
/// handle single-BIN and multi-BIN CUE rips uniformly.
pub fn open_pcm_reader(
    rip: &RipFile,
    track_number: u8,
) -> Result<TrackPcmReader, junk_libs_core::AnalysisError> {
    if let Some(chd) = rip.chd_path.as_ref() {
        return TrackPcmReader::from_chd(chd, track_number);
    }
    if let Some(cue) = rip.cue_path.as_ref() {
        return TrackPcmReader::from_cue(cue, track_number);
    }
    Err(junk_libs_core::AnalysisError::invalid_format(
        "rip_file has neither cue_path nor chd_path",
    ))
}

/// Resolve the on-disk cache dir for asset bytes. Prefers the OS cache dir
/// (via [`env::default_asset_cache_dir`]); falls back to
/// `<library_root>/.cache/assets` on the rare platforms where `dirs::cache_dir`
/// returns `None`.
fn resolve_asset_cache_dir(library_root: &Path) -> PathBuf {
    env::default_asset_cache_dir().unwrap_or_else(|| library_root.join(".cache").join("assets"))
}

/// Pick the front-cover asset, ensure its bytes are locally cached, and
/// return those bytes. Returns `Ok(None)` if the release has no front
/// cover at all — export proceeds without embedded art.
fn resolve_cover(
    ctx: &PhonoContext,
    conn: &Connection,
    assets: &[Asset],
    cache_dir: &Path,
) -> Result<Option<ResolvedArtwork>, ExportError> {
    let Some(asset) = phono_junk_catalog::pick_front_cover(assets) else {
        return Ok(None);
    };
    let bytes = cache_asset_bytes(ctx, conn, asset, cache_dir)?;
    let kind = ArtworkKind::detect(&bytes)
        .ok_or(ExportError::UnsupportedArtwork { asset_id: asset.id })?;
    Ok(Some(ResolvedArtwork { bytes, kind }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArtworkKind {
    Jpeg,
    Png,
    WebP,
}

impl ArtworkKind {
    fn detect(bytes: &[u8]) -> Option<Self> {
        if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            Some(Self::Jpeg)
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            Some(Self::WebP)
        } else {
            None
        }
    }

    fn mime_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::WebP => "image/webp",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::WebP => "webp",
        }
    }
}

struct ResolvedArtwork {
    bytes: Vec<u8>,
    kind: ArtworkKind,
}

/// Read an [`Asset`]'s bytes from the on-disk cache, downloading and
/// persisting them first if necessary.
///
/// Cache hit: `asset.file_path` points at a readable absolute file → bytes
/// are read from disk; `ctx.http` and `conn` are untouched. Cache miss:
/// downloads via `ctx.http.get(source_url)`, writes `<cache_dir>/<id>.<ext>`
/// atomically (`.tmp` + rename), and updates the `assets` row so the
/// absolute path is persisted for the next lookup.
///
/// Used by both the export pipeline (via [`PhonoContext::export_disc`]) and
/// the GUI detail-panel cover-fetch worker — same cache, same invalidation
/// semantics (URL change on the asset row implicitly invalidates).
pub fn cache_asset_bytes(
    ctx: &PhonoContext,
    conn: &Connection,
    asset: &Asset,
    cache_dir: &Path,
) -> Result<Vec<u8>, ExportError> {
    if let Some(path) = asset.file_path.as_ref()
        && path.is_absolute()
        && path.exists()
    {
        return fs::read(path).map_err(|e| ExportError::io(path.clone(), e));
        // Absolute-but-missing, or a legacy library-root-relative path left
        // over from pre-cache-unification DBs: fall through to re-fetch.
    }
    // Validate source_url before touching the HTTP client so a malformed
    // catalog row surfaces a specific error rather than a generic
    // "no HTTP client" in environments where `ctx.http` is absent.
    let url = asset
        .source_url
        .as_deref()
        .ok_or(ExtractPrimitiveError::InvalidTrack(
            "asset has neither file_path nor source_url".into(),
        ))?;
    let parsed_url = url::Url::parse(url).map_err(|_| {
        ExtractPrimitiveError::InvalidTrack("asset source_url is not a valid URL".into())
    })?;
    let http = ctx.http.as_ref().ok_or(ExportError::NoHttpClient)?;
    let resp = http.get(&parsed_url)?;
    if !(200..300).contains(&resp.status) {
        return Err(ExportError::AssetFetchStatus {
            asset_id: asset.id,
            url: url.to_string(),
            status: resp.status,
        });
    }
    let kind = ArtworkKind::detect(&resp.body)
        .ok_or(ExportError::UnsupportedArtwork { asset_id: asset.id })?;
    if let Some(content_type) = resp.content_type.as_deref()
        && !content_type
            .split(';')
            .next()
            .is_some_and(|value| value.trim().eq_ignore_ascii_case(kind.mime_type()))
    {
        log::warn!(
            "asset {} content type {content_type:?} disagrees with detected {}; using detected type",
            asset.id,
            kind.mime_type()
        );
    }
    let ext = kind.extension();
    fs::create_dir_all(cache_dir).map_err(|e| ExportError::io(cache_dir, e))?;
    let filename = format!("{}.{}", asset.id, ext);
    let abs_path = cache_dir.join(&filename);
    let tmp_path = cache_dir.join(format!("{}.{}.tmp", asset.id, ext));
    fs::write(&tmp_path, &resp.body).map_err(|e| ExportError::io(&tmp_path, e))?;
    fs::rename(&tmp_path, &abs_path).map_err(|e| ExportError::io(&abs_path, e))?;

    let mut updated = asset.clone();
    updated.file_path = Some(abs_path);
    crud::update_asset(conn, &updated)?;
    Ok(resp.body)
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
    fn artwork_kind_is_detected_from_bytes() {
        assert_eq!(
            ArtworkKind::detect(b"\xff\xd8\xffrest"),
            Some(ArtworkKind::Jpeg)
        );
        assert_eq!(
            ArtworkKind::detect(b"\x89PNG\r\n\x1a\nrest"),
            Some(ArtworkKind::Png)
        );
        assert_eq!(
            ArtworkKind::detect(b"RIFF1234WEBPrest"),
            Some(ArtworkKind::WebP)
        );
        assert_eq!(ArtworkKind::detect(b"not-an-image"), None);
    }

    fn mk_asset(id: Id, file_path: Option<PathBuf>, source_url: Option<&str>) -> Asset {
        Asset {
            id,
            release_id: 0,
            provider: "test".into(),
            asset_type: phono_junk_catalog::AssetType::FrontCover,
            group_id: None,
            sequence: 0,
            source_url: source_url.map(String::from),
            file_path,
            width: None,
            height: None,
            confidence: None,
            mime_type: None,
            acquired_at: None,
        }
    }

    #[test]
    fn cache_asset_bytes_hit_reads_from_disk_without_http_or_db() {
        let tmp = tempfile::tempdir().unwrap();
        let cached = tmp.path().join("42.jpg");
        fs::write(&cached, b"bytes-on-disk").unwrap();

        let asset = mk_asset(42, Some(cached.clone()), Some("http://example/none"));
        let ctx = PhonoContext::new(); // http = None — proves we don't touch the network
        // An in-memory DB with no schema — a hit must not call update_asset.
        let conn = rusqlite::Connection::open_in_memory().unwrap();

        let bytes = cache_asset_bytes(&ctx, &conn, &asset, tmp.path()).unwrap();
        assert_eq!(bytes, b"bytes-on-disk");
    }

    #[test]
    fn cache_asset_bytes_miss_without_source_url_errors_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        // file_path set but points at a missing file, and no URL to re-fetch.
        let missing = tmp.path().join("99.jpg");
        let asset = mk_asset(99, Some(missing), None);
        let ctx = PhonoContext::new();
        let conn = rusqlite::Connection::open_in_memory().unwrap();

        let err = cache_asset_bytes(&ctx, &conn, &asset, tmp.path()).unwrap_err();
        match err {
            ExportError::Extract(ExtractPrimitiveError::InvalidTrack(msg)) => {
                assert!(msg.contains("source_url"), "unexpected msg: {msg}");
            }
            other => panic!("expected Extract(InvalidTrack), got {other:?}"),
        }
    }
}
