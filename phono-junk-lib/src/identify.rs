//! Identify pipeline: [`PhonoContext::identify_disc`].
//!
//! Given a [`Toc`] and its [`DiscIds`], this is the single entry point
//! for "resolve this disc against every registered provider, reconcile
//! field-level disagreements, apply any user overrides, and persist the
//! result into the catalog." Shared by CLI and GUI; Sprint 12's extract
//! pipeline consumes whatever this writes to SQLite.
//!
//! The full pipeline:
//!
//! 1. Cache lookup — if a `Disc` with a matching MusicBrainz DiscID or
//!    AccurateRip triple already exists and `force_refresh` is false,
//!    return the cached ids (`IdentifiedDisc.cached = true`).
//! 2. Parallel fan-out across [`IdentificationProvider`]s via
//!    [`Aggregator::identify`]. Provider errors are collected, never
//!    fatal.
//! 3. Consensus merge — one winning value per field, conflicts tracked
//!    as `RawDisagreement`s. MBID-cohort rule excludes providers with
//!    different Album/Release MBIDs from field merging.
//! 4. If no provider returned a match, mark the `RipFile` as
//!    `Unidentified` and return — the TOC is preserved so a later
//!    scan with richer provider credentials can retry.
//! 5. Upsert `Album` / `Release` / `Disc` / `Track` rows. An existing
//!    album with matching MBID is reused to avoid duplicates.
//! 6. Translate `RawDisagreement`s to `Disagreement` rows with the
//!    fresh entity IDs.
//! 7. Apply any existing `Override` rows to the newly-persisted
//!    entities via `phono_junk_db::overrides::apply`, re-updating
//!    mutated rows. Overrides do *not* flip `Disagreement.resolved` —
//!    they bypass, not resolve.
//! 8. Asset fan-out across [`AssetProvider`]s; insert each candidate
//!    as an `Asset` row on the release (deduped by `(type, url)`).
//! 9. Update the `RipFile` with the resolved `disc_id`, confidence,
//!    and provider source.

use phono_junk_catalog::{
    Album, Asset, AssetType as CatalogAssetType, Disagreement, Disc, Id, Release, Track,
};
use phono_junk_core::{AudioError, DiscIds, IdentificationConfidence, IdentificationSource, Toc};
use phono_junk_db::overrides::{OverrideTarget, apply as apply_override};
use phono_junk_db::{DbError, crud};
use phono_junk_identify::{
    AssetCandidate, AssetLookupCtx, AssetType as ProviderAssetType, DisagreementEntity,
    IdentifyOutcome, RawDisagreement,
};
use rusqlite::Connection;

use crate::PhonoContext;

/// Outcome of [`PhonoContext::identify_disc`]. IDs point at the persisted
/// catalog rows; counts / flags are convenience stats for callers that
/// want to log or branch on the result.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IdentifiedDisc {
    pub disc_id: Option<Id>,
    pub album_id: Option<Id>,
    pub release_id: Option<Id>,
    pub any_disagreements: bool,
    pub asset_count: usize,
    pub cached: bool,
    pub identified: bool,
    /// Per-provider error messages surfaced during fan-out. Non-fatal —
    /// the pipeline proceeds on whichever providers succeeded.
    pub provider_errors: Vec<(String, String)>,
}

/// Errors from [`PhonoContext::identify_disc`].
#[derive(Debug, thiserror::Error)]
pub enum IdentifyError {
    #[error(transparent)]
    Audio(#[from] AudioError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("override application failed: {0}")]
    Override(#[from] phono_junk_db::overrides::OverrideError),
}

impl PhonoContext {
    /// Identify a disc and persist the result into `conn`.
    ///
    /// `rip_file_id` points at the pre-existing `rip_files` row for the
    /// source medium (created during scan). When set, it's updated with
    /// the resolved `disc_id` and confidence; when `None` no rip-file
    /// bookkeeping happens (useful for identify-only flows like a
    /// user-driven manual lookup from the GUI).
    ///
    /// `force_refresh = true` bypasses the catalog cache and re-runs
    /// every provider. Used by "re-identify" workflows.
    pub fn identify_disc(
        &self,
        conn: &Connection,
        toc: &Toc,
        ids: &DiscIds,
        rip_file_id: Option<Id>,
        force_refresh: bool,
    ) -> Result<IdentifiedDisc, IdentifyError> {
        // Step 1: cache lookup.
        if !force_refresh {
            if let Some(disc) = find_cached_disc(conn, ids)? {
                return Ok(cached_outcome(conn, disc, rip_file_id)?);
            }
        }

        // Step 2+3: fan-out + merge.
        let creds = self.credentials.to_credentials();
        log::info!(
            "identify: dispatching to providers — mb_discid={:?} cddb_id={:?} ar1={:?}",
            ids.mb_discid,
            ids.cddb_id,
            ids.ar_discid1,
        );
        let outcome: IdentifyOutcome = self.aggregator.identify(toc, ids, &creds);
        let provider_errors: Vec<(String, String)> = outcome
            .errors
            .iter()
            .map(|(name, e)| (name.clone(), e.to_string()))
            .collect();
        for (name, err) in &provider_errors {
            log::warn!("identify: provider {name} returned error: {err}");
        }
        log::info!(
            "identify: fan-out complete — any_match={} sources={:?} errors={}",
            outcome.any_match,
            outcome.merged.sources,
            provider_errors.len(),
        );

        if !outcome.any_match {
            // Step 4: unidentified. Preserve TOC on the rip file; no
            // Album/Release/Disc row is created.
            mark_unidentified(conn, rip_file_id)?;
            return Ok(IdentifiedDisc {
                disc_id: None,
                album_id: None,
                release_id: None,
                any_disagreements: false,
                asset_count: 0,
                cached: false,
                identified: false,
                provider_errors,
            });
        }

        // Step 5: persist catalog rows. Reuse existing Album by MBID.
        let merged = outcome.merged;
        let album_id = upsert_album(conn, &merged.album)?;
        let release_id = upsert_release(conn, album_id, &merged.release)?;
        let (disc_id, disc_was_reused) = upsert_disc(conn, release_id, toc, ids)?;
        if disc_was_reused {
            clear_stale_children(conn, release_id, disc_id)?;
        }
        let mut tracks = insert_tracks(conn, disc_id, &merged.tracks)?;

        // Step 6: disagreements.
        let any_disagreements = !merged.disagreements.is_empty();
        persist_disagreements(conn, &merged.disagreements, album_id, release_id, &tracks)?;

        // Step 7: apply overrides.
        let mut album = crud::get_album(conn, album_id)?
            .ok_or_else(|| IdentifyError::Db(DbError::Migration("album vanished".into())))?;
        let mut release = crud::get_release(conn, release_id)?
            .ok_or_else(|| IdentifyError::Db(DbError::Migration("release vanished".into())))?;
        let mut disc = crud::get_disc(conn, disc_id)?
            .ok_or_else(|| IdentifyError::Db(DbError::Migration("disc vanished".into())))?;
        apply_all_overrides(
            conn,
            &mut album,
            &mut release,
            &mut disc,
            &mut tracks,
        )?;

        // Step 8: assets.
        let source = first_source(&merged.sources);
        let ctx = AssetLookupCtx {
            album: &merged.album,
            release: &merged.release,
            ids,
            creds: &creds,
        };
        let asset_outcome = self.aggregator.lookup_assets(&ctx);
        let asset_count = insert_assets(conn, release_id, &asset_outcome.candidates)?;
        let mut provider_errors = provider_errors;
        for (name, e) in asset_outcome.errors {
            provider_errors.push((name, e.to_string()));
        }

        // Step 9: update rip file (if present).
        if let Some(rf_id) = rip_file_id {
            update_rip_file(conn, rf_id, disc_id, source.as_ref())?;
        }

        Ok(IdentifiedDisc {
            disc_id: Some(disc_id),
            album_id: Some(album_id),
            release_id: Some(release_id),
            any_disagreements,
            asset_count,
            cached: false,
            identified: true,
            provider_errors,
        })
    }
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

fn find_cached_disc(conn: &Connection, ids: &DiscIds) -> Result<Option<Disc>, DbError> {
    if let Some(mb) = ids.mb_discid.as_deref() {
        if let Some(disc) = crud::find_disc_by_mb_discid(conn, mb)? {
            return Ok(Some(disc));
        }
    }
    if let (Some(a1), Some(a2), Some(cddb)) = (
        ids.ar_discid1.as_deref(),
        ids.ar_discid2.as_deref(),
        ids.cddb_id.as_deref(),
    ) {
        if let Some(disc) = crud::find_disc_by_ar_triple(conn, a1, a2, cddb)? {
            return Ok(Some(disc));
        }
    }
    Ok(None)
}

fn cached_outcome(
    conn: &Connection,
    disc: Disc,
    rip_file_id: Option<Id>,
) -> Result<IdentifiedDisc, IdentifyError> {
    let release = crud::get_release(conn, disc.release_id)?
        .ok_or_else(|| IdentifyError::Db(DbError::Migration("release missing for cached disc".into())))?;
    if let Some(rf_id) = rip_file_id {
        update_rip_file(conn, rf_id, disc.id, None)?;
    }
    Ok(IdentifiedDisc {
        disc_id: Some(disc.id),
        album_id: Some(release.album_id),
        release_id: Some(disc.release_id),
        any_disagreements: false,
        asset_count: 0,
        cached: true,
        identified: true,
        provider_errors: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Catalog upserts
// ---------------------------------------------------------------------------

fn upsert_album(
    conn: &Connection,
    meta: &phono_junk_identify::AlbumMeta,
) -> Result<Id, DbError> {
    if let Some(mbid) = meta.mbid.as_deref() {
        if let Some(existing) = find_album_by_mbid(conn, mbid)? {
            return Ok(existing.id);
        }
    }
    let album = Album {
        id: 0,
        title: meta.title.clone().unwrap_or_default(),
        sort_title: None,
        artist_credit: meta.artist_credit.clone(),
        year: meta.year,
        mbid: meta.mbid.clone(),
        primary_type: None,
        secondary_types: Vec::new(),
        first_release_date: None,
    };
    crud::insert_album(conn, &album)
}

fn find_album_by_mbid(conn: &Connection, mbid: &str) -> Result<Option<Album>, DbError> {
    // CRUD doesn't expose a by-MBID finder yet, but a full scan of
    // `list_albums` is cheap for libraries in the thousands and keeps
    // Sprint 11 from bloating the DB surface. Revisit if bench numbers
    // prove this is a hot path — a `find_album_by_mbid` helper is
    // trivial to add.
    for a in crud::list_albums(conn)? {
        if a.mbid.as_deref() == Some(mbid) {
            return Ok(Some(a));
        }
    }
    Ok(None)
}

fn upsert_release(
    conn: &Connection,
    album_id: Id,
    meta: &phono_junk_identify::ReleaseMeta,
) -> Result<Id, DbError> {
    if let Some(mbid) = meta.mbid.as_deref() {
        for r in crud::list_releases_for_album(conn, album_id)? {
            if r.mbid.as_deref() == Some(mbid) {
                return Ok(r.id);
            }
        }
    }
    let release = Release {
        id: 0,
        album_id,
        country: meta.country.clone(),
        date: meta.date.clone(),
        label: meta.label.clone(),
        catalog_number: meta.catalog_number.clone(),
        barcode: meta.barcode.clone(),
        mbid: meta.mbid.clone(),
        status: None,
        language: meta.language.clone(),
        script: meta.script.clone(),
    };
    crud::insert_release(conn, &release)
}

fn upsert_disc(
    conn: &Connection,
    release_id: Id,
    toc: &Toc,
    ids: &DiscIds,
) -> Result<(Id, bool), DbError> {
    // Reuse an existing disc under this release with the same disc_number
    // + mb_discid. For brand-new identifies this is always empty so falls
    // through to insert. The bool signals "reused, so wipe stale children
    // before re-inserting."
    for d in crud::list_discs_for_release(conn, release_id)? {
        if d.mb_discid == ids.mb_discid && d.disc_number == 1 {
            return Ok((d.id, true));
        }
    }
    let disc = Disc {
        id: 0,
        release_id,
        disc_number: 1,
        format: "CD".to_string(),
        toc: Some(toc.clone()),
        mb_discid: ids.mb_discid.clone(),
        cddb_id: ids.cddb_id.clone(),
        ar_discid1: ids.ar_discid1.clone(),
        ar_discid2: ids.ar_discid2.clone(),
        dbar_raw: None,
    };
    Ok((crud::insert_disc(conn, &disc)?, false))
}

/// When an identify run reuses an existing disc (force-refresh of a disc
/// we already scraped), wipe tracks / disagreements / assets before
/// inserting fresh rows — otherwise the `UNIQUE (disc_id, position)`
/// constraint on tracks triggers, and stale disagreements / assets pile
/// up alongside the new ones.
fn clear_stale_children(conn: &Connection, release_id: Id, disc_id: Id) -> Result<(), DbError> {
    for t in crud::list_tracks_for_disc(conn, disc_id)? {
        for d in crud::list_disagreements_for(conn, "Track", t.id)? {
            crud::delete_disagreement(conn, d.id)?;
        }
        crud::delete_track(conn, t.id)?;
    }
    for d in crud::list_disagreements_for(conn, "Disc", disc_id)? {
        crud::delete_disagreement(conn, d.id)?;
    }
    for d in crud::list_disagreements_for(conn, "Release", release_id)? {
        crud::delete_disagreement(conn, d.id)?;
    }
    for a in crud::list_assets_for_release(conn, release_id)? {
        crud::delete_asset(conn, a.id)?;
    }
    Ok(())
}

fn insert_tracks(
    conn: &Connection,
    disc_id: Id,
    metas: &[phono_junk_identify::TrackMeta],
) -> Result<Vec<Track>, DbError> {
    let mut out = Vec::with_capacity(metas.len());
    for m in metas {
        let track = Track {
            id: 0,
            disc_id,
            position: m.position,
            title: m.title.clone(),
            artist_credit: m.artist_credit.clone(),
            length_frames: m.length_frames,
            isrc: m.isrc.clone(),
            mbid: m.mbid.clone(),
            recording_mbid: None,
        };
        let id = crud::insert_track(conn, &track)?;
        let mut stored = track;
        stored.id = id;
        out.push(stored);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Disagreements
// ---------------------------------------------------------------------------

fn persist_disagreements(
    conn: &Connection,
    raw: &[RawDisagreement],
    album_id: Id,
    release_id: Id,
    tracks: &[Track],
) -> Result<(), DbError> {
    for d in raw {
        let (entity_type, entity_id) = match d.entity {
            DisagreementEntity::Album => ("Album", album_id),
            DisagreementEntity::Release => ("Release", release_id),
            DisagreementEntity::Track { position } => {
                match tracks.iter().find(|t| t.position == position) {
                    Some(t) => ("Track", t.id),
                    None => {
                        log::warn!(
                            "disagreement references missing track position {position}; skipping"
                        );
                        continue;
                    }
                }
            }
        };
        let row = Disagreement {
            id: 0,
            entity_type: entity_type.to_string(),
            entity_id,
            field: d.field.to_string(),
            source_a: d.source_a.clone(),
            value_a: json_to_string(&d.value_a),
            source_b: d.source_b.clone(),
            value_b: json_to_string(&d.value_b),
            resolved: false,
            created_at: None,
        };
        crud::insert_disagreement(conn, &row)?;
    }
    Ok(())
}

fn json_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Overrides
// ---------------------------------------------------------------------------

fn apply_all_overrides(
    conn: &Connection,
    album: &mut Album,
    release: &mut Release,
    disc: &mut Disc,
    tracks: &mut [Track],
) -> Result<(), IdentifyError> {
    let mut changed_album = false;
    let mut changed_release = false;
    let mut changed_disc_or_tracks = false;
    let mut changed_tracks: Vec<Id> = Vec::new();

    for ovr in crud::list_overrides_for(conn, "Album", album.id)? {
        apply_override(OverrideTarget::Album(album), &ovr)?;
        changed_album = true;
    }
    for ovr in crud::list_overrides_for(conn, "Release", release.id)? {
        apply_override(OverrideTarget::Release(release), &ovr)?;
        changed_release = true;
    }
    for ovr in crud::list_overrides_for(conn, "Disc", disc.id)? {
        apply_override(
            OverrideTarget::Disc {
                disc,
                tracks,
            },
            &ovr,
        )?;
        changed_disc_or_tracks = true;
    }
    for t in tracks.iter_mut() {
        let mut touched = false;
        for ovr in crud::list_overrides_for(conn, "Track", t.id)? {
            apply_override(OverrideTarget::Track(t), &ovr)?;
            touched = true;
        }
        if touched {
            changed_tracks.push(t.id);
        }
    }

    if changed_album {
        crud::update_album(conn, album)?;
    }
    if changed_release {
        crud::update_release(conn, release)?;
    }
    if changed_disc_or_tracks {
        crud::update_disc(conn, disc)?;
        for t in tracks.iter() {
            crud::update_track(conn, t)?;
        }
    } else {
        for t in tracks.iter() {
            if changed_tracks.contains(&t.id) {
                crud::update_track(conn, t)?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Assets
// ---------------------------------------------------------------------------

fn insert_assets(
    conn: &Connection,
    release_id: Id,
    candidates: &[AssetCandidate],
) -> Result<usize, DbError> {
    let mut count = 0;
    for c in candidates {
        let asset = Asset {
            id: 0,
            release_id,
            asset_type: provider_to_catalog(&c.asset_type),
            group_id: None,
            sequence: 0,
            source_url: Some(c.source_url.as_str().to_string()),
            file_path: None,
            scraped_at: None,
        };
        crud::insert_asset(conn, &asset)?;
        count += 1;
    }
    Ok(count)
}

fn provider_to_catalog(t: &ProviderAssetType) -> CatalogAssetType {
    match t {
        ProviderAssetType::FrontCover => CatalogAssetType::FrontCover,
        ProviderAssetType::BackCover => CatalogAssetType::BackCover,
        ProviderAssetType::CdLabel => CatalogAssetType::CdLabel,
        ProviderAssetType::Booklet => CatalogAssetType::Booklet,
        ProviderAssetType::ObiStrip => CatalogAssetType::ObiStrip,
        ProviderAssetType::TrayInsert => CatalogAssetType::TrayInsert,
        ProviderAssetType::Other => CatalogAssetType::Other("unspecified".into()),
    }
}

// ---------------------------------------------------------------------------
// RipFile bookkeeping
// ---------------------------------------------------------------------------

fn mark_unidentified(conn: &Connection, rip_file_id: Option<Id>) -> Result<(), DbError> {
    let Some(rf_id) = rip_file_id else {
        return Ok(());
    };
    let Some(mut rf) = crud::get_rip_file(conn, rf_id)? else {
        return Ok(());
    };
    rf.disc_id = None;
    rf.identification_confidence = IdentificationConfidence::Unidentified;
    rf.identification_source = None;
    crud::update_rip_file(conn, &rf)
}

fn update_rip_file(
    conn: &Connection,
    rip_file_id: Id,
    disc_id: Id,
    source: Option<&IdentificationSource>,
) -> Result<(), DbError> {
    let Some(mut rf) = crud::get_rip_file(conn, rip_file_id)? else {
        return Ok(());
    };
    rf.disc_id = Some(disc_id);
    rf.identification_confidence = IdentificationConfidence::Certain;
    if let Some(src) = source {
        rf.identification_source = Some(src.clone());
    }
    crud::update_rip_file(conn, &rf)
}

fn first_source(sources: &[String]) -> Option<IdentificationSource> {
    sources.first().map(|s| match s.as_str() {
        "musicbrainz" => IdentificationSource::MusicBrainz,
        "discogs" => IdentificationSource::Discogs,
        "itunes" => IdentificationSource::ITunes,
        "amazon" => IdentificationSource::Amazon,
        other => IdentificationSource::Other(other.to_string()),
    })
}

