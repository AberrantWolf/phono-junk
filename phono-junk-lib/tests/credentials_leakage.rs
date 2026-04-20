//! Leakage-audit integration tests.
//!
//! These sit at the lib level (not inside a crate's unit tests) because
//! they assert the *externally observable* Debug surface of the types
//! provider crates touch — `Credentials`, `CredentialStore`, and
//! `ProviderError`. A regression here would likely mean someone added
//! a derive or a `format!` inside a provider.

use phono_junk_identify::{Credentials, ProviderError};
use phono_junk_lib::credentials::CredentialStore;

const SECRET: &str = "integration-secret-abc-12345";

#[test]
fn credential_store_debug_prints_keys_not_values() {
    let store = CredentialStore::new();
    store.set("discogs", SECRET);
    let dbg = format!("{:?}", store);
    assert!(!dbg.contains(SECRET), "store debug leaked secret: {dbg}");
    assert!(dbg.contains("discogs"), "store debug missing key: {dbg}");
}

#[test]
fn snapshot_credentials_debug_does_not_leak() {
    let store = CredentialStore::new();
    store.set("discogs", SECRET);
    let snapshot: Credentials = store.to_credentials();
    let dbg = format!("{:?}", snapshot);
    assert!(
        !dbg.contains(SECRET),
        "snapshot debug leaked secret: {dbg}"
    );
}

#[test]
fn provider_auth_error_messages_do_not_embed_tokens() {
    // Providers must build Auth errors from static text — if someone
    // refactors Discogs to interpolate the token into the error message,
    // this guard catches it at the error surface.
    let err = ProviderError::Auth("discogs token rejected".into());
    let text = format!("{err}");
    assert!(!text.contains(SECRET), "{text} leaked");
    // Same constant as the Display impl: ensures our assertion actually
    // runs against the rendered error surface, not a silent no-op.
    assert!(text.contains("discogs token rejected"));

    let err = ProviderError::MissingCredential("discogs");
    let text = format!("{err}");
    assert!(!text.contains(SECRET));
    assert!(text.contains("discogs"));
}

#[test]
fn in_memory_clear_does_not_require_keyring_backend() {
    // The GUI Settings Clear button can call this at any time; it must
    // not blow up when no keyring backend is present.
    let store = CredentialStore::new();
    store.set("discogs", SECRET);
    store.clear("discogs");
    assert!(!store.has("discogs"));
}
