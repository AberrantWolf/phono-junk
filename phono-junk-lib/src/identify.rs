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

use chrono::Utc;
use phono_junk_catalog::{
    Album, Asset, AssetType as CatalogAssetType, Disagreement, Disc, Id, IdentifyAttemptError,
    Release, Track,
};
use phono_junk_core::{AudioError, DiscIds, IdentificationConfidence, IdentificationSource, Toc};
use phono_junk_db::overrides::{OverrideTarget, apply as apply_override};
use phono_junk_db::{DbError, crud};
use phono_junk_identify::{
    AssetCandidate, AssetLookupCtx, AssetType as ProviderAssetType, DisagreementEntity,
    IdentifyOutcome, ProviderError, RawDisagreement,
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
            if let Some(disc) = find_disc_by_ids(conn, ids)? {
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
        let mut humanized_errors: Vec<IdentifyAttemptError> = outcome
            .errors
            .iter()
            .map(|(name, e)| humanize_provider_error(name, e))
            .collect();
        let mut provider_errors: Vec<(String, String)> = humanized_errors
            .iter()
            .map(|e| (e.provider.clone(), e.message.clone()))
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
            persist_identify_attempt(conn, rip_file_id, &humanized_errors)?;
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

        let merged = outcome.merged;
        let source = first_source(&merged.sources);

        // Asset fan-out runs BEFORE opening the catalog transaction — it's
        // HTTP I/O that can take seconds, and we don't want a SQLite write
        // lock held across it. The candidates are inserted inside the txn.
        let asset_ctx = AssetLookupCtx {
            album: &merged.album,
            release: &merged.release,
            ids,
            creds: &creds,
        };
        let asset_outcome = self.aggregator.lookup_assets(&asset_ctx);
        for (name, e) in &asset_outcome.errors {
            let h = humanize_provider_error(name, e);
            provider_errors.push((h.provider.clone(), h.message.clone()));
            humanized_errors.push(h);
        }

        // Steps 5–9 run in a single transaction so a mid-pipeline failure
        // (e.g. UNIQUE violation during disc upsert) rolls back every
        // partial row instead of stranding an orphan album/release.
        let txn = conn.unchecked_transaction().map_err(DbError::from)?;

        // Step 5: persist catalog rows. Reuse existing Album by MBID.
        let album_id = upsert_album(&txn, &merged.album)?;
        let release_id = upsert_release(&txn, album_id, &merged.release)?;
        let (disc_id, disc_was_reused) = upsert_disc(&txn, release_id, toc, ids)?;
        if disc_was_reused {
            clear_stale_children(&txn, release_id, disc_id)?;
        }
        let mut tracks = insert_tracks(&txn, disc_id, &merged.tracks)?;

        // Step 6: disagreements.
        let any_disagreements = !merged.disagreements.is_empty();
        persist_disagreements(&txn, &merged.disagreements, album_id, release_id, &tracks)?;

        // Step 7: apply overrides.
        let mut album = crud::get_album(&txn, album_id)?
            .ok_or_else(|| IdentifyError::Db(DbError::Migration("album vanished".into())))?;
        let mut release = crud::get_release(&txn, release_id)?
            .ok_or_else(|| IdentifyError::Db(DbError::Migration("release vanished".into())))?;
        let mut disc = crud::get_disc(&txn, disc_id)?
            .ok_or_else(|| IdentifyError::Db(DbError::Migration("disc vanished".into())))?;
        apply_all_overrides(
            &txn,
            &mut album,
            &mut release,
            &mut disc,
            &mut tracks,
        )?;

        // Step 8: insert assets (candidates fetched above, pre-txn).
        let asset_count = insert_assets(&txn, release_id, &asset_outcome.candidates)?;

        // Step 9: update rip file (if present).
        if let Some(rf_id) = rip_file_id {
            update_rip_file(&txn, rf_id, disc_id, source.as_ref())?;
        }
        persist_identify_attempt(&txn, rip_file_id, &humanized_errors)?;

        txn.commit().map_err(DbError::from)?;

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

/// Look up an existing disc by any of its TOC-derived IDs. Shared by the
/// cache-hit path (skip providers entirely) and the re-parent path
/// (`upsert_disc` on force-refresh) so both agree on what "same disc"
/// means — the UNIQUE index on `(ar_discid1, ar_discid2, cddb_id)` is
/// global, so this lookup must be global too.
fn find_disc_by_ids(conn: &Connection, ids: &DiscIds) -> Result<Option<Disc>, DbError> {
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
    // A disc's identity lives in its TOC-derived IDs, not the release it
    // happens to be attached to. On re-identify the providers may route
    // the disc to a different release than before — if we scoped the
    // lookup to the new release_id we'd try to INSERT a fresh disc row,
    // collide with the global UNIQUE (ar_discid1, ar_discid2, cddb_id)
    // index, and leave the freshly-created album/release orphaned.
    // Look up globally; if we find the disc under a different release,
    // re-parent it and sweep the now-empty old release.
    if let Some(mut existing) = find_disc_by_ids(conn, ids)? {
        if existing.release_id != release_id {
            let old_release_id = existing.release_id;
            existing.release_id = release_id;
            crud::update_disc(conn, &existing)?;
            delete_release_if_orphan(conn, old_release_id)?;
        }
        return Ok((existing.id, true));
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
        mcn: None,
    };
    Ok((crud::insert_disc(conn, &disc)?, false))
}

/// When re-parenting a disc moves it off a release, the old release may
/// now be empty. Delete it if so, and cascade-clean its album if that
/// leaves the album empty. Disagreements and overrides are loose-linked
/// (no FK), so sweep them explicitly; assets cascade via the schema FK.
fn delete_release_if_orphan(conn: &Connection, release_id: Id) -> Result<(), DbError> {
    if !crud::list_discs_for_release(conn, release_id)?.is_empty() {
        return Ok(());
    }
    let release = crud::get_release(conn, release_id)?;
    for d in crud::list_disagreements_for(conn, "Release", release_id)? {
        crud::delete_disagreement(conn, d.id)?;
    }
    for o in crud::list_overrides_for(conn, "Release", release_id)? {
        crud::delete_override(conn, o.id)?;
    }
    crud::delete_release(conn, release_id)?;
    if let Some(r) = release {
        if crud::list_releases_for_album(conn, r.album_id)?.is_empty() {
            for d in crud::list_disagreements_for(conn, "Album", r.album_id)? {
                crud::delete_disagreement(conn, d.id)?;
            }
            for o in crud::list_overrides_for(conn, "Album", r.album_id)? {
                crud::delete_override(conn, o.id)?;
            }
            crud::delete_album(conn, r.album_id)?;
        }
    }
    Ok(())
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

/// Persist the per-provider error log from the most recent identify attempt
/// to `rip_files.last_identify_errors` + `last_identify_at`. Called on every
/// fan-out completion (success or failure) so the GUI's detail panel can
/// answer "why didn't this match?" without forcing a re-run.
///
/// `errors` may be empty (all providers succeeded); we still write the
/// timestamp so the panel can show "Last attempted at ...". A `None`
/// `rip_file_id` (identify-only flow with no scan-time row) is a no-op.
fn persist_identify_attempt(
    conn: &Connection,
    rip_file_id: Option<Id>,
    errors: &[IdentifyAttemptError],
) -> Result<(), DbError> {
    let Some(rf_id) = rip_file_id else {
        return Ok(());
    };
    let now = Utc::now().to_rfc3339();
    crud::set_rip_file_identify_attempt(conn, rf_id, Some(errors), &now)
}

/// Convert a `phono-junk-identify::ProviderError` into the persistable,
/// user-facing form. Single boundary between the trait crate's enum and the
/// catalog crate's storage type — nothing else (CLI, GUI, tests) should ever
/// see `ProviderError` formatted as text.
///
/// Detail strings are truncated so a verbose backend response can't bloat the
/// catalog row; the GUI shows full strings either way (text wraps, but
/// pathological responses would still hurt list rendering).
pub(crate) fn humanize_provider_error(
    name: &str,
    err: &ProviderError,
) -> IdentifyAttemptError {
    let message = match err {
        ProviderError::Network(s) => format!("network error: {}", truncate(s, 80)),
        ProviderError::Auth(_) => "authentication failed".to_string(),
        ProviderError::RateLimited => "rate limited".to_string(),
        ProviderError::Parse(_) => "unexpected response from provider".to_string(),
        ProviderError::MissingCredential(_) => "no token (open Settings…)".to_string(),
        ProviderError::Other(s) => truncate(s, 80).to_string(),
    };
    IdentifyAttemptError {
        provider: name.to_string(),
        message,
    }
}

fn truncate(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

fn first_source(sources: &[String]) -> Option<IdentificationSource> {
    sources.first().map(|s| match s.as_str() {
        "musicbrainz" => IdentificationSource::MusicBrainz,
        "discogs" => IdentificationSource::Discogs,
        "itunes" => IdentificationSource::ITunes,
        "amazon" => IdentificationSource::Amazon,
        "tower" => IdentificationSource::Tower,
        other => IdentificationSource::Other(other.to_string()),
    })
}

