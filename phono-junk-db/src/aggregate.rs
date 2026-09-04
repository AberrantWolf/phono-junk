//! Fixed-query repository read models for catalog list and detail screens.

use std::collections::HashMap;

use phono_junk_catalog::{Album, Asset, Disagreement, Disc, Id, Release, RipFile, Track};
use rusqlite::{Connection, OptionalExtension};

use crate::{DbError, crud};

#[derive(Debug, Clone)]
pub struct AlbumListRecord {
    pub album_id: Id,
    pub title: String,
    pub artist: Option<String>,
    pub year: Option<u16>,
    pub mbid: Option<String>,
    pub country: Option<String>,
    pub label: Option<String>,
    pub language: Option<String>,
    pub script: Option<String>,
    pub disc_count: usize,
    pub release_count: usize,
    pub has_non_redumper_rip: bool,
}

/// Load every list row in one statement. Scalar subqueries are deliberately
/// correlated by album so SQLite can use the v7 foreign-key indexes without
/// multiplying rows across releases, discs, and rips.
pub fn list_albums(conn: &Connection) -> Result<Vec<AlbumListRecord>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.title, a.artist_credit, a.year, a.mbid,
                (SELECT r.country FROM releases r WHERE r.album_id = a.id
                 AND r.country IS NOT NULL ORDER BY r.id LIMIT 1) AS country,
                (SELECT r.label FROM releases r WHERE r.album_id = a.id
                 AND r.label IS NOT NULL ORDER BY r.id LIMIT 1) AS label,
                (SELECT r.language FROM releases r WHERE r.album_id = a.id
                 AND r.language IS NOT NULL ORDER BY r.id LIMIT 1) AS language,
                (SELECT r.script FROM releases r WHERE r.album_id = a.id
                 AND r.script IS NOT NULL ORDER BY r.id LIMIT 1) AS script,
                (SELECT COUNT(*) FROM releases r WHERE r.album_id = a.id) AS release_count,
                (SELECT COUNT(*) FROM discs d JOIN releases r ON r.id = d.release_id
                 WHERE r.album_id = a.id) AS disc_count,
                EXISTS(
                    SELECT 1 FROM rip_files rf
                    JOIN discs d ON d.id = rf.disc_id
                    JOIN releases r ON r.id = d.release_id
                    LEFT JOIN rip_file_provenance p ON p.rip_file_id = rf.id
                    WHERE r.album_id = a.id
                    AND (p.ripper IS NULL OR p.ripper != 'redumper')
                ) AS has_non_redumper_rip
         FROM albums a ORDER BY a.id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(AlbumListRecord {
            album_id: row.get("id")?,
            title: row.get("title")?,
            artist: row.get("artist_credit")?,
            year: row.get::<_, Option<i64>>("year")?.map(|value| value as u16),
            mbid: row.get("mbid")?,
            country: row.get("country")?,
            label: row.get("label")?,
            language: row.get("language")?,
            script: row.get("script")?,
            disc_count: row.get::<_, i64>("disc_count")? as usize,
            release_count: row.get::<_, i64>("release_count")? as usize,
            has_non_redumper_rip: row.get("has_non_redumper_rip")?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[derive(Debug, Clone)]
pub struct AlbumAggregate {
    pub album: Album,
    pub releases: Vec<Release>,
    pub discs: Vec<Disc>,
    pub tracks: Vec<Track>,
    pub rip_files: Vec<RipFile>,
    pub assets: Vec<Asset>,
    pub disagreements: Vec<Disagreement>,
}

/// Load an album subtree in a fixed number of indexed queries. No query is
/// issued from inside a release/disc loop, so catalog size affects row count,
/// not round-trip count.
pub fn load_album(conn: &Connection, album_id: Id) -> Result<Option<AlbumAggregate>, DbError> {
    let Some(album) = crud::get_album(conn, album_id)? else {
        return Ok(None);
    };
    let releases = crud::list_releases_for_album(conn, album_id)?;
    let discs = query_discs(conn, album_id)?;
    let tracks = query_tracks(conn, album_id)?;
    let mut rip_files = query_rip_files(conn, album_id)?;
    attach_provenance(conn, album_id, &mut rip_files)?;
    let assets = query_assets(conn, album_id)?;
    let disagreements = query_disagreements(conn, album_id)?;
    Ok(Some(AlbumAggregate {
        album,
        releases,
        discs,
        tracks,
        rip_files,
        assets,
        disagreements,
    }))
}

/// Resolve a disc to the same aggregate consumed by album detail/export.
pub fn load_for_disc(conn: &Connection, disc_id: Id) -> Result<Option<AlbumAggregate>, DbError> {
    let album_id = conn
        .query_row(
            "SELECT r.album_id FROM discs d JOIN releases r ON r.id = d.release_id
             WHERE d.id = ?1",
            [disc_id],
            |row| row.get(0),
        )
        .optional()?;
    album_id
        .map(|id| load_album(conn, id))
        .transpose()
        .map(Option::flatten)
}

fn query_discs(conn: &Connection, album_id: Id) -> Result<Vec<Disc>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT d.id, d.release_id, d.disc_number, d.format, d.toc_json,
                d.mb_discid, d.cddb_id, d.ar_discid1, d.ar_discid2, d.mcn
         FROM discs d JOIN releases r ON r.id = d.release_id
         WHERE r.album_id = ?1 ORDER BY d.release_id, d.disc_number, d.id",
    )?;
    let rows = stmt.query_map([album_id], crud::row_to_disc)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn query_tracks(conn: &Connection, album_id: Id) -> Result<Vec<Track>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT t.id, t.disc_id, t.position, t.title, t.artist_credit,
                t.length_frames, t.isrc, t.mbid, t.recording_mbid
         FROM tracks t JOIN discs d ON d.id = t.disc_id
         JOIN releases r ON r.id = d.release_id
         WHERE r.album_id = ?1 ORDER BY t.disc_id, t.position",
    )?;
    let rows = stmt.query_map([album_id], crud::row_to_track)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn query_rip_files(conn: &Connection, album_id: Id) -> Result<Vec<RipFile>, DbError> {
    let sql = format!(
        "SELECT {} FROM rip_files WHERE disc_id IN (
             SELECT d.id FROM discs d JOIN releases r ON r.id = d.release_id
             WHERE r.album_id = ?1
         ) ORDER BY id",
        crud::RIP_FILE_COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([album_id], crud::row_to_rip_file)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn attach_provenance(
    conn: &Connection,
    album_id: Id,
    rip_files: &mut [RipFile],
) -> Result<(), DbError> {
    let mut stmt = conn.prepare(
        "SELECT p.rip_file_id, p.ripper, p.version, p.drive_json,
                p.read_offset, p.log_path, p.rip_date
         FROM rip_file_provenance p JOIN rip_files rf ON rf.id = p.rip_file_id
         JOIN discs d ON d.id = rf.disc_id JOIN releases r ON r.id = d.release_id
         WHERE r.album_id = ?1",
    )?;
    let rows = stmt.query_map([album_id], |row| {
        Ok((
            row.get::<_, Id>("rip_file_id")?,
            crud::row_to_provenance(row)?,
        ))
    })?;
    let by_id: HashMap<Id, _> = rows.collect::<rusqlite::Result<_>>()?;
    for rip in rip_files {
        rip.provenance = by_id.get(&rip.id).cloned();
    }
    Ok(())
}

fn query_assets(conn: &Connection, album_id: Id) -> Result<Vec<Asset>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.release_id, a.provider, a.asset_type, a.group_id, a.sequence,
                a.source_url, a.file_path, a.width, a.height, a.confidence,
                a.mime_type, a.acquired_at
         FROM assets a JOIN releases r ON r.id = a.release_id
         WHERE r.album_id = ?1
         ORDER BY a.release_id, COALESCE(a.group_id, -1), a.sequence, a.id",
    )?;
    let rows = stmt.query_map([album_id], crud::row_to_asset)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn query_disagreements(conn: &Connection, album_id: Id) -> Result<Vec<Disagreement>, DbError> {
    let mut stmt = conn.prepare(
        "SELECT id, entity_type, entity_id, entity_key, field, source_a, value_a,
                source_b, value_b, resolved, created_at
         FROM disagreements
         WHERE entity_key IN (SELECT stable_key FROM albums WHERE id = ?1)
            OR entity_key IN (SELECT stable_key FROM releases WHERE album_id = ?1)
            OR entity_key IN (
                SELECT d.stable_key FROM discs d JOIN releases r ON r.id = d.release_id
                WHERE r.album_id = ?1)
         ORDER BY id",
    )?;
    let rows = stmt.query_map([album_id], crud::row_to_disagreement)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
