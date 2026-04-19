//! Fan-out behaviour: in-order collection and error isolation.
//!
//! Rate-limit coordination across providers that share a cloned
//! HttpClient is verified in an internal unit test (`src/tests/
//! fanout_tests.rs`) — `HttpClientBuilder::fake_host_quota` is
//! `#[cfg(test)]`-gated and only visible inside the crate.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use phono_junk_core::{DiscIds, Toc};
use phono_junk_identify::{
    Credentials, DiscIdKind, IdentificationProvider, ProviderError, ProviderResult,
    identify_parallel, spawn_all,
};

// -----------------------------------------------------------------------
// Mock identification provider — canned result, counts lookup calls.
// -----------------------------------------------------------------------

struct MockIdentifier {
    name: &'static str,
    outcome: MockOutcome,
}

enum MockOutcome {
    Some(ProviderResult),
    None,
    Error(&'static str),
}

impl MockIdentifier {
    fn new(name: &'static str, outcome: MockOutcome) -> Self {
        Self { name, outcome }
    }
}

impl IdentificationProvider for MockIdentifier {
    fn name(&self) -> &'static str {
        self.name
    }

    fn supported_ids(&self) -> &[DiscIdKind] {
        &[DiscIdKind::MbDiscId]
    }

    fn lookup(
        &self,
        _toc: &Toc,
        _ids: &DiscIds,
        _creds: &Credentials,
    ) -> Result<Option<ProviderResult>, ProviderError> {
        match &self.outcome {
            MockOutcome::Some(r) => Ok(Some(r.clone())),
            MockOutcome::None => Ok(None),
            MockOutcome::Error(msg) => Err(ProviderError::Other((*msg).to_string())),
        }
    }
}

fn default_toc() -> Toc {
    Toc {
        first_track: 1,
        last_track: 1,
        leadout_sector: 100,
        track_offsets: vec![0],
    }
}

fn discid_ids() -> DiscIds {
    DiscIds {
        mb_discid: Some("test-discid".into()),
        ..Default::default()
    }
}

// -----------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------

#[test]
fn all_ok_results_returned_in_provider_order() {
    let a: Box<dyn IdentificationProvider> = Box::new(MockIdentifier::new(
        "a",
        MockOutcome::Some(ProviderResult {
            provider: "a".into(),
            ..Default::default()
        }),
    ));
    let b: Box<dyn IdentificationProvider> = Box::new(MockIdentifier::new(
        "b",
        MockOutcome::Some(ProviderResult {
            provider: "b".into(),
            ..Default::default()
        }),
    ));
    let providers = vec![a, b];
    let results = identify_parallel(&providers, &default_toc(), &discid_ids(), &Credentials::new());
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, "a");
    assert_eq!(results[1].0, "b");
}

#[test]
fn one_failure_does_not_block_others() {
    let a: Box<dyn IdentificationProvider> = Box::new(MockIdentifier::new(
        "a",
        MockOutcome::Error("boom"),
    ));
    let b: Box<dyn IdentificationProvider> = Box::new(MockIdentifier::new(
        "b",
        MockOutcome::Some(ProviderResult {
            provider: "b".into(),
            ..Default::default()
        }),
    ));
    let providers = vec![a, b];
    let results = identify_parallel(&providers, &default_toc(), &discid_ids(), &Credentials::new());
    assert_eq!(results.len(), 2);
    assert!(results[0].1.is_err(), "first provider should be Err");
    assert!(results[1].1.is_ok(), "second provider should still succeed");
}

#[test]
fn filter_by_supported_ids_skips_non_applicable() {
    struct BarcodeOnly {
        calls: Arc<AtomicUsize>,
    }
    impl IdentificationProvider for BarcodeOnly {
        fn name(&self) -> &'static str {
            "barcode"
        }
        fn supported_ids(&self) -> &[DiscIdKind] {
            &[DiscIdKind::Barcode]
        }
        fn lookup(
            &self,
            _toc: &Toc,
            _ids: &DiscIds,
            _creds: &Credentials,
        ) -> Result<Option<ProviderResult>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let a: Box<dyn IdentificationProvider> = Box::new(BarcodeOnly {
        calls: calls.clone(),
    });
    let providers = vec![a];
    // discid_ids has no barcode — barcode-only provider should be skipped.
    let results = identify_parallel(&providers, &default_toc(), &discid_ids(), &Credentials::new());
    assert!(results.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn spawn_all_is_empty_on_empty_input() {
    let providers: Vec<&dyn IdentificationProvider> = Vec::new();
    let results: Vec<Result<(), ProviderError>> =
        spawn_all::<dyn IdentificationProvider, _, _>(&providers, |_p| Ok(()));
    assert!(results.is_empty());
}

#[test]
fn all_providers_returning_none_yields_all_nones() {
    let a: Box<dyn IdentificationProvider> =
        Box::new(MockIdentifier::new("a", MockOutcome::None));
    let b: Box<dyn IdentificationProvider> =
        Box::new(MockIdentifier::new("b", MockOutcome::None));
    let providers = vec![a, b];
    let results = identify_parallel(&providers, &default_toc(), &discid_ids(), &Credentials::new());
    assert_eq!(results.len(), 2);
    assert!(matches!(results[0].1, Ok(None)));
    assert!(matches!(results[1].1, Ok(None)));
}
