//! CRUD for catalog entities.
//!
//! One function per (operation, entity). Insert returns the new rowid; get
//! returns `Option`; update/delete take an `Id`. Foreign-key cascades are
//! schema-enforced, not duplicated here.
//!
//! JSON-typed columns (`secondary_types_json`, `toc_json`, `bin_paths_json`)
//! serialize via `serde_json` inline. If this file starts reaching for the
//! same pair of helpers more than a handful of times, extract them into
//! `junk-libs-db` per TODO.md's Cross-repo section.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use junk_libs_disc::redumper::{DriveInfo, Ripper};
use phono_junk_catalog::{
    Album, Asset, AssetType, CatalogEntityKey, Disagreement, Disc, Id, IdentifyAttemptError,
    LibraryFolder, Override, Release, RipFile, RipperProvenance, Track,
};
use phono_junk_core::{IdentificationConfidence, IdentificationSource, IdentificationState, Toc};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Serialize, de::DeserializeOwned};
use url::Url;
use uuid::Uuid;

use crate::DbError;

// ---------------------------------------------------------------------------
// JSON helpers (inline — extract if call-site count grows past a handful).
// ---------------------------------------------------------------------------

fn json_write<T: Serialize + ?Sized>(value: &T) -> Result<String, DbError> {
    serde_json::to_string(value).map_err(|e| DbError::Migration(format!("json encode: {e}")))
}

fn json_read<T: DeserializeOwned>(s: &str) -> Result<T, DbError> {
    serde_json::from_str(s).map_err(|e| DbError::Migration(format!("json decode: {e}")))
}

// ---------------------------------------------------------------------------
// IdentificationConfidence / IdentificationSource DB encoding.
// Confidence is a plain unit-variant string. Source may carry `Other(String)`
// so it round-trips as serde_json (`"MusicBrainz"` or `{"Other":"foo"}`).
// ---------------------------------------------------------------------------

fn confidence_to_str(c: IdentificationConfidence) -> &'static str {
    match c {
        IdentificationConfidence::Certain => "Certain",
        IdentificationConfidence::Likely => "Likely",
        IdentificationConfidence::Manual => "Manual",
        IdentificationConfidence::Unidentified => "Unidentified",
    }
}

fn confidence_from_str(s: &str) -> Result<IdentificationConfidence, DbError> {
    Ok(match s {
        "Certain" => IdentificationConfidence::Certain,
        "Likely" => IdentificationConfidence::Likely,
        "Manual" => IdentificationConfidence::Manual,
        "Unidentified" => IdentificationConfidence::Unidentified,
        other => {
            return Err(DbError::Migration(format!(
                "unknown identification confidence: {other}"
            )));
        }
    })
}

fn source_to_str(s: &IdentificationSource) -> Result<String, DbError> {
    json_write(s)
}

fn source_from_str(s: &str) -> Result<IdentificationSource, DbError> {
    json_read(s)
}

// ---------------------------------------------------------------------------
// PathBuf <-> TEXT conversion. UTF-8 paths only; non-UTF-8 is rejected rather
// than silently corrupting the DB.
// ---------------------------------------------------------------------------

fn path_to_string(p: &Path) -> Result<String, DbError> {
    p.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| DbError::Migration(format!("non-UTF-8 path: {}", p.display())))
}

fn opt_path_to_string(p: &Option<PathBuf>) -> Result<Option<String>, DbError> {
    p.as_deref().map(path_to_string).transpose()
}

fn fresh_key(kind: &str) -> String {
    format!("{kind}:local:{}", Uuid::new_v4())
}

fn normalize_source_url(source_url: &str) -> String {
    let Ok(mut url) = Url::parse(source_url) else {
        return source_url.to_string();
    };
    url.set_fragment(None);
    if url.path().is_empty() {
        url.set_path("/");
    }
    url.to_string()
}

fn entity_table(entity_type: &str) -> Result<&'static str, DbError> {
    match entity_type.to_ascii_lowercase().as_str() {
        "album" => Ok("albums"),
        "release" => Ok("releases"),
        "disc" => Ok("discs"),
        "track" => Ok("tracks"),
        "asset" => Ok("assets"),
        "ripfile" | "rip_file" => Ok("rip_files"),
        other => Err(DbError::Migration(format!(
            "unsupported stable-key entity type: {other}"
        ))),
    }
}

fn assign_entity_key(
    conn: &Connection,
    entity_type: &str,
    row_id: Id,
    preferred: Option<String>,
) -> Result<String, DbError> {
    let table = entity_table(entity_type)?;
    let existing: Option<String> = conn
        .query_row(
            &format!("SELECT stable_key FROM {table} WHERE id = ?1"),
            [row_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    if let Some(existing) = existing {
        return Ok(existing);
    }

    let key = preferred.unwrap_or_else(|| fresh_key(&entity_type.to_ascii_lowercase()));
    let updated = conn.execute(
        &format!("UPDATE {table} SET stable_key = ?1 WHERE id = ?2"),
        params![key, row_id],
    )?;
    if updated == 0 {
        return Err(DbError::Migration(format!(
            "cannot assign a stable key to missing {entity_type} row {row_id}"
        )));
    }
    Ok(key)
}

pub fn catalog_entity_key(
    conn: &Connection,
    entity_type: &str,
    row_id: Id,
) -> Result<String, DbError> {
    assign_entity_key(conn, entity_type, row_id, None)
}

fn authority_key(
    conn: &Connection,
    entity_type: &str,
    row_id: Id,
    explicit: Option<&CatalogEntityKey>,
) -> Result<String, DbError> {
    if let Some(explicit) = explicit {
        if !explicit.kind().eq_ignore_ascii_case(entity_type) {
            return Err(DbError::Migration(format!(
                "authority key kind {} does not match entity type {entity_type}",
                explicit.kind()
            )));
        }
        return Ok(explicit.value().to_string());
    }
    catalog_entity_key(conn, entity_type, row_id)
}

pub fn set_catalog_entity_key(
    conn: &Connection,
    entity_type: &str,
    row_id: Id,
    key: &str,
) -> Result<(), DbError> {
    let table = entity_table(entity_type)?;
    let updated = conn.execute(
        &format!("UPDATE {table} SET stable_key = ?1 WHERE id = ?2"),
        params![key, row_id],
    )?;
    if updated == 0 {
        return Err(DbError::Migration(format!(
            "cannot set a stable key on missing {entity_type} row {row_id}"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Album
// ---------------------------------------------------------------------------

pub(crate) fn row_to_album(row: &Row) -> rusqlite::Result<Album> {
    let secondary_types_json: Option<String> = row.get("secondary_types_json")?;
    let secondary_types: Vec<String> = secondary_types_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?
        .unwrap_or_default();
    Ok(Album {
        id: row.get("id")?,
        title: row.get("title")?,
        sort_title: row.get("sort_title")?,
        artist_credit: row.get("artist_credit")?,
        year: row.get::<_, Option<i64>>("year")?.map(|y| y as u16),
        mbid: row.get("mbid")?,
        primary_type: row.get("primary_type")?,
        secondary_types,
        first_release_date: row.get("first_release_date")?,
    })
}

pub fn insert_album(conn: &Connection, album: &Album) -> Result<Id, DbError> {
    let secondary_json = if album.secondary_types.is_empty() {
        None
    } else {
        Some(json_write(&album.secondary_types)?)
    };
    conn.execute(
        "INSERT INTO albums (title, sort_title, artist_credit, year, mbid,
                             primary_type, secondary_types_json, first_release_date)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            album.title,
            album.sort_title,
            album.artist_credit,
            album.year.map(|y| y as i64),
            album.mbid,
            album.primary_type,
            secondary_json,
            album.first_release_date,
        ],
    )?;
    let id = conn.last_insert_rowid();
    let preferred = album
        .mbid
        .as_ref()
        .map(|mbid| format!("album:musicbrainz-release-group:{mbid}"));
    assign_entity_key(conn, "album", id, preferred)?;
    Ok(id)
}

pub fn get_album(conn: &Connection, id: Id) -> Result<Option<Album>, DbError> {
    Ok(conn
        .query_row(
            "SELECT id, title, sort_title, artist_credit, year, mbid,
                    primary_type, secondary_types_json, first_release_date
             FROM albums WHERE id = ?1",
            [id],
            row_to_album,
        )
        .optional()?)
}

pub fn update_album(conn: &Connection, album: &Album) -> Result<(), DbError> {
    let secondary_json = if album.secondary_types.is_empty() {
        None
    } else {
        Some(json_write(&album.secondary_types)?)
    };
    conn.execute(
        "UPDATE albums SET title = ?1, sort_title = ?2, artist_credit = ?3, year = ?4,
                           mbid = ?5, primary_type = ?6, secondary_types_json = ?7,
                           first_release_date = ?8
         WHERE id = ?9",
        params![
            album.title,
            album.sort_title,
            album.artist_credit,
            album.year.map(|y| y as i64),
            album.mbid,
            album.primary_type,
            secondary_json,
            album.first_release_date,
            album.id,
        ],
    )?;
    Ok(())
}

pub fn delete_album(conn: &Connection, id: Id) -> Result<(), DbError> {
    conn.execute("DELETE FROM albums WHERE id = ?1", [id])?;
    Ok(())
}

pub fn list_albums(conn: &Connection) -> Result<Vec<Album>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, title, sort_title, artist_credit, year, mbid,
                primary_type, secondary_types_json, first_release_date
         FROM albums ORDER BY id",
    )?;
    let rows = stmt.query_map([], row_to_album)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------------------------------------------------------------------------
// Release
// ---------------------------------------------------------------------------

const RELEASE_COLS: &str =
    "id, album_id, country, date, label, catalog_number, barcode, mbid, status, language, script";

pub(crate) fn row_to_release(row: &Row) -> rusqlite::Result<Release> {
    Ok(Release {
        id: row.get("id")?,
        album_id: row.get("album_id")?,
        country: row.get("country")?,
        date: row.get("date")?,
        label: row.get("label")?,
        catalog_number: row.get("catalog_number")?,
        barcode: row.get("barcode")?,
        mbid: row.get("mbid")?,
        status: row.get("status")?,
        language: row.get("language")?,
        script: row.get("script")?,
    })
}

pub fn insert_release(conn: &Connection, release: &Release) -> Result<Id, DbError> {
    conn.execute(
        "INSERT INTO releases (album_id, country, date, label, catalog_number,
                               barcode, mbid, status, language, script)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            release.album_id,
            release.country,
            release.date,
            release.label,
            release.catalog_number,
            release.barcode,
            release.mbid,
            release.status,
            release.language,
            release.script,
        ],
    )?;
    let id = conn.last_insert_rowid();
    let preferred = release
        .mbid
        .as_ref()
        .map(|mbid| format!("release:musicbrainz:{mbid}"));
    assign_entity_key(conn, "release", id, preferred)?;
    Ok(id)
}

pub fn get_release(conn: &Connection, id: Id) -> Result<Option<Release>, DbError> {
    let sql = format!("SELECT {RELEASE_COLS} FROM releases WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], row_to_release).optional()?)
}

pub fn find_release_by_stable_key(
    conn: &Connection,
    stable_key: &str,
) -> Result<Option<Release>, DbError> {
    let sql = format!("SELECT {RELEASE_COLS} FROM releases WHERE stable_key = ?1");
    Ok(conn
        .query_row(&sql, [stable_key], row_to_release)
        .optional()?)
}

pub fn update_release(conn: &Connection, release: &Release) -> Result<(), DbError> {
    conn.execute(
        "UPDATE releases SET album_id = ?1, country = ?2, date = ?3, label = ?4,
                             catalog_number = ?5, barcode = ?6, mbid = ?7, status = ?8,
                             language = ?9, script = ?10
         WHERE id = ?11",
        params![
            release.album_id,
            release.country,
            release.date,
            release.label,
            release.catalog_number,
            release.barcode,
            release.mbid,
            release.status,
            release.language,
            release.script,
            release.id,
        ],
    )?;
    Ok(())
}

pub fn delete_release(conn: &Connection, id: Id) -> Result<(), DbError> {
    conn.execute("DELETE FROM releases WHERE id = ?1", [id])?;
    Ok(())
}

pub fn list_releases_for_album(conn: &Connection, album_id: Id) -> Result<Vec<Release>, DbError> {
    let sql = format!("SELECT {RELEASE_COLS} FROM releases WHERE album_id = ?1 ORDER BY id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([album_id], row_to_release)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------------------------------------------------------------------------
// Disc
// ---------------------------------------------------------------------------

const DISC_COLS: &str = "id, release_id, disc_number, format, toc_json, \
     mb_discid, cddb_id, ar_discid1, ar_discid2, mcn";

pub(crate) fn row_to_disc(row: &Row) -> rusqlite::Result<Disc> {
    let toc_json: Option<String> = row.get("toc_json")?;
    let toc: Option<Toc> = toc_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let disc_number: i64 = row.get("disc_number")?;
    Ok(Disc {
        id: row.get("id")?,
        release_id: row.get("release_id")?,
        disc_number: disc_number as u8,
        format: row.get("format")?,
        toc,
        mb_discid: row.get("mb_discid")?,
        cddb_id: row.get("cddb_id")?,
        ar_discid1: row.get("ar_discid1")?,
        ar_discid2: row.get("ar_discid2")?,
        mcn: row.get("mcn")?,
    })
}

pub fn insert_disc(conn: &Connection, disc: &Disc) -> Result<Id, DbError> {
    let toc_json = disc.toc.as_ref().map(json_write).transpose()?;
    conn.execute(
        "INSERT INTO discs (release_id, disc_number, format, toc_json,
                            mb_discid, cddb_id, ar_discid1, ar_discid2, mcn)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            disc.release_id,
            disc.disc_number as i64,
            disc.format,
            toc_json,
            disc.mb_discid,
            disc.cddb_id,
            disc.ar_discid1,
            disc.ar_discid2,
            disc.mcn,
        ],
    )?;
    let id = conn.last_insert_rowid();
    let preferred = match (
        disc.ar_discid1.as_deref(),
        disc.ar_discid2.as_deref(),
        disc.cddb_id.as_deref(),
    ) {
        (Some(id1), Some(id2), Some(cddb)) => Some(format!("disc:accuraterip:{id1}:{id2}:{cddb}")),
        _ => disc
            .mb_discid
            .as_ref()
            .map(|mbid| format!("disc:musicbrainz:{mbid}")),
    };
    assign_entity_key(conn, "disc", id, preferred)?;
    Ok(id)
}

pub fn get_disc(conn: &Connection, id: Id) -> Result<Option<Disc>, DbError> {
    let sql = format!("SELECT {DISC_COLS} FROM discs WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], row_to_disc).optional()?)
}

pub fn update_disc(conn: &Connection, disc: &Disc) -> Result<(), DbError> {
    let toc_json = disc.toc.as_ref().map(json_write).transpose()?;
    conn.execute(
        "UPDATE discs SET release_id = ?1, disc_number = ?2, format = ?3, toc_json = ?4,
                          mb_discid = ?5, cddb_id = ?6, ar_discid1 = ?7, ar_discid2 = ?8,
                          mcn = ?9
         WHERE id = ?10",
        params![
            disc.release_id,
            disc.disc_number as i64,
            disc.format,
            toc_json,
            disc.mb_discid,
            disc.cddb_id,
            disc.ar_discid1,
            disc.ar_discid2,
            disc.mcn,
            disc.id,
        ],
    )?;
    Ok(())
}

pub fn delete_disc(conn: &Connection, id: Id) -> Result<(), DbError> {
    conn.execute("DELETE FROM discs WHERE id = ?1", [id])?;
    Ok(())
}

pub fn list_discs_for_release(conn: &Connection, release_id: Id) -> Result<Vec<Disc>, DbError> {
    let sql =
        format!("SELECT {DISC_COLS} FROM discs WHERE release_id = ?1 ORDER BY disc_number, id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([release_id], row_to_disc)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn find_disc_by_mb_discid(conn: &Connection, mb_discid: &str) -> Result<Option<Disc>, DbError> {
    let sql = format!("SELECT {DISC_COLS} FROM discs WHERE mb_discid = ?1 LIMIT 1");
    Ok(conn.query_row(&sql, [mb_discid], row_to_disc).optional()?)
}

pub fn find_disc_by_ar_triple(
    conn: &Connection,
    ar_discid1: &str,
    ar_discid2: &str,
    cddb_id: &str,
) -> Result<Option<Disc>, DbError> {
    let sql = format!(
        "SELECT {DISC_COLS} FROM discs
         WHERE ar_discid1 = ?1 AND ar_discid2 = ?2 AND cddb_id = ?3
         LIMIT 1"
    );
    Ok(conn
        .query_row(&sql, params![ar_discid1, ar_discid2, cddb_id], row_to_disc)
        .optional()?)
}

// ---------------------------------------------------------------------------
// Track
// ---------------------------------------------------------------------------

pub(crate) fn row_to_track(row: &Row) -> rusqlite::Result<Track> {
    let position: i64 = row.get("position")?;
    Ok(Track {
        id: row.get("id")?,
        disc_id: row.get("disc_id")?,
        position: position as u8,
        title: row.get("title")?,
        artist_credit: row.get("artist_credit")?,
        length_frames: row
            .get::<_, Option<i64>>("length_frames")?
            .map(|v| v as u64),
        isrc: row.get("isrc")?,
        mbid: row.get("mbid")?,
        recording_mbid: row.get("recording_mbid")?,
    })
}

pub fn insert_track(conn: &Connection, track: &Track) -> Result<Id, DbError> {
    conn.execute(
        "INSERT INTO tracks (disc_id, position, title, artist_credit,
                             length_frames, isrc, mbid, recording_mbid)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            track.disc_id,
            track.position as i64,
            track.title,
            track.artist_credit,
            track.length_frames.map(|v| v as i64),
            track.isrc,
            track.mbid,
            track.recording_mbid,
        ],
    )?;
    let id = conn.last_insert_rowid();
    let disc_key = catalog_entity_key(conn, "disc", track.disc_id)?;
    assign_entity_key(
        conn,
        "track",
        id,
        Some(format!("track:{disc_key}:{}", track.position)),
    )?;
    Ok(id)
}

pub fn get_track(conn: &Connection, id: Id) -> Result<Option<Track>, DbError> {
    Ok(conn
        .query_row(
            "SELECT id, disc_id, position, title, artist_credit,
                    length_frames, isrc, mbid, recording_mbid
             FROM tracks WHERE id = ?1",
            [id],
            row_to_track,
        )
        .optional()?)
}

pub fn update_track(conn: &Connection, track: &Track) -> Result<(), DbError> {
    conn.execute(
        "UPDATE tracks SET disc_id = ?1, position = ?2, title = ?3, artist_credit = ?4,
                           length_frames = ?5, isrc = ?6, mbid = ?7, recording_mbid = ?8
         WHERE id = ?9",
        params![
            track.disc_id,
            track.position as i64,
            track.title,
            track.artist_credit,
            track.length_frames.map(|v| v as i64),
            track.isrc,
            track.mbid,
            track.recording_mbid,
            track.id,
        ],
    )?;
    Ok(())
}

pub fn delete_track(conn: &Connection, id: Id) -> Result<(), DbError> {
    conn.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
    Ok(())
}

pub fn list_tracks_for_disc(conn: &Connection, disc_id: Id) -> Result<Vec<Track>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, disc_id, position, title, artist_credit,
                length_frames, isrc, mbid, recording_mbid
         FROM tracks WHERE disc_id = ?1 ORDER BY position",
    )?;
    let rows = stmt.query_map([disc_id], row_to_track)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------------------------------------------------------------------------
// RipFile
// ---------------------------------------------------------------------------

/// Single source of truth for the rip_files SELECT column list — keeps the
/// five callers (get/find_by_cue/find_by_chd/find_for_disc/list_unidentified)
/// from drifting when fields are added.
pub(crate) const RIP_FILE_COLUMNS: &str = "id, disc_id, cue_path, chd_path, bin_paths_json, mtime, size, \
     identification_confidence, identification_source, \
     (SELECT vr.status FROM verification_runs vr WHERE vr.rip_file_id = rip_files.id \
      AND vr.finished_at IS NOT NULL ORDER BY vr.id DESC LIMIT 1) AS accuraterip_status, \
     (SELECT vr.finished_at FROM verification_runs vr WHERE vr.rip_file_id = rip_files.id \
      AND vr.finished_at IS NOT NULL ORDER BY vr.id DESC LIMIT 1) AS last_verified_at, \
     (SELECT vr.chosen_sample_shift FROM verification_runs vr WHERE vr.rip_file_id = rip_files.id \
      AND vr.finished_at IS NOT NULL ORDER BY vr.id DESC LIMIT 1) AS inferred_sample_shift, \
     last_identify_errors, last_identify_at, \
     identification_state, last_state_change_at";

pub(crate) fn row_to_rip_file(row: &Row) -> rusqlite::Result<RipFile> {
    let bin_paths_json: String = row.get("bin_paths_json")?;
    let bin_paths: Vec<PathBuf> = serde_json::from_str(&bin_paths_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let confidence_str: String = row.get("identification_confidence")?;
    let confidence = confidence_from_str(&confidence_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e.to_string(),
            )),
        )
    })?;
    let source_str: Option<String> = row.get("identification_source")?;
    let identification_source = source_str
        .as_deref()
        .map(source_from_str)
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    e.to_string(),
                )),
            )
        })?;
    let errors_str: Option<String> = row.get("last_identify_errors")?;
    let last_identify_errors = errors_str
        .as_deref()
        .map(serde_json::from_str::<Vec<IdentifyAttemptError>>)
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let state_str: String = row.get("identification_state")?;
    let identification_state = IdentificationState::from_str_db(&state_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown identification_state: {state_str}"),
            )),
        )
    })?;
    Ok(RipFile {
        id: row.get("id")?,
        disc_id: row.get("disc_id")?,
        cue_path: row.get::<_, Option<String>>("cue_path")?.map(PathBuf::from),
        chd_path: row.get::<_, Option<String>>("chd_path")?.map(PathBuf::from),
        bin_paths,
        mtime: row.get("mtime")?,
        size: row.get::<_, Option<i64>>("size")?.map(|v| v as u64),
        identification_confidence: confidence,
        identification_source,
        accuraterip_status: row.get("accuraterip_status")?,
        last_verified_at: row.get("last_verified_at")?,
        inferred_sample_shift: row.get("inferred_sample_shift")?,
        last_identify_errors,
        last_identify_at: row.get("last_identify_at")?,
        identification_state,
        last_state_change_at: row.get("last_state_change_at")?,
        // Provenance loads lazily via load_provenance below; keep it out
        // of the join so callers who don't need it don't pay the read.
        provenance: None,
    })
}

pub(crate) fn row_to_provenance(row: &Row) -> rusqlite::Result<RipperProvenance> {
    let ripper_str: String = row.get("ripper")?;
    let drive_json: Option<String> = row.get("drive_json")?;
    let drive: Option<DriveInfo> = drive_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
    let rip_date_str: Option<String> = row.get("rip_date")?;
    let rip_date: Option<DateTime<Utc>> = rip_date_str
        .as_deref()
        .map(|s| s.parse::<DateTime<Utc>>())
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    e.to_string(),
                )),
            )
        })?;
    let log_path: String = row.get("log_path")?;
    Ok(RipperProvenance {
        ripper: Ripper::from_str(&ripper_str),
        version: row.get("version")?,
        drive,
        read_offset: row.get::<_, Option<i64>>("read_offset")?.map(|v| v as i32),
        log_path: PathBuf::from(log_path),
        rip_date,
    })
}

pub fn load_rip_file_provenance(
    conn: &Connection,
    rip_file_id: Id,
) -> Result<Option<RipperProvenance>, DbError> {
    load_provenance(conn, rip_file_id)
}

fn load_provenance(
    conn: &Connection,
    rip_file_id: Id,
) -> Result<Option<RipperProvenance>, DbError> {
    Ok(conn
        .query_row(
            "SELECT ripper, version, drive_json, read_offset, log_path, rip_date
             FROM rip_file_provenance WHERE rip_file_id = ?1",
            [rip_file_id],
            row_to_provenance,
        )
        .optional()?)
}

/// Insert or replace the provenance row for `rip_file_id`. Called by
/// insert/update_rip_file; also exposed for targeted re-stamping from
/// the scan pipeline when a new sidecar is detected on an existing rip.
pub fn upsert_rip_file_provenance(
    conn: &Connection,
    rip_file_id: Id,
    prov: &RipperProvenance,
) -> Result<(), DbError> {
    let drive_json = prov.drive.as_ref().map(json_write).transpose()?;
    let log_path_str = path_to_string(&prov.log_path)?;
    let rip_date_str = prov.rip_date.map(|d| d.to_rfc3339());
    conn.execute(
        "INSERT OR REPLACE INTO rip_file_provenance
            (rip_file_id, ripper, version, drive_json, read_offset, log_path, rip_date)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            rip_file_id,
            prov.ripper.as_str(),
            prov.version,
            drive_json,
            prov.read_offset.map(i64::from),
            log_path_str,
            rip_date_str,
        ],
    )?;
    Ok(())
}

pub fn delete_rip_file_provenance(conn: &Connection, rip_file_id: Id) -> Result<(), DbError> {
    conn.execute(
        "DELETE FROM rip_file_provenance WHERE rip_file_id = ?1",
        [rip_file_id],
    )?;
    Ok(())
}

pub fn insert_rip_file(conn: &Connection, file: &RipFile) -> Result<Id, DbError> {
    let bin_paths: Vec<String> = file
        .bin_paths
        .iter()
        .map(|p| path_to_string(p))
        .collect::<Result<_, _>>()?;
    let bin_json = json_write(&bin_paths)?;
    let cue = opt_path_to_string(&file.cue_path)?;
    let chd = opt_path_to_string(&file.chd_path)?;
    let source = file
        .identification_source
        .as_ref()
        .map(source_to_str)
        .transpose()?;
    let errors_json = file
        .last_identify_errors
        .as_ref()
        .map(json_write)
        .transpose()?;
    conn.execute(
        "INSERT INTO rip_files (disc_id, cue_path, chd_path, bin_paths_json,
                                mtime, size, identification_confidence,
                                identification_source, last_identify_errors,
                                last_identify_at, identification_state,
                                last_state_change_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            file.disc_id,
            cue,
            chd,
            bin_json,
            file.mtime,
            file.size.map(|v| v as i64),
            confidence_to_str(file.identification_confidence),
            source,
            errors_json,
            file.last_identify_at,
            file.identification_state.as_str(),
            file.last_state_change_at,
        ],
    )?;
    let id = conn.last_insert_rowid();
    assign_entity_key(conn, "rip_file", id, None)?;
    if let Some(prov) = &file.provenance {
        upsert_rip_file_provenance(conn, id, prov)?;
    }
    Ok(id)
}

pub fn get_rip_file(conn: &Connection, id: Id) -> Result<Option<RipFile>, DbError> {
    let sql = format!("SELECT {RIP_FILE_COLUMNS} FROM rip_files WHERE id = ?1");
    let mut file = match conn.query_row(&sql, [id], row_to_rip_file).optional()? {
        Some(f) => f,
        None => return Ok(None),
    };
    file.provenance = load_provenance(conn, file.id)?;
    Ok(Some(file))
}

pub fn update_rip_file(conn: &Connection, file: &RipFile) -> Result<(), DbError> {
    let bin_paths: Vec<String> = file
        .bin_paths
        .iter()
        .map(|p| path_to_string(p))
        .collect::<Result<_, _>>()?;
    let bin_json = json_write(&bin_paths)?;
    let cue = opt_path_to_string(&file.cue_path)?;
    let chd = opt_path_to_string(&file.chd_path)?;
    let source = file
        .identification_source
        .as_ref()
        .map(source_to_str)
        .transpose()?;
    let errors_json = file
        .last_identify_errors
        .as_ref()
        .map(json_write)
        .transpose()?;
    conn.execute(
        "UPDATE rip_files SET disc_id = ?1, cue_path = ?2, chd_path = ?3,
                              bin_paths_json = ?4, mtime = ?5, size = ?6,
                              identification_confidence = ?7, identification_source = ?8,
                              last_identify_errors = ?9, last_identify_at = ?10,
                              identification_state = ?11, last_state_change_at = ?12
         WHERE id = ?13",
        params![
            file.disc_id,
            cue,
            chd,
            bin_json,
            file.mtime,
            file.size.map(|v| v as i64),
            confidence_to_str(file.identification_confidence),
            source,
            errors_json,
            file.last_identify_at,
            file.identification_state.as_str(),
            file.last_state_change_at,
            file.id,
        ],
    )?;
    match &file.provenance {
        Some(prov) => upsert_rip_file_provenance(conn, file.id, prov)?,
        None => delete_rip_file_provenance(conn, file.id)?,
    }
    Ok(())
}

pub fn delete_rip_file(conn: &Connection, id: Id) -> Result<(), DbError> {
    conn.execute("DELETE FROM rip_files WHERE id = ?1", [id])?;
    Ok(())
}

pub fn find_rip_file_by_cue_path(
    conn: &Connection,
    cue_path: &Path,
) -> Result<Option<RipFile>, DbError> {
    let path_str = path_to_string(cue_path)?;
    let sql = format!("SELECT {RIP_FILE_COLUMNS} FROM rip_files WHERE cue_path = ?1 LIMIT 1");
    let mut file = match conn
        .query_row(&sql, [path_str], row_to_rip_file)
        .optional()?
    {
        Some(f) => f,
        None => return Ok(None),
    };
    file.provenance = load_provenance(conn, file.id)?;
    Ok(Some(file))
}

pub fn find_rip_file_by_chd_path(
    conn: &Connection,
    chd_path: &Path,
) -> Result<Option<RipFile>, DbError> {
    let path_str = path_to_string(chd_path)?;
    let sql = format!("SELECT {RIP_FILE_COLUMNS} FROM rip_files WHERE chd_path = ?1 LIMIT 1");
    let mut file = match conn
        .query_row(&sql, [path_str], row_to_rip_file)
        .optional()?
    {
        Some(f) => f,
        None => return Ok(None),
    };
    file.provenance = load_provenance(conn, file.id)?;
    Ok(Some(file))
}

/// Return the first `RipFile` linked to `disc_id`, if any.
///
/// Sprint 12's export path consumes this to locate the BIN/CUE or CHD that
/// backs a catalog disc. Many-to-one is tolerated (e.g. a future re-rip
/// attached to the same disc) but we take the earliest by id so behaviour
/// is stable across re-runs.
pub fn find_rip_file_for_disc(conn: &Connection, disc_id: Id) -> Result<Option<RipFile>, DbError> {
    let sql =
        format!("SELECT {RIP_FILE_COLUMNS} FROM rip_files WHERE disc_id = ?1 ORDER BY id LIMIT 1");
    let mut file = match conn
        .query_row(&sql, [disc_id], row_to_rip_file)
        .optional()?
    {
        Some(f) => f,
        None => return Ok(None),
    };
    file.provenance = load_provenance(conn, file.id)?;
    Ok(Some(file))
}

pub fn list_all_rip_files(conn: &Connection) -> Result<Vec<RipFile>, DbError> {
    let sql = format!("SELECT {RIP_FILE_COLUMNS} FROM rip_files ORDER BY id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_rip_file)?;
    let mut files: Vec<RipFile> = rows.collect::<rusqlite::Result<_>>()?;
    for f in &mut files {
        f.provenance = load_provenance(conn, f.id)?;
    }
    Ok(files)
}

/// Every rip file that isn't yet in the terminal `Identified` state —
/// includes `Unscanned`, `Queued`, `Working`, `Unidentified`, `Failed`.
/// The GUI renders all of these as "(unidentified)" rows (the Status
/// column differentiates the lifecycle phase). Sprint 26.
pub fn list_unidentified_rip_files(conn: &Connection) -> Result<Vec<RipFile>, DbError> {
    list_rip_files_by_state(
        conn,
        &[
            IdentificationState::Unscanned,
            IdentificationState::Queued,
            IdentificationState::Working,
            IdentificationState::Unidentified,
            IdentificationState::Failed,
        ],
    )
}

/// All rip files in a given lifecycle state (or states). Used by the
/// identify queue drain ("every row that's Queued or Failed") and by the
/// GUI album-list filter.
pub fn list_rip_files_by_state(
    conn: &Connection,
    states: &[IdentificationState],
) -> Result<Vec<RipFile>, DbError> {
    if states.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders: Vec<String> = (1..=states.len()).map(|i| format!("?{i}")).collect();
    let sql = format!(
        "SELECT {RIP_FILE_COLUMNS} FROM rip_files
         WHERE identification_state IN ({})
         ORDER BY id",
        placeholders.join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let state_strs: Vec<&str> = states.iter().map(|s| s.as_str()).collect();
    let params_refs: Vec<&dyn rusqlite::ToSql> = state_strs
        .iter()
        .map(|s| s as &dyn rusqlite::ToSql)
        .collect();
    let rows = stmt.query_map(params_refs.as_slice(), row_to_rip_file)?;
    let mut files: Vec<RipFile> = rows.collect::<rusqlite::Result<_>>()?;
    for f in &mut files {
        f.provenance = load_provenance(conn, f.id)?;
    }
    Ok(files)
}

/// Targeted state transition: writes `identification_state` +
/// `last_state_change_at` without touching any other column. Mirrors the
/// `set_rip_file_identify_attempt` pattern — avoids
/// re-serialising the whole row for a simple lifecycle tick.
pub fn set_rip_file_identification_state(
    conn: &Connection,
    rip_file_id: Id,
    state: IdentificationState,
    now_rfc3339: &str,
) -> Result<(), DbError> {
    conn.execute(
        "UPDATE rip_files SET identification_state = ?1, last_state_change_at = ?2 WHERE id = ?3",
        params![state.as_str(), now_rfc3339, rip_file_id],
    )?;
    Ok(())
}

/// Targeted identify-attempt persistence: writes the per-provider error log
/// (and timestamp) without re-serializing the whole rip_file row. Called from
/// `phono-junk-lib::identify::identify_disc` after every fan-out completes,
/// regardless of whether identification succeeded.
///
/// `errors` may be empty (all providers returned cleanly — useful for an
/// identified disc to show "no errors" in the panel). Pass `None` to clear
/// (e.g. on a fresh re-scan).
pub fn set_rip_file_identify_attempt(
    conn: &Connection,
    rip_file_id: Id,
    errors: Option<&[IdentifyAttemptError]>,
    at: &str,
) -> Result<(), DbError> {
    let errors_json = errors.map(json_write).transpose()?;
    conn.execute(
        "UPDATE rip_files SET last_identify_errors = ?1, last_identify_at = ?2 WHERE id = ?3",
        params![errors_json, at, rip_file_id],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Asset
// ---------------------------------------------------------------------------

pub(crate) fn row_to_asset(row: &Row) -> rusqlite::Result<Asset> {
    let asset_type_str: String = row.get("asset_type")?;
    let sequence: i64 = row.get("sequence")?;
    Ok(Asset {
        id: row.get("id")?,
        release_id: row.get("release_id")?,
        provider: row.get("provider")?,
        asset_type: AssetType::from_db_str(&asset_type_str),
        group_id: row.get("group_id")?,
        sequence: sequence as u16,
        source_url: row.get("source_url")?,
        file_path: row
            .get::<_, Option<String>>("file_path")?
            .map(PathBuf::from),
        width: row
            .get::<_, Option<i64>>("width")?
            .map(|value| value as u32),
        height: row
            .get::<_, Option<i64>>("height")?
            .map(|value| value as u32),
        confidence: row.get("confidence")?,
        mime_type: row.get("mime_type")?,
        acquired_at: row.get("acquired_at")?,
    })
}

pub fn insert_asset(conn: &Connection, asset: &Asset) -> Result<Id, DbError> {
    let file_path = opt_path_to_string(&asset.file_path)?;
    conn.execute(
        "INSERT INTO assets (release_id, provider, asset_type, group_id, sequence,
                             source_url, file_path, width, height, confidence,
                             mime_type, acquired_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            asset.release_id,
            asset.provider,
            asset.asset_type.as_db_str(),
            asset.group_id,
            asset.sequence as i64,
            asset.source_url,
            file_path,
            asset.width.map(i64::from),
            asset.height.map(i64::from),
            asset.confidence,
            asset.mime_type,
            asset.acquired_at,
        ],
    )?;
    let id = conn.last_insert_rowid();
    let preferred = asset.source_url.as_ref().map(|url| {
        format!(
            "asset:unknown:{}:{}",
            asset.asset_type.as_db_str(),
            normalize_source_url(url)
        )
    });
    assign_entity_key(conn, "asset", id, preferred)?;
    Ok(id)
}

/// Upsert an asset observation without replacing its stable projection row.
/// Provider provenance and image facts are storage inputs so this crate does
/// not depend on provider traits.
#[allow(clippy::too_many_arguments)]
pub fn upsert_asset_evidence(
    conn: &Connection,
    release_id: Id,
    provider: &str,
    asset_type: &AssetType,
    source_url: &str,
    width: Option<u32>,
    height: Option<u32>,
    confidence: &str,
    mime_type: Option<&str>,
    acquired_at: &str,
) -> Result<Id, DbError> {
    let source_url = normalize_source_url(source_url);
    let stable_key = format!("asset:{provider}:{}:{source_url}", asset_type.as_db_str());
    conn.execute(
        "INSERT INTO assets
            (stable_key, release_id, provider, asset_type, source_url, width,
             height, confidence, mime_type, acquired_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(stable_key) DO UPDATE SET
             release_id = excluded.release_id,
             width = COALESCE(excluded.width, assets.width),
             height = COALESCE(excluded.height, assets.height),
             confidence = excluded.confidence,
             mime_type = COALESCE(excluded.mime_type, assets.mime_type),
             acquired_at = excluded.acquired_at",
        params![
            stable_key,
            release_id,
            provider,
            asset_type.as_db_str(),
            source_url,
            width.map(i64::from),
            height.map(i64::from),
            confidence,
            mime_type,
            acquired_at
        ],
    )?;
    conn.query_row(
        "SELECT id FROM assets WHERE stable_key = ?1",
        [stable_key],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn get_asset(conn: &Connection, id: Id) -> Result<Option<Asset>, DbError> {
    Ok(conn
        .query_row(
            "SELECT id, release_id, provider, asset_type, group_id, sequence,
                    source_url, file_path, width, height, confidence, mime_type, acquired_at
             FROM assets WHERE id = ?1",
            [id],
            row_to_asset,
        )
        .optional()?)
}

pub fn update_asset(conn: &Connection, asset: &Asset) -> Result<(), DbError> {
    let file_path = opt_path_to_string(&asset.file_path)?;
    conn.execute(
        "UPDATE assets SET release_id = ?1, provider = ?2, asset_type = ?3, group_id = ?4,
                           sequence = ?5, source_url = ?6, file_path = ?7, width = ?8,
                           height = ?9, confidence = ?10, mime_type = ?11, acquired_at = ?12
         WHERE id = ?13",
        params![
            asset.release_id,
            asset.provider,
            asset.asset_type.as_db_str(),
            asset.group_id,
            asset.sequence as i64,
            asset.source_url,
            file_path,
            asset.width.map(i64::from),
            asset.height.map(i64::from),
            asset.confidence,
            asset.mime_type,
            asset.acquired_at,
            asset.id,
        ],
    )?;
    Ok(())
}

pub fn delete_asset(conn: &Connection, id: Id) -> Result<(), DbError> {
    conn.execute("DELETE FROM assets WHERE id = ?1", [id])?;
    Ok(())
}

pub fn list_assets_for_release(conn: &Connection, release_id: Id) -> Result<Vec<Asset>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, release_id, provider, asset_type, group_id, sequence,
                source_url, file_path, width, height, confidence, mime_type, acquired_at
         FROM assets
         WHERE release_id = ?1
         ORDER BY COALESCE(group_id, -1), sequence, id",
    )?;
    let rows = stmt.query_map([release_id], row_to_asset)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------------------------------------------------------------------------
// Disagreement
// ---------------------------------------------------------------------------

pub(crate) fn row_to_disagreement(row: &Row) -> rusqlite::Result<Disagreement> {
    let resolved: i64 = row.get("resolved")?;
    let entity_type: String = row.get("entity_type")?;
    let entity_key = CatalogEntityKey::from_parts(&entity_type, row.get("entity_key")?)
        .ok_or_else(|| rusqlite::Error::InvalidColumnName("entity_type".into()))?;
    Ok(Disagreement {
        id: row.get("id")?,
        entity_type,
        entity_id: row.get("entity_id")?,
        entity_key: Some(entity_key),
        field: row.get("field")?,
        source_a: row.get("source_a")?,
        value_a: row.get("value_a")?,
        source_b: row.get("source_b")?,
        value_b: row.get("value_b")?,
        resolved: resolved != 0,
        created_at: row.get("created_at")?,
    })
}

pub fn insert_disagreement(conn: &Connection, d: &Disagreement) -> Result<Id, DbError> {
    let entity_key = authority_key(conn, &d.entity_type, d.entity_id, d.entity_key.as_ref())?;
    conn.execute(
        "INSERT INTO disagreements (entity_type, entity_id, entity_key, field,
                                    source_a, value_a, source_b, value_b, resolved)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            d.entity_type,
            d.entity_id,
            entity_key,
            d.field,
            d.source_a,
            d.value_a,
            d.source_b,
            d.value_b,
            d.resolved as i64,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_disagreement(conn: &Connection, id: Id) -> Result<Option<Disagreement>, DbError> {
    Ok(conn
        .query_row(
            "SELECT id, entity_type, entity_id, entity_key, field, source_a, value_a,
                    source_b, value_b, resolved, created_at
             FROM disagreements WHERE id = ?1",
            [id],
            row_to_disagreement,
        )
        .optional()?)
}

pub fn update_disagreement(conn: &Connection, d: &Disagreement) -> Result<(), DbError> {
    let entity_key = authority_key(conn, &d.entity_type, d.entity_id, d.entity_key.as_ref())?;
    conn.execute(
        "UPDATE disagreements SET entity_type = ?1, entity_id = ?2, entity_key = ?3, field = ?4,
                                  source_a = ?5, value_a = ?6, source_b = ?7, value_b = ?8,
                                  resolved = ?9
         WHERE id = ?10",
        params![
            d.entity_type,
            d.entity_id,
            entity_key,
            d.field,
            d.source_a,
            d.value_a,
            d.source_b,
            d.value_b,
            d.resolved as i64,
            d.id,
        ],
    )?;
    Ok(())
}

pub fn delete_disagreement(conn: &Connection, id: Id) -> Result<(), DbError> {
    conn.execute("DELETE FROM disagreements WHERE id = ?1", [id])?;
    Ok(())
}

pub fn list_disagreements_for(
    conn: &Connection,
    entity_type: &str,
    entity_id: Id,
) -> Result<Vec<Disagreement>, DbError> {
    let entity_key = catalog_entity_key(conn, entity_type, entity_id)?;
    let mut stmt = conn.prepare(
        "SELECT id, entity_type, entity_id, entity_key, field, source_a, value_a,
                source_b, value_b, resolved, created_at
         FROM disagreements
         WHERE entity_key = ?1
         ORDER BY id",
    )?;
    let rows = stmt.query_map([entity_key], row_to_disagreement)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------------------------------------------------------------------------
// Override
// ---------------------------------------------------------------------------

fn row_to_override(row: &Row) -> rusqlite::Result<Override> {
    let entity_type: String = row.get("entity_type")?;
    let entity_key = CatalogEntityKey::from_parts(&entity_type, row.get("entity_key")?)
        .ok_or_else(|| rusqlite::Error::InvalidColumnName("entity_type".into()))?;
    Ok(Override {
        id: row.get("id")?,
        entity_type,
        entity_id: row.get("entity_id")?,
        entity_key: Some(entity_key),
        sub_path: row.get("sub_path")?,
        field: row.get("field")?,
        override_value: row.get("override_value")?,
        reason: row.get("reason")?,
        created_at: row.get("created_at")?,
    })
}

pub fn insert_override(conn: &Connection, o: &Override) -> Result<Id, DbError> {
    let entity_key = authority_key(conn, &o.entity_type, o.entity_id, o.entity_key.as_ref())?;
    conn.execute(
        "INSERT INTO overrides (entity_type, entity_id, entity_key, sub_path, field,
                                override_value, reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            o.entity_type,
            o.entity_id,
            entity_key,
            o.sub_path,
            o.field,
            o.override_value,
            o.reason,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_override(conn: &Connection, id: Id) -> Result<Option<Override>, DbError> {
    Ok(conn
        .query_row(
            "SELECT id, entity_type, entity_id, entity_key, sub_path, field,
                    override_value, reason, created_at
             FROM overrides WHERE id = ?1",
            [id],
            row_to_override,
        )
        .optional()?)
}

pub fn update_override(conn: &Connection, o: &Override) -> Result<(), DbError> {
    let entity_key = authority_key(conn, &o.entity_type, o.entity_id, o.entity_key.as_ref())?;
    conn.execute(
        "UPDATE overrides SET entity_type = ?1, entity_id = ?2, entity_key = ?3, sub_path = ?4,
                              field = ?5, override_value = ?6, reason = ?7
         WHERE id = ?8",
        params![
            o.entity_type,
            o.entity_id,
            entity_key,
            o.sub_path,
            o.field,
            o.override_value,
            o.reason,
            o.id,
        ],
    )?;
    Ok(())
}

pub fn delete_override(conn: &Connection, id: Id) -> Result<(), DbError> {
    conn.execute("DELETE FROM overrides WHERE id = ?1", [id])?;
    Ok(())
}

pub fn list_overrides_for(
    conn: &Connection,
    entity_type: &str,
    entity_id: Id,
) -> Result<Vec<Override>, DbError> {
    let entity_key = catalog_entity_key(conn, entity_type, entity_id)?;
    let mut stmt = conn.prepare(
        "SELECT id, entity_type, entity_id, entity_key, sub_path, field,
                override_value, reason, created_at
         FROM overrides
         WHERE entity_key = ?1
         ORDER BY id",
    )?;
    let rows = stmt.query_map([entity_key], row_to_override)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------------------------------------------------------------------------
// LibraryFolder
// ---------------------------------------------------------------------------

fn row_to_library_folder(row: &Row) -> rusqlite::Result<LibraryFolder> {
    let path: String = row.get("path")?;
    Ok(LibraryFolder {
        id: row.get("id")?,
        path: PathBuf::from(path),
        added_at: row.get("added_at")?,
    })
}

/// Register a folder as a tracked library root. Idempotent: re-adding a
/// path that's already tracked is a no-op (returns the existing id).
pub fn insert_library_folder(conn: &Connection, path: &Path) -> Result<Id, DbError> {
    let path_str = path_to_string(path)?;
    conn.execute(
        "INSERT OR IGNORE INTO library_folders (path) VALUES (?1)",
        params![path_str],
    )?;
    // `INSERT OR IGNORE` doesn't return last_insert_rowid when the row
    // was a duplicate; read back by path so the caller always gets the
    // canonical id.
    let id: Id = conn.query_row(
        "SELECT id FROM library_folders WHERE path = ?1",
        params![path_str],
        |r| r.get(0),
    )?;
    Ok(id)
}

pub fn list_library_folders(conn: &Connection) -> Result<Vec<LibraryFolder>, DbError> {
    let mut stmt = conn.prepare("SELECT id, path, added_at FROM library_folders ORDER BY id")?;
    let rows = stmt.query_map([], row_to_library_folder)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn delete_library_folder(conn: &Connection, id: Id) -> Result<(), DbError> {
    conn.execute("DELETE FROM library_folders WHERE id = ?1", [id])?;
    Ok(())
}
