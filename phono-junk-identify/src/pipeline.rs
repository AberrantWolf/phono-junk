//! Staged provider execution and deterministic release-candidate scoring.

use std::collections::{HashMap, HashSet};

use phono_junk_core::{DiscIds, Toc};

use crate::{
    CandidateResolution, CandidateScore, Credentials, DiscIdKind, IdentificationProvider,
    ProviderError, ProviderLookup, ProviderTier, ReleaseCandidate, ScoredCandidate,
};

struct LookupTask<'a> {
    provider: &'a dyn IdentificationProvider,
    input_kind: DiscIdKind,
    input_value: String,
    ids: DiscIds,
}

#[derive(Debug, Clone)]
pub struct ProviderObservation {
    pub provider: String,
    pub input_kind: DiscIdKind,
    pub input_value: String,
    pub stage: u8,
    pub lookup: Option<ProviderLookup>,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct StagedIdentifyOutcome {
    pub resolution: Option<CandidateResolution>,
    pub observations: Vec<ProviderObservation>,
    pub errors: Vec<(String, ProviderError)>,
}

pub(crate) fn identify_staged(
    providers: &[Box<dyn IdentificationProvider>],
    toc: &Toc,
    initial_ids: &DiscIds,
    creds: &Credentials,
) -> StagedIdentifyOutcome {
    let mut ids = initial_ids.clone();
    let mut observations = Vec::new();
    let mut errors = Vec::new();
    let mut candidates = Vec::new();
    let mut queried: HashSet<(String, DiscIdKind, String)> = HashSet::new();

    for (stage, tier) in [
        ProviderTier::ExactDisc,
        ProviderTier::MusicApi,
        ProviderTier::MusicFallback,
        ProviderTier::GenericBarcode,
    ]
    .into_iter()
    .enumerate()
    {
        let current = score_and_resolve(toc, &candidates);
        if tier == ProviderTier::MusicFallback
            && current
                .as_ref()
                .is_some_and(|resolution| !resolution.evidentially_ambiguous)
        {
            continue;
        }
        if tier == ProviderTier::GenericBarcode && !candidates.is_empty() {
            continue;
        }

        let mut tasks = Vec::new();
        for provider in providers
            .iter()
            .map(|provider| provider.as_ref())
            .filter(|provider| provider.descriptor().tier == tier)
        {
            for &input_kind in provider.descriptor().required_ids {
                let Some(input_value) = input_value(input_kind, &ids) else {
                    continue;
                };
                let query_key = (provider.name().to_string(), input_kind, input_value.clone());
                if queried.insert(query_key) {
                    tasks.push(LookupTask {
                        provider,
                        input_kind,
                        input_value,
                        ids: ids_for_input(&ids, input_kind),
                    });
                }
            }
        }
        let results = std::thread::scope(|scope| {
            let handles: Vec<_> = tasks
                .iter()
                .map(|task| scope.spawn(|| task.provider.lookup_many(toc, &task.ids, creds)))
                .collect();
            handles
                .into_iter()
                .map(|handle| match handle.join() {
                    Ok(result) => result,
                    Err(_) => Err(ProviderError::Other("provider thread panicked".into())),
                })
                .collect::<Vec<_>>()
        });

        for (task, result) in tasks.into_iter().zip(results) {
            let provider = task.provider;
            match result {
                Ok(mut lookup) => {
                    learn_from_candidates(&mut lookup);
                    apply_learned_ids(&mut ids, &lookup);
                    for candidate in &lookup.release_candidates {
                        if !candidates.iter().any(|existing: &ReleaseCandidate| {
                            existing.provider == candidate.provider
                                && existing.candidate_key == candidate.candidate_key
                        }) {
                            candidates.push(candidate.clone());
                        }
                    }
                    observations.push(ProviderObservation {
                        provider: provider.name().to_string(),
                        input_kind: task.input_kind,
                        input_value: task.input_value,
                        stage: stage as u8 + 1,
                        lookup: Some(lookup),
                        error: None,
                    });
                }
                Err(error) => {
                    observations.push(ProviderObservation {
                        provider: provider.name().to_string(),
                        input_kind: task.input_kind,
                        input_value: task.input_value,
                        stage: stage as u8 + 1,
                        lookup: None,
                        error: Some(error.to_string()),
                    });
                    errors.push((provider.name().to_string(), error));
                }
            }
        }
    }

    StagedIdentifyOutcome {
        resolution: score_and_resolve(toc, &candidates),
        observations,
        errors,
    }
}

pub fn score_and_resolve(
    toc: &Toc,
    candidates: &[ReleaseCandidate],
) -> Option<CandidateResolution> {
    if candidates.is_empty() {
        return None;
    }
    let mut release_support: HashMap<String, HashSet<&str>> = HashMap::new();
    for candidate in candidates {
        release_support
            .entry(release_identity(candidate))
            .or_default()
            .insert(candidate.provider.as_str());
    }

    let mut scored: Vec<ScoredCandidate> = candidates
        .iter()
        .cloned()
        .map(|candidate| {
            let peers = candidates
                .iter()
                .filter(|other| other.candidate_key != candidate.candidate_key)
                .collect::<Vec<_>>();
            let exact_release_corroboration = candidate.release.mbid.as_ref().is_some_and(|id| {
                peers
                    .iter()
                    .any(|other| other.release.mbid.as_ref() == Some(id))
            }) as u8;
            let barcode_catalog_corroboration = peers.iter().any(|other| {
                same_nonempty(&candidate.release.barcode, &other.release.barcode)
                    && (same_nonempty(
                        &candidate.release.catalog_number,
                        &other.release.catalog_number,
                    ) || same_nonempty(&candidate.release.label, &other.release.label))
            }) as u8;
            let score = CandidateScore {
                exact_disc_association: candidate.exact_disc_association as u8,
                exact_release_corroboration,
                barcode_catalog_corroboration,
                music_provider_support: release_support
                    .get(&release_identity(&candidate))
                    .map_or(0, |providers| providers.len() as u8),
                track_duration_agreement: track_agreement(toc, &candidate),
                metadata_completeness: completeness(&candidate),
                provider_priority: provider_priority(&candidate.provider),
            };
            ScoredCandidate { candidate, score }
        })
        .collect();

    scored.sort_by(|left, right| {
        right.score.rank().cmp(&left.score.rank()).then_with(|| {
            left.candidate
                .candidate_key
                .cmp(&right.candidate.candidate_key)
        })
    });
    let selected = scored.remove(0);
    let evidentially_ambiguous = scored.first().is_some_and(|runner_up| {
        runner_up.score.evidence_components() == selected.score.evidence_components()
    });
    Some(CandidateResolution {
        selected,
        alternatives: scored,
        evidentially_ambiguous,
    })
}

fn input_value(kind: DiscIdKind, ids: &DiscIds) -> Option<String> {
    match kind {
        DiscIdKind::MbDiscId => ids.mb_discid.clone(),
        DiscIdKind::CddbId => ids.cddb_id.clone(),
        DiscIdKind::AccurateRipId => ids.ar_discid1.clone(),
        DiscIdKind::Barcode => ids.barcode.clone(),
        DiscIdKind::CatalogNumber => ids.catalog_number.clone(),
    }
}

/// Give a provider exactly the frontier identifier represented by this query.
/// Structural TOC IDs remain available because AccurateRip-style providers may
/// need their full tuple, but competing metadata keys are cleared so a
/// barcode/catalog provider cannot silently execute the same preferred lookup
/// twice.
fn ids_for_input(ids: &DiscIds, kind: DiscIdKind) -> DiscIds {
    let mut scoped = ids.clone();
    match kind {
        DiscIdKind::Barcode => scoped.catalog_number = None,
        DiscIdKind::CatalogNumber => scoped.barcode = None,
        DiscIdKind::MbDiscId | DiscIdKind::CddbId | DiscIdKind::AccurateRipId => {}
    }
    scoped
}

fn learn_from_candidates(lookup: &mut ProviderLookup) {
    for candidate in &lookup.release_candidates {
        for (kind, value) in [
            (DiscIdKind::Barcode, candidate.release.barcode.as_ref()),
            (
                DiscIdKind::CatalogNumber,
                candidate.release.catalog_number.as_ref(),
            ),
        ] {
            if let Some(value) = value
                && !lookup
                    .learned_ids
                    .iter()
                    .any(|learned| learned.kind == kind && learned.value == *value)
            {
                lookup.learned_ids.push(crate::LearnedExternalId {
                    kind,
                    value: value.clone(),
                });
            }
        }
    }
}

fn apply_learned_ids(ids: &mut DiscIds, lookup: &ProviderLookup) {
    for learned in &lookup.learned_ids {
        match learned.kind {
            DiscIdKind::Barcode if ids.barcode.is_none() => {
                ids.barcode = Some(learned.value.clone());
            }
            DiscIdKind::CatalogNumber if ids.catalog_number.is_none() => {
                ids.catalog_number = Some(learned.value.clone());
            }
            _ => {}
        }
    }
}

fn release_identity(candidate: &ReleaseCandidate) -> String {
    candidate
        .release
        .mbid
        .as_ref()
        .map(|id| format!("mb:{id}"))
        .or_else(|| {
            candidate.release.barcode.as_ref().map(|barcode| {
                format!(
                    "barcode:{barcode}:{}",
                    candidate.release.catalog_number.as_deref().unwrap_or("")
                )
            })
        })
        .unwrap_or_else(|| candidate.candidate_key.clone())
}

fn same_nonempty(left: &Option<String>, right: &Option<String>) -> bool {
    left.as_ref()
        .is_some_and(|left| !left.is_empty() && right.as_ref() == Some(left))
}

fn track_agreement(toc: &Toc, candidate: &ReleaseCandidate) -> u32 {
    let count = (candidate.tracks.len() == toc.track_count()) as u32;
    let durations = candidate
        .tracks
        .iter()
        .enumerate()
        .filter(|(index, track)| {
            track
                .length_frames
                .zip(toc.track_length_frames(*index))
                .is_some_and(|(actual, expected)| actual.abs_diff(expected) <= 2)
        })
        .count() as u32;
    count * 10_000 + durations
}

fn completeness(candidate: &ReleaseCandidate) -> u8 {
    [
        candidate.album.title.is_some(),
        candidate.album.artist_credit.is_some(),
        candidate.album.year.is_some(),
        candidate.release.country.is_some(),
        candidate.release.label.is_some(),
        candidate.release.catalog_number.is_some(),
        candidate.release.barcode.is_some(),
        !candidate.tracks.is_empty(),
    ]
    .into_iter()
    .map(u8::from)
    .sum()
}

fn provider_priority(provider: &str) -> u8 {
    match provider {
        "musicbrainz" => 4,
        "discogs" => 3,
        "tower" => 2,
        "barcodelookup" => 1,
        _ => 0,
    }
}
