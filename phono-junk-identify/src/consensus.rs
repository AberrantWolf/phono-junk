//! Field-level consensus across provider results.
//!
//! Takes a slice of [`ProviderResult`] (one per provider, in registration
//! order) and produces a [`MergedDisc`]: one winning value per field plus
//! a list of [`RawDisagreement`] rows for every conflict.
//!
//! **Consensus policy.** Per-field: plurality of non-None values, ties
//! broken by registration order. With two providers this degenerates to
//! "first non-None wins"; with three+ the majority naturally wins. This
//! is the MVP default — a configurable per-field policy is out of scope
//! and tracked in TODO.md ("Consensus-policy UI").
//!
//! **MBID-cohort rule** (correctness, not style). We pick the winning
//! `album.mbid` first. Any provider whose `album.mbid` is `Some(x)` with
//! `x != winner` is excluded from merging other album/track fields (its
//! disagreement is still recorded). Same for `release.mbid`. Rationale:
//! two MBIDs are two different releases — mixing their track lists onto
//! one `Disc` corrupts the catalog.
//!
//! Pure function — no HTTP, no DB. Side effects (writing `Disagreement`
//! rows to SQLite) live in `phono-junk-lib::identify`.

use std::collections::HashMap;

use phono_junk_core::Toc;
use serde::Serialize;

use crate::{AlbumMeta, ProviderResult, ReleaseMeta, TrackMeta};

/// Which catalog entity a disagreement refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum DisagreementEntity {
    Album,
    Release,
    Track { position: u8 },
}

/// A field-level conflict recorded at merge time, not yet attached to a
/// DB entity ID. The [`crate::Aggregator`]'s facade translates this to a
/// `phono_junk_catalog::Disagreement` once the entity rows have been
/// inserted and real `entity_id`s are available.
#[derive(Debug, Clone, Serialize)]
pub struct RawDisagreement {
    pub entity: DisagreementEntity,
    pub field: &'static str,
    pub source_a: String,
    pub value_a: serde_json::Value,
    pub source_b: String,
    pub value_b: serde_json::Value,
}

/// Result of merging per-provider [`ProviderResult`]s.
#[derive(Debug, Clone, Default)]
pub struct MergedDisc {
    pub album: AlbumMeta,
    pub release: ReleaseMeta,
    pub tracks: Vec<TrackMeta>,
    pub disagreements: Vec<RawDisagreement>,
    /// Provider names, in input order, that contributed at least one
    /// non-None field to the merged result (the MBID cohort).
    pub sources: Vec<String>,
}

/// Merge `results` into one [`MergedDisc`]. `results` is assumed to be in
/// registration priority order — earlier providers win ties.
pub fn merge(results: &[ProviderResult]) -> MergedDisc {
    let mut disagreements = Vec::new();

    // Step 1: MBID-cohort filtering. Pick winning album.mbid / release.mbid,
    // drop providers whose explicit MBID disagrees with the winner.
    let album_mbid = pick_mbid(
        results,
        |r| r.album.as_ref().and_then(|a| a.mbid.as_deref()),
        DisagreementEntity::Album,
        "album.mbid",
        &mut disagreements,
    );
    let release_mbid = pick_mbid(
        results,
        |r| r.release.as_ref().and_then(|a| a.mbid.as_deref()),
        DisagreementEntity::Release,
        "release.mbid",
        &mut disagreements,
    );

    let cohort: Vec<&ProviderResult> = results
        .iter()
        .filter(|r| {
            let a_ok = match (&album_mbid, r.album.as_ref().and_then(|a| a.mbid.as_deref())) {
                (Some(w), Some(x)) => w == x,
                _ => true, // winner is None, or this provider had no MBID → stays in cohort
            };
            let r_ok = match (
                &release_mbid,
                r.release.as_ref().and_then(|r| r.mbid.as_deref()),
            ) {
                (Some(w), Some(x)) => w == x,
                _ => true,
            };
            a_ok && r_ok
        })
        .collect();

    // Step 2: per-field merge inside the cohort.
    let album_values = |f: fn(&AlbumMeta) -> Option<String>| -> Vec<(String, Option<String>)> {
        cohort
            .iter()
            .map(|r| {
                (
                    r.provider.clone(),
                    r.album.as_ref().and_then(f),
                )
            })
            .collect()
    };
    let album_values_u16 = |f: fn(&AlbumMeta) -> Option<u16>| -> Vec<(String, Option<u16>)> {
        cohort
            .iter()
            .map(|r| (r.provider.clone(), r.album.as_ref().and_then(f)))
            .collect()
    };

    let mut album = AlbumMeta {
        mbid: album_mbid,
        title: merge_opt(
            "album.title",
            DisagreementEntity::Album,
            &album_values(|a| a.title.clone()),
            &mut disagreements,
        ),
        artist_credit: merge_opt(
            "album.artist_credit",
            DisagreementEntity::Album,
            &album_values(|a| a.artist_credit.clone()),
            &mut disagreements,
        ),
        year: merge_opt(
            "album.year",
            DisagreementEntity::Album,
            &album_values_u16(|a| a.year),
            &mut disagreements,
        ),
    };
    // Avoid empty-string titles slipping through from bad provider data.
    if album.title.as_deref() == Some("") {
        album.title = None;
    }

    let rel_values = |f: fn(&ReleaseMeta) -> Option<String>| -> Vec<(String, Option<String>)> {
        cohort
            .iter()
            .map(|r| (r.provider.clone(), r.release.as_ref().and_then(f)))
            .collect()
    };
    let release = ReleaseMeta {
        mbid: release_mbid,
        country: merge_opt(
            "release.country",
            DisagreementEntity::Release,
            &rel_values(|r| r.country.clone()),
            &mut disagreements,
        ),
        date: merge_opt(
            "release.date",
            DisagreementEntity::Release,
            &rel_values(|r| r.date.clone()),
            &mut disagreements,
        ),
        label: merge_opt(
            "release.label",
            DisagreementEntity::Release,
            &rel_values(|r| r.label.clone()),
            &mut disagreements,
        ),
        catalog_number: merge_opt(
            "release.catalog_number",
            DisagreementEntity::Release,
            &rel_values(|r| r.catalog_number.clone()),
            &mut disagreements,
        ),
        barcode: merge_opt(
            "release.barcode",
            DisagreementEntity::Release,
            &rel_values(|r| r.barcode.clone()),
            &mut disagreements,
        ),
        language: merge_opt(
            "release.language",
            DisagreementEntity::Release,
            &rel_values(|r| r.language.clone()),
            &mut disagreements,
        ),
        script: merge_opt(
            "release.script",
            DisagreementEntity::Release,
            &rel_values(|r| r.script.clone()),
            &mut disagreements,
        ),
    };

    let tracks = merge_tracks(&cohort, &mut disagreements);

    let sources: Vec<String> = cohort.iter().map(|r| r.provider.clone()).collect();

    MergedDisc {
        album,
        release,
        tracks,
        disagreements,
        sources,
    }
}

/// Pick the winning MBID across all providers (not just the cohort — MBID
/// is what defines the cohort). Records disagreements for losing MBIDs.
fn pick_mbid<'a, F>(
    results: &'a [ProviderResult],
    extract: F,
    entity: DisagreementEntity,
    field: &'static str,
    sink: &mut Vec<RawDisagreement>,
) -> Option<String>
where
    F: Fn(&'a ProviderResult) -> Option<&'a str>,
{
    let values: Vec<(String, Option<String>)> = results
        .iter()
        .map(|r| (r.provider.clone(), extract(r).map(str::to_string)))
        .collect();
    merge_opt(field, entity, &values, sink)
}

/// Per-field plurality-with-priority merge.
///
/// `values` is `(provider_name, optional_value)` pairs in registration
/// order. Returns the winning value (plurality count, ties broken by order)
/// or `None` if every value is `None`. Every non-winning distinct value
/// becomes a [`RawDisagreement`] sourced from the highest-priority
/// provider that produced it; the "A" side of the disagreement is always
/// the winning provider + winning value.
pub(crate) fn merge_opt<T: Clone + Eq + std::hash::Hash + Serialize>(
    field: &'static str,
    entity: DisagreementEntity,
    values: &[(String, Option<T>)],
    sink: &mut Vec<RawDisagreement>,
) -> Option<T> {
    // Vote: value → (count, first_provider_name)
    let mut votes: HashMap<&T, (usize, &str, usize)> = HashMap::new();
    for (idx, (provider, v)) in values.iter().enumerate() {
        if let Some(v) = v {
            votes
                .entry(v)
                .and_modify(|(count, _, _)| *count += 1)
                .or_insert((1, provider.as_str(), idx));
        }
    }
    if votes.is_empty() {
        return None;
    }
    // Winner: highest count, break ties by earliest first_idx.
    let (winner_val, (_, winner_provider, _)) = votes
        .iter()
        .max_by(|(_, (ca, _, ia)), (_, (cb, _, ib))| ca.cmp(cb).then(ib.cmp(ia)))
        .expect("non-empty");
    let winner_val = (*winner_val).clone();
    let winner_provider = winner_provider.to_string();

    for (provider, v) in values {
        let Some(v) = v else { continue };
        if v == &winner_val {
            continue;
        }
        sink.push(RawDisagreement {
            entity,
            field,
            source_a: winner_provider.clone(),
            value_a: serde_json::to_value(&winner_val).unwrap_or(serde_json::Value::Null),
            source_b: provider.clone(),
            value_b: serde_json::to_value(v).unwrap_or(serde_json::Value::Null),
        });
    }
    Some(winner_val)
}

/// Consensus merge with a TOC-derived fallback for the tracklist.
///
/// Runs [`merge`] first; if the resulting `tracks` vector is empty (every
/// provider returned zero track entries — typical when only a barcode-keyed
/// provider like Discogs matched), synthesises one stub `TrackMeta` per
/// TOC entry with `length_frames` populated and all text fields `None`.
///
/// The trigger is strictly "no tracks at all". A partial provider return
/// (e.g. MB lists 2 of 3 tracks) is a distinct failure mode — silently
/// padding it would hide the bug. See TODO.md's Sprint 19 section for the
/// partial-return policy follow-up.
pub fn merge_with_toc_fallback(results: &[ProviderResult], toc: &Toc) -> MergedDisc {
    let mut merged = merge(results);
    if merged.tracks.is_empty() {
        merged.tracks = toc
            .iter_track_spans()
            .map(|s| TrackMeta {
                position: s.position,
                title: None,
                artist_credit: None,
                length_frames: Some(s.length_frames),
                isrc: None,
                mbid: None,
            })
            .collect();
    }
    merged
}

/// Merge tracks across the cohort, keyed by `position`. Tracks present in
/// only some providers are kept as-is; conflicting fields at the same
/// position produce per-field disagreements tagged with that position.
fn merge_tracks(
    cohort: &[&ProviderResult],
    sink: &mut Vec<RawDisagreement>,
) -> Vec<TrackMeta> {
    // Collect per-position -> Vec<(provider, &TrackMeta)> in priority order.
    let mut by_pos: HashMap<u8, Vec<(String, &TrackMeta)>> = HashMap::new();
    let mut ordered_positions: Vec<u8> = Vec::new();
    for r in cohort {
        for t in &r.tracks {
            if !by_pos.contains_key(&t.position) {
                ordered_positions.push(t.position);
            }
            by_pos
                .entry(t.position)
                .or_default()
                .push((r.provider.clone(), t));
        }
    }
    ordered_positions.sort_unstable();
    ordered_positions.dedup();

    let mut out = Vec::with_capacity(ordered_positions.len());
    for pos in ordered_positions {
        let entries = &by_pos[&pos];
        let entity = DisagreementEntity::Track { position: pos };

        let values_string =
            |f: fn(&TrackMeta) -> Option<String>| -> Vec<(String, Option<String>)> {
                entries.iter().map(|(p, t)| (p.clone(), f(t))).collect()
            };
        let values_u64 = |f: fn(&TrackMeta) -> Option<u64>| -> Vec<(String, Option<u64>)> {
            entries.iter().map(|(p, t)| (p.clone(), f(t))).collect()
        };

        out.push(TrackMeta {
            position: pos,
            title: merge_opt(
                "track.title",
                entity,
                &values_string(|t| t.title.clone()),
                sink,
            ),
            artist_credit: merge_opt(
                "track.artist_credit",
                entity,
                &values_string(|t| t.artist_credit.clone()),
                sink,
            ),
            length_frames: merge_opt(
                "track.length_frames",
                entity,
                &values_u64(|t| t.length_frames),
                sink,
            ),
            isrc: merge_opt(
                "track.isrc",
                entity,
                &values_string(|t| t.isrc.clone()),
                sink,
            ),
            mbid: merge_opt(
                "track.mbid",
                entity,
                &values_string(|t| t.mbid.clone()),
                sink,
            ),
        });
    }
    out
}
