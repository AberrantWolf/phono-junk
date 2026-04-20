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

use phono_junk_catalog::{
    Album, Asset, AssetType, Disagreement, Disc, Id, IdentifyAttemptError, Override, Release,
    RipFile, Track,
};
use phono_junk_core::{IdentificationConfidence, IdentificationSource, Toc};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Serialize, de::DeserializeOwned};

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

// ---------------------------------------------------------------------------
// Album
// ---------------------------------------------------------------------------

fn row_to_album(row: &Row) -> rusqlite::Result<Album> {
    let secondary_types_json: Option<String> = row.get("secondary_types_json")?;
    let secondary_types: Vec<String> = secondary_types_json
        .as_deref()
        .map(|s| serde_json::from_str(s))
        .transpose()
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?
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
    Ok(conn.last_insert_rowid())
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

fn row_to_release(row: &Row) -> rusqlite::Result<Release> {
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
    Ok(conn.last_insert_rowid())
}

pub fn get_release(conn: &Connection, id: Id) -> Result<Option<Release>, DbError> {
    let sql = format!("SELECT {RELEASE_COLS} FROM releases WHERE id = ?1");
    Ok(conn
        .query_row(&sql, [id], row_to_release)
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
    let sql = format!(
        "SELECT {RELEASE_COLS} FROM releases WHERE album_id = ?1 ORDER BY id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([album_id], row_to_release)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------------------------------------------------------------------------
// Disc
// ---------------------------------------------------------------------------

fn row_to_disc(row: &Row) -> rusqlite::Result<Disc> {
    let toc_json: Option<String> = row.get("toc_json")?;
    let toc: Option<Toc> = toc_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?;
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
        dbar_raw: row.get("dbar_raw")?,
    })
}

pub fn insert_disc(conn: &Connection, disc: &Disc) -> Result<Id, DbError> {
    let toc_json = disc.toc.as_ref().map(json_write).transpose()?;
    conn.execute(
        "INSERT INTO discs (release_id, disc_number, format, toc_json,
                            mb_discid, cddb_id, ar_discid1, ar_discid2, dbar_raw)
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
            disc.dbar_raw,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_disc(conn: &Connection, id: Id) -> Result<Option<Disc>, DbError> {
    Ok(conn
        .query_row(
            "SELECT id, release_id, disc_number, format, toc_json,
                    mb_discid, cddb_id, ar_discid1, ar_discid2, dbar_raw
             FROM discs WHERE id = ?1",
            [id],
            row_to_disc,
        )
        .optional()?)
}

pub fn update_disc(conn: &Connection, disc: &Disc) -> Result<(), DbError> {
    let toc_json = disc.toc.as_ref().map(json_write).transpose()?;
    conn.execute(
        "UPDATE discs SET release_id = ?1, disc_number = ?2, format = ?3, toc_json = ?4,
                          mb_discid = ?5, cddb_id = ?6, ar_discid1 = ?7, ar_discid2 = ?8,
                          dbar_raw = ?9
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
            disc.dbar_raw,
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
    let mut stmt = conn.prepare(
        "SELECT id, release_id, disc_number, format, toc_json,
                mb_discid, cddb_id, ar_discid1, ar_discid2, dbar_raw
         FROM discs WHERE release_id = ?1 ORDER BY disc_number, id",
    )?;
    let rows = stmt.query_map([release_id], row_to_disc)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn find_disc_by_mb_discid(
    conn: &Connection,
    mb_discid: &str,
) -> Result<Option<Disc>, DbError> {
    Ok(conn
        .query_row(
            "SELECT id, release_id, disc_number, format, toc_json,
                    mb_discid, cddb_id, ar_discid1, ar_discid2, dbar_raw
             FROM discs WHERE mb_discid = ?1 LIMIT 1",
            [mb_discid],
            row_to_disc,
        )
        .optional()?)
}

pub fn find_disc_by_ar_triple(
    conn: &Connection,
    ar_discid1: &str,
    ar_discid2: &str,
    cddb_id: &str,
) -> Result<Option<Disc>, DbError> {
    Ok(conn
        .query_row(
            "SELECT id, release_id, disc_number, format, toc_json,
                    mb_discid, cddb_id, ar_discid1, ar_discid2, dbar_raw
             FROM discs
             WHERE ar_discid1 = ?1 AND ar_discid2 = ?2 AND cddb_id = ?3
             LIMIT 1",
            params![ar_discid1, ar_discid2, cddb_id],
            row_to_disc,
        )
        .optional()?)
}

/// Targeted dBAR persistence: writes raw bytes without re-serializing the
/// whole disc row. Called from the AccurateRip verify path on first fetch so
/// subsequent verifications can skip the network.
pub fn set_disc_dbar_raw(conn: &Connection, disc_id: Id, bytes: &[u8]) -> Result<(), DbError> {
    conn.execute(
        "UPDATE discs SET dbar_raw = ?1 WHERE id = ?2",
        params![bytes, disc_id],
    )?;
    Ok(())
}

pub fn get_disc_dbar_raw(conn: &Connection, disc_id: Id) -> Result<Option<Vec<u8>>, DbError> {
    Ok(conn
        .query_row(
            "SELECT dbar_raw FROM discs WHERE id = ?1",
            [disc_id],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()?
        .flatten())
}

// ---------------------------------------------------------------------------
// Track
// ---------------------------------------------------------------------------

fn row_to_track(row: &Row) -> rusqlite::Result<Track> {
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
    Ok(conn.last_insert_rowid())
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
const RIP_FILE_COLUMNS: &str =
    "id, disc_id, cue_path, chd_path, bin_paths_json, mtime, size, \
     identification_confidence, identification_source, accuraterip_status, \
     last_verified_at, last_identify_errors, last_identify_at";

fn row_to_rip_file(row: &Row) -> rusqlite::Result<RipFile> {
    let bin_paths_json: String = row.get("bin_paths_json")?;
    let bin_paths: Vec<PathBuf> = serde_json::from_str(&bin_paths_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let confidence_str: String = row.get("identification_confidence")?;
    let confidence = confidence_from_str(&confidence_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
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
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
            )
        })?;
    let errors_str: Option<String> = row.get("last_identify_errors")?;
    let last_identify_errors = errors_str
        .as_deref()
        .map(serde_json::from_str::<Vec<IdentifyAttemptError>>)
        .transpose()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(e),
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
        last_identify_errors,
        last_identify_at: row.get("last_identify_at")?,
    })
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
    let source = file.identification_source.as_ref().map(source_to_str).transpose()?;
    let errors_json = file
        .last_identify_errors
        .as_ref()
        .map(json_write)
        .transpose()?;
    conn.execute(
        "INSERT INTO rip_files (disc_id, cue_path, chd_path, bin_paths_json,
                                mtime, size, identification_confidence,
                                identification_source, accuraterip_status,
                                last_verified_at, last_identify_errors,
                                last_identify_at)
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
            file.accuraterip_status,
            file.last_verified_at,
            errors_json,
            file.last_identify_at,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_rip_file(conn: &Connection, id: Id) -> Result<Option<RipFile>, DbError> {
    let sql = format!("SELECT {RIP_FILE_COLUMNS} FROM rip_files WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], row_to_rip_file).optional()?)
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
    let source = file.identification_source.as_ref().map(source_to_str).transpose()?;
    let errors_json = file
        .last_identify_errors
        .as_ref()
        .map(json_write)
        .transpose()?;
    conn.execute(
        "UPDATE rip_files SET disc_id = ?1, cue_path = ?2, chd_path = ?3,
                              bin_paths_json = ?4, mtime = ?5, size = ?6,
                              identification_confidence = ?7, identification_source = ?8,
                              accuraterip_status = ?9, last_verified_at = ?10,
                              last_identify_errors = ?11, last_identify_at = ?12
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
            file.accuraterip_status,
            file.last_verified_at,
            errors_json,
            file.last_identify_at,
            file.id,
        ],
    )?;
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
    Ok(conn.query_row(&sql, [path_str], row_to_rip_file).optional()?)
}

pub fn find_rip_file_by_chd_path(
    conn: &Connection,
    chd_path: &Path,
) -> Result<Option<RipFile>, DbError> {
    let path_str = path_to_string(chd_path)?;
    let sql = format!("SELECT {RIP_FILE_COLUMNS} FROM rip_files WHERE chd_path = ?1 LIMIT 1");
    Ok(conn.query_row(&sql, [path_str], row_to_rip_file).optional()?)
}

/// Return the first `RipFile` linked to `disc_id`, if any.
///
/// Sprint 12's export path consumes this to locate the BIN/CUE or CHD that
/// backs a catalog disc. Many-to-one is tolerated (e.g. a future re-rip
/// attached to the same disc) but we take the earliest by id so behaviour
/// is stable across re-runs.
pub fn find_rip_file_for_disc(
    conn: &Connection,
    disc_id: Id,
) -> Result<Option<RipFile>, DbError> {
    let sql = format!(
        "SELECT {RIP_FILE_COLUMNS} FROM rip_files WHERE disc_id = ?1 ORDER BY id LIMIT 1"
    );
    Ok(conn.query_row(&sql, [disc_id], row_to_rip_file).optional()?)
}

pub fn list_unidentified_rip_files(conn: &Connection) -> Result<Vec<RipFile>, DbError> {
    let sql =
        format!("SELECT {RIP_FILE_COLUMNS} FROM rip_files WHERE disc_id IS NULL ORDER BY id");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], row_to_rip_file)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
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

fn row_to_asset(row: &Row) -> rusqlite::Result<Asset> {
    let asset_type_str: String = row.get("asset_type")?;
    let sequence: i64 = row.get("sequence")?;
    Ok(Asset {
        id: row.get("id")?,
        release_id: row.get("release_id")?,
        asset_type: AssetType::from_db_str(&asset_type_str),
        group_id: row.get("group_id")?,
        sequence: sequence as u16,
        source_url: row.get("source_url")?,
        file_path: row.get::<_, Option<String>>("file_path")?.map(PathBuf::from),
        scraped_at: row.get("scraped_at")?,
    })
}

pub fn insert_asset(conn: &Connection, asset: &Asset) -> Result<Id, DbError> {
    let file_path = opt_path_to_string(&asset.file_path)?;
    conn.execute(
        "INSERT INTO assets (release_id, asset_type, group_id, sequence,
                             source_url, file_path, scraped_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            asset.release_id,
            asset.asset_type.as_db_str(),
            asset.group_id,
            asset.sequence as i64,
            asset.source_url,
            file_path,
            asset.scraped_at,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn get_asset(conn: &Connection, id: Id) -> Result<Option<Asset>, DbError> {
    Ok(conn
        .query_row(
            "SELECT id, release_id, asset_type, group_id, sequence,
                    source_url, file_path, scraped_at
             FROM assets WHERE id = ?1",
            [id],
            row_to_asset,
        )
        .optional()?)
}

pub fn update_asset(conn: &Connection, asset: &Asset) -> Result<(), DbError> {
    let file_path = opt_path_to_string(&asset.file_path)?;
    conn.execute(
        "UPDATE assets SET release_id = ?1, asset_type = ?2, group_id = ?3,
                           sequence = ?4, source_url = ?5, file_path = ?6, scraped_at = ?7
         WHERE id = ?8",
        params![
            asset.release_id,
            asset.asset_type.as_db_str(),
            asset.group_id,
            asset.sequence as i64,
            asset.source_url,
            file_path,
            asset.scraped_at,
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
        "SELECT id, release_id, asset_type, group_id, sequence,
                source_url, file_path, scraped_at
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

fn row_to_disagreement(row: &Row) -> rusqlite::Result<Disagreement> {
    let resolved: i64 = row.get("resolved")?;
    Ok(Disagreement {
        id: row.get("id")?,
        entity_type: row.get("entity_type")?,
        entity_id: row.get("entity_id")?,
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
    conn.execute(
        "INSERT INTO disagreements (entity_type, entity_id, field,
                                    source_a, value_a, source_b, value_b, resolved)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            d.entity_type,
            d.entity_id,
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
            "SELECT id, entity_type, entity_id, field, source_a, value_a,
                    source_b, value_b, resolved, created_at
             FROM disagreements WHERE id = ?1",
            [id],
            row_to_disagreement,
        )
        .optional()?)
}

pub fn update_disagreement(conn: &Connection, d: &Disagreement) -> Result<(), DbError> {
    conn.execute(
        "UPDATE disagreements SET entity_type = ?1, entity_id = ?2, field = ?3,
                                  source_a = ?4, value_a = ?5, source_b = ?6, value_b = ?7,
                                  resolved = ?8
         WHERE id = ?9",
        params![
            d.entity_type,
            d.entity_id,
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
    let mut stmt = conn.prepare(
        "SELECT id, entity_type, entity_id, field, source_a, value_a,
                source_b, value_b, resolved, created_at
         FROM disagreements
         WHERE entity_type = ?1 AND entity_id = ?2
         ORDER BY id",
    )?;
    let rows = stmt.query_map(params![entity_type, entity_id], row_to_disagreement)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------------------------------------------------------------------------
// Override
// ---------------------------------------------------------------------------

fn row_to_override(row: &Row) -> rusqlite::Result<Override> {
    Ok(Override {
        id: row.get("id")?,
        entity_type: row.get("entity_type")?,
        entity_id: row.get("entity_id")?,
        sub_path: row.get("sub_path")?,
        field: row.get("field")?,
        override_value: row.get("override_value")?,
        reason: row.get("reason")?,
        created_at: row.get("created_at")?,
    })
}

pub fn insert_override(conn: &Connection, o: &Override) -> Result<Id, DbError> {
    conn.execute(
        "INSERT INTO overrides (entity_type, entity_id, sub_path, field,
                                override_value, reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            o.entity_type,
            o.entity_id,
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
            "SELECT id, entity_type, entity_id, sub_path, field,
                    override_value, reason, created_at
             FROM overrides WHERE id = ?1",
            [id],
            row_to_override,
        )
        .optional()?)
}

pub fn update_override(conn: &Connection, o: &Override) -> Result<(), DbError> {
    conn.execute(
        "UPDATE overrides SET entity_type = ?1, entity_id = ?2, sub_path = ?3,
                              field = ?4, override_value = ?5, reason = ?6
         WHERE id = ?7",
        params![
            o.entity_type,
            o.entity_id,
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
    let mut stmt = conn.prepare(
        "SELECT id, entity_type, entity_id, sub_path, field,
                override_value, reason, created_at
         FROM overrides
         WHERE entity_type = ?1 AND entity_id = ?2
         ORDER BY id",
    )?;
    let rows = stmt.query_map(params![entity_type, entity_id], row_to_override)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
