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
    Album, AssetType as CatalogAssetType, Disagreement, Disc, Id, IdentifyAttemptError, Release,
    Track,
};
use phono_junk_core::{AudioError, DiscIds, IdentificationConfidence, IdentificationSource, Toc};
use phono_junk_db::overrides::{OverrideTarget, apply as apply_override};
use phono_junk_db::{DbError, crud, evidence};
use phono_junk_identify::{
    AssetCandidate, AssetConfidence, AssetLookupCtx, AssetType as ProviderAssetType,
    CandidateResolution, DisagreementEntity, DiscIdKind, ProviderError, RawDisagreement,
    ReleaseCandidate,
};
use rusqlite::Connection;
use std::collections::HashMap;
use url::Url;

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
        if !force_refresh && let Some(disc) = find_disc_by_ids(conn, ids)? {
            return cached_outcome(conn, disc, rip_file_id);
        }

        // Append-only attempt row lands before provider work. Individual
        // observations are committed after each staged outcome is available;
        // the final projection is a separate atomic transaction.
        let attempt_key = stable_disc_key(ids);
        let attempt_id = evidence::start_identification_attempt(conn, rip_file_id, &attempt_key)?;
        let creds = self.credentials.to_credentials();
        log::info!(
            "identify: dispatching to providers — mb_discid={:?} cddb_id={:?} ar1={:?}",
            ids.mb_discid,
            ids.cddb_id,
            ids.ar_discid1,
        );
        let outcome = self.aggregator.identify_staged(toc, ids, &creds);
        let observation_ids =
            persist_provider_observations(conn, attempt_id, &outcome.observations)?;
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
            "identify: staged lookup complete — candidates={} errors={}",
            outcome
                .resolution
                .as_ref()
                .map_or(0, |resolution| resolution.alternatives.len() + 1),
            provider_errors.len(),
        );

        let Some(resolution) = outcome.resolution else {
            // Step 4: unidentified. Preserve TOC on the rip file; no
            // Album/Release/Disc row is created.
            mark_unidentified(conn, rip_file_id)?;
            persist_identify_attempt(conn, rip_file_id, &humanized_errors)?;
            evidence::finish_identification_attempt(
                conn,
                attempt_id,
                "unidentified",
                None,
                Some("unidentified"),
                None,
            )?;
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
        };

        persist_candidates(conn, attempt_id, &resolution, &observation_ids)?;
        let selected_candidate = resolution.selected.candidate.clone();
        let selected_provider = selected_candidate.provider.clone();
        let physical_disc_number = selected_candidate.physical_disc_number.unwrap_or(1);
        let mut merged = phono_junk_identify::merge_with_toc_fallback(
            &[selected_candidate.clone().into_provider_result()],
            toc,
        );
        // Alternative observations remain disagreement evidence even though
        // only the scored winner is projected into catalog fields.
        let all_results: Vec<_> = std::iter::once(&resolution.selected)
            .chain(resolution.alternatives.iter())
            .map(|scored| scored.candidate.clone().into_provider_result())
            .collect();
        merged.disagreements =
            phono_junk_identify::merge_with_toc_fallback(&all_results, toc).disagreements;
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
        let mut asset_outcome = self
            .aggregator
            .lookup_assets_excluding(&asset_ctx, &[selected_provider.as_str()]);
        asset_outcome
            .candidates
            .extend(reused_asset_candidates(&selected_candidate));
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
        let release_id = upsert_release(
            &txn,
            album_id,
            &merged.release,
            &resolution.selected.candidate.candidate_key,
        )?;
        let (disc_id, _) = upsert_disc(&txn, release_id, physical_disc_number, toc, ids)?;
        let mut tracks = upsert_tracks(&txn, disc_id, &merged.tracks)?;

        // Step 6: disagreements.
        let any_disagreements =
            !merged.disagreements.is_empty() || resolution.evidentially_ambiguous;
        persist_disagreements(&txn, &merged.disagreements, album_id, release_id, &tracks)?;
        if resolution.evidentially_ambiguous {
            persist_candidate_ambiguity(&txn, release_id, &resolution)?;
        }

        // Step 7: apply overrides.
        let mut album = crud::get_album(&txn, album_id)?
            .ok_or_else(|| IdentifyError::Db(DbError::Migration("album vanished".into())))?;
        let mut release = crud::get_release(&txn, release_id)?
            .ok_or_else(|| IdentifyError::Db(DbError::Migration("release vanished".into())))?;
        let mut disc = crud::get_disc(&txn, disc_id)?
            .ok_or_else(|| IdentifyError::Db(DbError::Migration("disc vanished".into())))?;
        apply_all_overrides(&txn, &mut album, &mut release, &mut disc, &mut tracks)?;

        // Step 8: insert assets (candidates fetched above, pre-txn).
        let asset_count = insert_assets(&txn, release_id, &asset_outcome.candidates)?;

        // Step 9: update rip file (if present).
        if let Some(rf_id) = rip_file_id {
            update_rip_file(
                &txn,
                rf_id,
                disc_id,
                source.as_ref(),
                resolution.evidentially_ambiguous,
            )?;
        }
        persist_identify_attempt(&txn, rip_file_id, &humanized_errors)?;
        let ambiguity_json = resolution
            .evidentially_ambiguous
            .then(|| ambiguity_json(&resolution))
            .transpose()?;
        evidence::finish_identification_attempt(
            &txn,
            attempt_id,
            "resolved",
            Some(&resolution.selected.candidate.candidate_key),
            Some(if resolution.evidentially_ambiguous {
                "low"
            } else {
                "high"
            }),
            ambiguity_json.as_deref(),
        )?;

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

fn stable_disc_key(ids: &DiscIds) -> String {
    if let (Some(id1), Some(id2), Some(cddb)) = (
        ids.ar_discid1.as_deref(),
        ids.ar_discid2.as_deref(),
        ids.cddb_id.as_deref(),
    ) {
        format!("disc:accuraterip:{id1}:{id2}:{cddb}")
    } else if let Some(mbid) = ids.mb_discid.as_deref() {
        format!("disc:musicbrainz:{mbid}")
    } else {
        format!(
            "disc:local:attempt-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        )
    }
}

fn persist_provider_observations(
    conn: &Connection,
    attempt_id: i64,
    observations: &[phono_junk_identify::ProviderObservation],
) -> Result<HashMap<String, Vec<i64>>, DbError> {
    let mut ids = HashMap::new();
    for observation in observations {
        let raw = observation
            .lookup
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| DbError::Migration(error.to_string()))?;
        let id = evidence::insert_provider_observation(
            conn,
            attempt_id,
            &evidence::NewProviderObservation {
                provider: &observation.provider,
                input_kind: id_kind(observation.input_kind),
                input_value: &observation.input_value,
                stage: observation.stage,
                raw_response_json: raw.as_deref(),
                error_text: observation.error.as_deref(),
            },
        )?;
        ids.entry(observation.provider.clone())
            .or_insert_with(Vec::new)
            .push(id);
    }
    Ok(ids)
}

fn persist_candidates(
    conn: &Connection,
    attempt_id: i64,
    resolution: &CandidateResolution,
    observation_ids: &HashMap<String, Vec<i64>>,
) -> Result<(), DbError> {
    for (selected, scored) in std::iter::once((true, &resolution.selected)).chain(
        resolution
            .alternatives
            .iter()
            .map(|candidate| (false, candidate)),
    ) {
        let candidate_json = serde_json::to_string(&scored.candidate)
            .map_err(|error| DbError::Migration(error.to_string()))?;
        let score_json = serde_json::to_string(&scored.score)
            .map_err(|error| DbError::Migration(error.to_string()))?;
        let candidate_id = evidence::insert_identification_candidate(
            conn,
            attempt_id,
            &scored.candidate.candidate_key,
            &candidate_json,
            &score_json,
            selected,
        )?;
        if let Some(provider_observations) = observation_ids.get(&scored.candidate.provider) {
            for &observation_id in provider_observations {
                evidence::link_candidate_observation(conn, candidate_id, observation_id)?;
            }
        }
    }
    Ok(())
}

fn persist_candidate_ambiguity(
    conn: &Connection,
    release_id: Id,
    resolution: &CandidateResolution,
) -> Result<(), DbError> {
    let alternatives = serde_json::to_string(
        &resolution
            .alternatives
            .iter()
            .map(|candidate| &candidate.candidate.candidate_key)
            .collect::<Vec<_>>(),
    )
    .map_err(|error| DbError::Migration(error.to_string()))?;
    crud::insert_disagreement(
        conn,
        &Disagreement {
            id: 0,
            entity_type: "Release".into(),
            entity_id: release_id,
            entity_key: None,
            field: "identification_candidate".into(),
            source_a: resolution.selected.candidate.provider.clone(),
            value_a: resolution.selected.candidate.candidate_key.clone(),
            source_b: "candidate_pipeline".into(),
            value_b: alternatives,
            resolved: false,
            created_at: None,
        },
    )?;
    Ok(())
}

fn ambiguity_json(resolution: &CandidateResolution) -> Result<String, DbError> {
    serde_json::to_string(
        &resolution
            .alternatives
            .iter()
            .map(|candidate| {
                serde_json::json!({
                    "candidate_key": candidate.candidate.candidate_key,
                    "provider": candidate.candidate.provider,
                    "score": candidate.score,
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|error| DbError::Migration(error.to_string()))
}

fn reused_asset_candidates(candidate: &ReleaseCandidate) -> Vec<AssetCandidate> {
    candidate
        .cover_art_urls
        .iter()
        .filter_map(|value| Url::parse(value).ok())
        .map(|source_url| AssetCandidate {
            provider: candidate.provider.clone(),
            asset_type: ProviderAssetType::FrontCover,
            source_url,
            width: None,
            height: None,
            confidence: AssetConfidence::Identifier,
        })
        .collect()
}

fn id_kind(kind: DiscIdKind) -> &'static str {
    match kind {
        DiscIdKind::MbDiscId => "mb_discid",
        DiscIdKind::CddbId => "cddb_id",
        DiscIdKind::AccurateRipId => "accuraterip_id",
        DiscIdKind::Barcode => "barcode",
        DiscIdKind::CatalogNumber => "catalog_number",
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
    if let Some(mb) = ids.mb_discid.as_deref()
        && let Some(disc) = crud::find_disc_by_mb_discid(conn, mb)?
    {
        return Ok(Some(disc));
    }
    if let (Some(a1), Some(a2), Some(cddb)) = (
        ids.ar_discid1.as_deref(),
        ids.ar_discid2.as_deref(),
        ids.cddb_id.as_deref(),
    ) && let Some(disc) = crud::find_disc_by_ar_triple(conn, a1, a2, cddb)?
    {
        return Ok(Some(disc));
    }
    Ok(None)
}

fn cached_outcome(
    conn: &Connection,
    disc: Disc,
    rip_file_id: Option<Id>,
) -> Result<IdentifiedDisc, IdentifyError> {
    let release = crud::get_release(conn, disc.release_id)?.ok_or_else(|| {
        IdentifyError::Db(DbError::Migration("release missing for cached disc".into()))
    })?;
    if let Some(rf_id) = rip_file_id {
        update_rip_file(conn, rf_id, disc.id, None, false)?;
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

fn upsert_album(conn: &Connection, meta: &phono_junk_identify::AlbumMeta) -> Result<Id, DbError> {
    if let Some(mbid) = meta.mbid.as_deref()
        && let Some(mut existing) = find_album_by_mbid(conn, mbid)?
    {
        existing.title = meta.title.clone().unwrap_or(existing.title);
        existing.artist_credit = meta.artist_credit.clone().or(existing.artist_credit);
        existing.year = meta.year.or(existing.year);
        crud::update_album(conn, &existing)?;
        return Ok(existing.id);
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
    candidate_key: &str,
) -> Result<Id, DbError> {
    if let Some(mbid) = meta.mbid.as_deref() {
        for mut r in crud::list_releases_for_album(conn, album_id)? {
            if r.mbid.as_deref() == Some(mbid) {
                r.country = meta.country.clone().or(r.country);
                r.date = meta.date.clone().or(r.date);
                r.label = meta.label.clone().or(r.label);
                r.catalog_number = meta.catalog_number.clone().or(r.catalog_number);
                r.barcode = meta.barcode.clone().or(r.barcode);
                r.language = meta.language.clone().or(r.language);
                r.script = meta.script.clone().or(r.script);
                crud::update_release(conn, &r)?;
                return Ok(r.id);
            }
        }
    }
    let stable_key = format!("release:{candidate_key}");
    if let Some(mut release) = crud::find_release_by_stable_key(conn, &stable_key)? {
        release.album_id = album_id;
        release.country = meta.country.clone().or(release.country);
        release.date = meta.date.clone().or(release.date);
        release.label = meta.label.clone().or(release.label);
        release.catalog_number = meta.catalog_number.clone().or(release.catalog_number);
        release.barcode = meta.barcode.clone().or(release.barcode);
        release.language = meta.language.clone().or(release.language);
        release.script = meta.script.clone().or(release.script);
        crud::update_release(conn, &release)?;
        return Ok(release.id);
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
    let release_id = crud::insert_release(conn, &release)?;
    if meta.mbid.is_none() {
        crud::set_catalog_entity_key(conn, "release", release_id, &stable_key)?;
    }
    Ok(release_id)
}

fn upsert_disc(
    conn: &Connection,
    release_id: Id,
    disc_number: u8,
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
        existing.disc_number = disc_number;
        existing.toc = Some(toc.clone());
        existing.mb_discid = ids.mb_discid.clone().or(existing.mb_discid);
        existing.cddb_id = ids.cddb_id.clone().or(existing.cddb_id);
        existing.ar_discid1 = ids.ar_discid1.clone().or(existing.ar_discid1);
        existing.ar_discid2 = ids.ar_discid2.clone().or(existing.ar_discid2);
        crud::update_disc(conn, &existing)?;
        return Ok((existing.id, true));
    }
    let disc = Disc {
        id: 0,
        release_id,
        disc_number,
        format: "CD".to_string(),
        toc: Some(toc.clone()),
        mb_discid: ids.mb_discid.clone(),
        cddb_id: ids.cddb_id.clone(),
        ar_discid1: ids.ar_discid1.clone(),
        ar_discid2: ids.ar_discid2.clone(),
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
    if let Some(r) = release
        && crud::list_releases_for_album(conn, r.album_id)?.is_empty()
    {
        for d in crud::list_disagreements_for(conn, "Album", r.album_id)? {
            crud::delete_disagreement(conn, d.id)?;
        }
        for o in crud::list_overrides_for(conn, "Album", r.album_id)? {
            crud::delete_override(conn, o.id)?;
        }
        crud::delete_album(conn, r.album_id)?;
    }
    Ok(())
}

/// Refresh track projections in place, preserving stable row IDs so overrides
/// and cached evidence remain attached across identification runs.
fn upsert_tracks(
    conn: &Connection,
    disc_id: Id,
    metas: &[phono_junk_identify::TrackMeta],
) -> Result<Vec<Track>, DbError> {
    let mut existing: HashMap<u8, Track> = crud::list_tracks_for_disc(conn, disc_id)?
        .into_iter()
        .map(|track| (track.position, track))
        .collect();
    let mut out = Vec::with_capacity(metas.len());
    for m in metas {
        let mut track = existing.remove(&m.position).unwrap_or(Track {
            id: 0,
            disc_id,
            position: m.position,
            title: None,
            artist_credit: None,
            length_frames: None,
            isrc: None,
            mbid: None,
            recording_mbid: None,
        });
        track.title = m.title.clone().or(track.title);
        track.artist_credit = m.artist_credit.clone().or(track.artist_credit);
        track.length_frames = m.length_frames.or(track.length_frames);
        track.isrc = m.isrc.clone().or(track.isrc);
        track.mbid = m.mbid.clone().or(track.mbid);
        if track.id == 0 {
            track.id = crud::insert_track(conn, &track)?;
        } else {
            crud::update_track(conn, &track)?;
        }
        out.push(track);
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
            entity_key: None,
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
        apply_override(OverrideTarget::Disc { disc, tracks }, &ovr)?;
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
        crud::upsert_asset_evidence(
            conn,
            release_id,
            &c.provider,
            &provider_to_catalog(&c.asset_type),
            c.source_url.as_str(),
            c.width,
            c.height,
            asset_confidence(c.confidence),
            None,
            &Utc::now().to_rfc3339(),
        )?;
        count += 1;
    }
    Ok(count)
}

fn asset_confidence(confidence: AssetConfidence) -> &'static str {
    match confidence {
        AssetConfidence::Exact => "exact",
        AssetConfidence::Identifier => "identifier",
        AssetConfidence::Fuzzy => "fuzzy",
    }
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
    low_confidence: bool,
) -> Result<(), DbError> {
    let Some(mut rf) = crud::get_rip_file(conn, rip_file_id)? else {
        return Ok(());
    };
    rf.disc_id = Some(disc_id);
    rf.identification_confidence = if low_confidence {
        IdentificationConfidence::Likely
    } else {
        IdentificationConfidence::Certain
    };
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
pub(crate) fn humanize_provider_error(name: &str, err: &ProviderError) -> IdentifyAttemptError {
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
        "tower" => IdentificationSource::Tower,
        other => IdentificationSource::Other(other.to_string()),
    })
}
