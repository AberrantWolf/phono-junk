//! Parallel provider fan-out using `std::thread::scope`.
//!
//! Providers implement the `Send + Sync` trait bounds already (see
//! `IdentificationProvider` + `AssetProvider` supertraits), so no runtime
//! is needed — a scoped thread per provider is sufficient for the MVP
//! N ≈ 3 providers. If that ever changes we can slot in `rayon`, but not
//! today.
//!
//! **Error isolation.** One provider failing does not poison the batch.
//! Each spawned thread returns its own `Result`, and the aggregator
//! decides what to do with mixed results (typically: merge the `Ok`s,
//! surface the `Err`s in an `errors` list).
//!
//! **Rate-limit coordination.** Providers that share a host must also
//! share an [`crate::HttpClient`] clone so the per-host token bucket is
//! enforced across them. `PhonoContext::with_default_providers` does this.
//! This module just dispatches — bucket state lives in the client.

use crate::{
    AssetCandidate, AssetLookupCtx, AssetProvider, Credentials, DiscIdKind, IdentificationProvider,
    ProviderError, ProviderResult,
};
use phono_junk_core::{DiscIds, Toc};

/// Spawn one scoped thread per provider, collect results in input order.
///
/// Generic over the provider trait object type `P` and its result type
/// `T`. Used by both [`identify_parallel`] (where `P = dyn
/// IdentificationProvider` and `T = Option<ProviderResult>`) and
/// [`lookup_assets_parallel`] (where `P = dyn AssetProvider` and
/// `T = Vec<AssetCandidate>`).
pub fn spawn_all<P, T, F>(providers: &[&P], f: F) -> Vec<Result<T, ProviderError>>
where
    P: ?Sized + Sync,
    T: Send,
    F: Fn(&P) -> Result<T, ProviderError> + Sync,
{
    if providers.is_empty() {
        return Vec::new();
    }
    std::thread::scope(|s| {
        let handles: Vec<_> = providers
            .iter()
            .map(|p| s.spawn(|| f(*p)))
            .collect();
        handles
            .into_iter()
            .map(|h| match h.join() {
                Ok(r) => r,
                Err(_) => Err(ProviderError::Other("provider thread panicked".into())),
            })
            .collect()
    })
}

/// Fan out identification lookups across every provider whose
/// `supported_ids()` overlaps with the ids actually available. Each entry
/// in the returned vec is `(provider_name, result)`.
pub fn identify_parallel(
    providers: &[Box<dyn IdentificationProvider>],
    toc: &Toc,
    ids: &DiscIds,
    creds: &Credentials,
) -> Vec<(String, Result<Option<ProviderResult>, ProviderError>)> {
    let available = available_id_kinds(ids);
    let applicable: Vec<&dyn IdentificationProvider> = providers
        .iter()
        .map(|p| p.as_ref())
        .filter(|p| p.supported_ids().iter().any(|k| available.contains(k)))
        .collect();

    if applicable.is_empty() {
        return Vec::new();
    }
    let names: Vec<String> = applicable.iter().map(|p| p.name().to_string()).collect();
    let results = spawn_all::<dyn IdentificationProvider, _, _>(&applicable, |p| {
        p.lookup(toc, ids, creds)
    });
    names.into_iter().zip(results).collect()
}

/// Fan out asset lookups across every registered asset provider. Each
/// entry in the returned vec is `(provider_name, result)`.
pub fn lookup_assets_parallel(
    providers: &[Box<dyn AssetProvider>],
    ctx: &AssetLookupCtx<'_>,
) -> Vec<(String, Result<Vec<AssetCandidate>, ProviderError>)> {
    if providers.is_empty() {
        return Vec::new();
    }
    let refs: Vec<&dyn AssetProvider> = providers.iter().map(|p| p.as_ref()).collect();
    let names: Vec<String> = refs.iter().map(|p| p.name().to_string()).collect();
    let results = spawn_all::<dyn AssetProvider, _, _>(&refs, |p| p.lookup_art(ctx));
    names.into_iter().zip(results).collect()
}

/// Which [`DiscIdKind`]s are populated in `ids`. Used to pre-filter
/// providers whose `supported_ids()` would produce no match with the
/// data we actually have (e.g. skip a barcode-only provider when we
/// have no barcode).
fn available_id_kinds(ids: &DiscIds) -> Vec<DiscIdKind> {
    let mut out = Vec::new();
    if ids.mb_discid.is_some() {
        out.push(DiscIdKind::MbDiscId);
    }
    if ids.cddb_id.is_some() {
        out.push(DiscIdKind::CddbId);
    }
    if ids.ar_discid1.is_some() || ids.ar_discid2.is_some() {
        out.push(DiscIdKind::AccurateRipId);
    }
    if ids.barcode.is_some() {
        out.push(DiscIdKind::Barcode);
    }
    if ids.catalog_number.is_some() {
        out.push(DiscIdKind::CatalogNumber);
    }
    out
}

#[cfg(test)]
#[path = "tests/fanout_tests.rs"]
mod tests;
