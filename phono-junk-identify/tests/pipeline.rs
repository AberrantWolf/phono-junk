use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use phono_junk_core::{DiscIds, Toc};
use phono_junk_identify::{
    Aggregator, AlbumMeta, Credentials, DiscIdKind, IdentificationProvider, ProviderDescriptor,
    ProviderError, ProviderLookup, ProviderTier, ReleaseCandidate, ReleaseMeta, score_and_resolve,
};

struct MockProvider {
    name: &'static str,
    tier: ProviderTier,
    required: &'static [DiscIdKind],
    candidate: Option<ReleaseCandidate>,
    calls: Arc<AtomicUsize>,
}

impl IdentificationProvider for MockProvider {
    fn name(&self) -> &'static str {
        self.name
    }

    fn supported_ids(&self) -> &'static [DiscIdKind] {
        self.required
    }

    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            name: self.name,
            tier: self.tier,
            required_ids: self.required,
            emitted_ids: &[],
            identifies: true,
            supplies_assets: false,
            required_credential: None,
            host_rate_policy: None,
        }
    }

    fn lookup_many(
        &self,
        _toc: &Toc,
        _ids: &DiscIds,
        _creds: &Credentials,
    ) -> Result<ProviderLookup, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ProviderLookup {
            release_candidates: self.candidate.clone().into_iter().collect(),
            ..ProviderLookup::default()
        })
    }
}

fn candidate(provider: &str, key: &str, release_mbid: Option<&str>) -> ReleaseCandidate {
    ReleaseCandidate {
        candidate_key: key.into(),
        provider: provider.into(),
        album: AlbumMeta {
            title: Some("Album".into()),
            artist_credit: Some("Artist".into()),
            ..AlbumMeta::default()
        },
        release: ReleaseMeta {
            mbid: release_mbid.map(str::to_string),
            barcode: Some("123456789012".into()),
            catalog_number: Some("CAT-1".into()),
            label: Some("Label".into()),
            ..ReleaseMeta::default()
        },
        tracks: Vec::new(),
        physical_disc_number: Some(1),
        exact_disc_association: provider == "musicbrainz",
        raw_response: None,
        cover_art_urls: Vec::new(),
    }
}

fn toc() -> Toc {
    Toc {
        first_track: 1,
        last_track: 1,
        leadout_sector: 10_000,
        track_offsets: vec![150],
    }
}

#[test]
fn learned_barcode_unlocks_music_api_and_clean_resolution_skips_fallbacks() {
    let mb_calls = Arc::new(AtomicUsize::new(0));
    let discogs_calls = Arc::new(AtomicUsize::new(0));
    let tower_calls = Arc::new(AtomicUsize::new(0));
    let generic_calls = Arc::new(AtomicUsize::new(0));
    let mut aggregator = Aggregator::new();
    aggregator.register_identifier(Box::new(MockProvider {
        name: "musicbrainz",
        tier: ProviderTier::ExactDisc,
        required: &[DiscIdKind::MbDiscId],
        candidate: Some(candidate("musicbrainz", "mb", Some("release-1"))),
        calls: mb_calls.clone(),
    }));
    aggregator.register_identifier(Box::new(MockProvider {
        name: "discogs",
        tier: ProviderTier::MusicApi,
        required: &[DiscIdKind::Barcode],
        candidate: Some(candidate("discogs", "discogs", Some("release-1"))),
        calls: discogs_calls.clone(),
    }));
    aggregator.register_identifier(Box::new(MockProvider {
        name: "tower",
        tier: ProviderTier::MusicFallback,
        required: &[DiscIdKind::Barcode],
        candidate: Some(candidate("tower", "tower", None)),
        calls: tower_calls.clone(),
    }));
    aggregator.register_identifier(Box::new(MockProvider {
        name: "barcodelookup",
        tier: ProviderTier::GenericBarcode,
        required: &[DiscIdKind::Barcode],
        candidate: Some(candidate("barcodelookup", "generic", None)),
        calls: generic_calls.clone(),
    }));

    let outcome = aggregator.identify_staged(
        &toc(),
        &DiscIds {
            mb_discid: Some("disc-id".into()),
            ..DiscIds::default()
        },
        &Credentials::new(),
    );
    assert!(outcome.resolution.is_some());
    assert_eq!(mb_calls.load(Ordering::SeqCst), 1);
    assert_eq!(discogs_calls.load(Ordering::SeqCst), 1);
    assert_eq!(tower_calls.load(Ordering::SeqCst), 0);
    assert_eq!(generic_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn evidential_tie_is_deterministic_and_explicit() {
    let candidates = vec![
        candidate("discogs", "z-key", None),
        candidate("tower", "a-key", None),
    ];
    let resolution = score_and_resolve(&toc(), &candidates).unwrap();
    assert!(resolution.evidentially_ambiguous);
    assert_eq!(resolution.selected.candidate.provider, "discogs");
    assert_eq!(resolution.alternatives.len(), 1);
}

#[test]
fn each_provider_identifier_pair_is_queried_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut aggregator = Aggregator::new();
    aggregator.register_identifier(Box::new(MockProvider {
        name: "discogs",
        tier: ProviderTier::MusicApi,
        required: &[DiscIdKind::Barcode, DiscIdKind::CatalogNumber],
        candidate: None,
        calls: calls.clone(),
    }));

    let outcome = aggregator.identify_staged(
        &toc(),
        &DiscIds {
            barcode: Some("123456789012".into()),
            catalog_number: Some("CAT-1".into()),
            ..DiscIds::default()
        },
        &Credentials::new(),
    );

    assert!(outcome.resolution.is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(outcome.observations.len(), 2);
    assert_eq!(outcome.observations[0].input_kind, DiscIdKind::Barcode);
    assert_eq!(
        outcome.observations[1].input_kind,
        DiscIdKind::CatalogNumber
    );
}
