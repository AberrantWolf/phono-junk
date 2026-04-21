//! Credential storage for provider tokens.
//!
//! Backed by the OS keyring ([`keyring`] crate) — macOS Keychain, Windows
//! Credential Manager, Linux Secret Service. The in-memory map is the
//! runtime source of truth for provider lookups; the keyring is the
//! persistence tier. Writers update both; readers pull from the in-memory
//! map after [`CredentialStore::load_from_keyring`] has populated it.
//!
//! Interior mutability ([`std::sync::RwLock`]) lets the GUI update a
//! token on a shared `Arc<PhonoContext>` without rebuilding the context
//! — providers see the new value on the next `to_credentials()` snapshot.
//!
//! Keyring unavailability (no backend, e.g. headless CI or a Linux box
//! without Secret Service) is **non-fatal**: we log a warning and keep
//! running with in-memory-only storage. The token will not survive a
//! restart, but identification still works for the current session.
//!
//! Known provider keys: `"discogs"` (Sprint 28), `"barcodelookup"`.
//! Future: `"amazon"` etc.
//!
//! ## Debug safety
//!
//! `CredentialStore`'s `Debug` impl emits provider *keys* only — tokens
//! never stringify. Same rule applies to the [`phono_junk_identify::Credentials`]
//! snapshot produced by [`CredentialStore::to_credentials`].

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use phono_junk_identify::Credentials;

/// Keyring service identifier. All provider tokens live under this
/// service so `credentials list`-style operations can enumerate us
/// without scanning every app's entries.
pub const KEYRING_SERVICE: &str = "phono-junk";

/// Providers whose tokens we try to load from the keyring on startup.
/// Listed explicitly rather than scanning all entries because the
/// keyring API doesn't expose enumeration on every backend.
pub const KNOWN_PROVIDERS: &[&str] = &["discogs", "barcodelookup"];

/// Error surface for keyring-backed operations. In-memory operations
/// are infallible.
#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    /// No keyring backend is available on this system. The in-memory
    /// store still works; persistence is the thing that fails.
    #[error("keyring unavailable ({0}) — token will not persist across restarts")]
    Unavailable(String),
    /// Any other keyring error. Includes encoding issues, platform
    /// failures, and permission problems.
    #[error("keyring: {0}")]
    Other(String),
}

fn map_keyring_err(e: keyring::Error) -> CredentialError {
    use keyring::Error;
    match e {
        Error::NoStorageAccess(inner) => CredentialError::Unavailable(inner.to_string()),
        Error::PlatformFailure(inner) => CredentialError::Unavailable(inner.to_string()),
        other => CredentialError::Other(other.to_string()),
    }
}

/// Clone-cheap (Arc) shared token store. Mutating operations take
/// `&self` because the inner map sits behind a [`RwLock`].
#[derive(Clone, Default)]
pub struct CredentialStore {
    entries: Arc<RwLock<HashMap<String, String>>>,
}

impl CredentialStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store `token` in-memory only. Use [`Self::store_to_keyring`] to
    /// persist across restarts.
    pub fn set(&self, provider: impl Into<String>, token: impl Into<String>) {
        let mut guard = self.entries.write().expect("credential lock poisoned");
        guard.insert(provider.into(), token.into());
    }

    /// Drop an in-memory credential. Does not touch the keyring.
    pub fn clear(&self, provider: &str) {
        let mut guard = self.entries.write().expect("credential lock poisoned");
        guard.remove(provider);
    }

    /// Return an owned clone of the token if set.
    pub fn get(&self, provider: &str) -> Option<String> {
        let guard = self.entries.read().expect("credential lock poisoned");
        guard.get(provider).cloned()
    }

    /// Whether a token is currently set (in-memory).
    pub fn has(&self, provider: &str) -> bool {
        let guard = self.entries.read().expect("credential lock poisoned");
        guard.contains_key(provider)
    }

    /// Enumerate the provider keys currently present in memory. Used by
    /// the `credentials list` CLI subcommand. Never returns token values.
    pub fn provider_names(&self) -> Vec<String> {
        let guard = self.entries.read().expect("credential lock poisoned");
        let mut v: Vec<String> = guard.keys().cloned().collect();
        v.sort_unstable();
        v
    }

    /// Snapshot the in-memory map into a frozen [`Credentials`] suitable
    /// for passing to providers during identify / asset fan-out. Cheap
    /// clone of each token string — fan-out is call-site-scoped, so the
    /// snapshot stays consistent across parallel provider calls even if
    /// the GUI writes a new token mid-run.
    pub fn to_credentials(&self) -> Credentials {
        let mut c = Credentials::new();
        let guard = self.entries.read().expect("credential lock poisoned");
        for (k, v) in guard.iter() {
            c.set(k, v);
        }
        c
    }

    /// Pull tokens for [`KNOWN_PROVIDERS`] out of the OS keyring into
    /// memory. Missing entries are not errors. Backend unavailability
    /// is logged and returned as [`CredentialError::Unavailable`] so
    /// the caller can surface a GUI hint if useful — but the in-memory
    /// store remains usable either way.
    ///
    /// Respects the `PHONO_SKIP_KEYRING` env var — when set, this is a
    /// no-op that returns `Ok(())`. Centralised here so every call site
    /// (GUI startup, CLI subcommands, tests, diagnostics) inherits the
    /// same opt-out without repeated env-var checks.
    pub fn load_from_keyring(&self) -> Result<(), CredentialError> {
        if std::env::var_os("PHONO_SKIP_KEYRING").is_some() {
            log::debug!("credentials: PHONO_SKIP_KEYRING set, skipping keyring load");
            return Ok(());
        }
        let mut unavailable: Option<CredentialError> = None;
        for provider in KNOWN_PROVIDERS {
            match keyring::Entry::new(KEYRING_SERVICE, provider) {
                Ok(entry) => match entry.get_password() {
                    Ok(token) => {
                        self.set(*provider, token);
                        log::info!("credentials: loaded {provider} from keyring");
                    }
                    Err(keyring::Error::NoEntry) => {
                        log::debug!("credentials: no keyring entry for {provider}");
                    }
                    Err(e) => {
                        let mapped = map_keyring_err(e);
                        if matches!(mapped, CredentialError::Unavailable(_)) {
                            log::warn!("credentials: keyring unavailable: {mapped}");
                            unavailable = Some(mapped);
                            break;
                        }
                        log::warn!("credentials: read {provider}: {mapped}");
                    }
                },
                Err(e) => {
                    let mapped = map_keyring_err(e);
                    log::warn!("credentials: open keyring entry {provider}: {mapped}");
                    if matches!(mapped, CredentialError::Unavailable(_)) {
                        unavailable = Some(mapped);
                        break;
                    }
                }
            }
        }
        match unavailable {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Persist `token` to the keyring and update in-memory. On keyring
    /// failure the in-memory store is still updated — identification
    /// keeps working for the current session — but the caller learns
    /// the token won't survive a restart.
    pub fn store_to_keyring(
        &self,
        provider: &str,
        token: &str,
    ) -> Result<(), CredentialError> {
        self.set(provider, token);
        let entry = keyring::Entry::new(KEYRING_SERVICE, provider).map_err(map_keyring_err)?;
        entry.set_password(token).map_err(map_keyring_err)?;
        Ok(())
    }

    /// Remove `provider` from both the keyring and the in-memory store.
    /// A missing keyring entry is treated as success (idempotent clear).
    pub fn clear_from_keyring(&self, provider: &str) -> Result<(), CredentialError> {
        self.clear(provider);
        let entry = match keyring::Entry::new(KEYRING_SERVICE, provider) {
            Ok(e) => e,
            Err(e) => return Err(map_keyring_err(e)),
        };
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(map_keyring_err(e)),
        }
    }
}

impl std::fmt::Debug for CredentialStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialStore")
            .field("providers", &self.provider_names())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Install an in-memory keyring backend for every keyring operation
    /// that runs inside this test process. Safe to call more than once —
    /// the builder just overwrites any previously-installed mock.
    fn install_mock_keyring() {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
    }

    #[test]
    fn set_get_clear_round_trip() {
        let store = CredentialStore::new();
        assert!(!store.has("discogs"));
        store.set("discogs", "hunter2");
        assert_eq!(store.get("discogs").as_deref(), Some("hunter2"));
        assert!(store.has("discogs"));
        store.clear("discogs");
        assert!(!store.has("discogs"));
    }

    #[test]
    fn debug_output_does_not_contain_tokens() {
        let store = CredentialStore::new();
        store.set("discogs", "hunter2");
        let dbg = format!("{:?}", store);
        assert!(!dbg.contains("hunter2"), "debug leaked secret: {dbg}");
        assert!(dbg.contains("discogs"), "debug missing provider key: {dbg}");
    }

    #[test]
    fn credentials_debug_does_not_contain_tokens() {
        let store = CredentialStore::new();
        store.set("discogs", "super-secret-xyz");
        let creds = store.to_credentials();
        let dbg = format!("{:?}", creds);
        assert!(!dbg.contains("super-secret-xyz"), "creds debug leaked: {dbg}");
        assert!(dbg.contains("discogs"), "creds debug missing key: {dbg}");
    }

    #[test]
    fn interior_mutation_visible_via_cloned_store() {
        // Same semantics as sharing via Arc<PhonoContext>: the GUI
        // writes a token on a clone and providers see it via the original.
        let store = CredentialStore::new();
        let alias = store.clone();
        alias.set("discogs", "t1");
        assert_eq!(store.get("discogs").as_deref(), Some("t1"));
    }

    #[test]
    fn store_to_keyring_updates_in_memory() {
        // keyring's `mock` module builds a fresh, independent
        // `MockCredential` per `Entry::new` call, so we can't
        // exercise cross-entry persistence here. What we can verify:
        // `store_to_keyring` writes to the in-memory store (visible
        // to providers immediately), and the keyring write doesn't
        // error on the mock backend.
        install_mock_keyring();
        let store = CredentialStore::new();
        store.store_to_keyring("discogs", "mock-token").unwrap();
        assert_eq!(store.get("discogs").as_deref(), Some("mock-token"));
    }

    #[test]
    fn clear_from_keyring_is_idempotent_and_clears_memory() {
        install_mock_keyring();
        let store = CredentialStore::new();
        // No entry yet — still Ok.
        store.clear_from_keyring("never-set").unwrap();

        store.set("discogs", "to-be-cleared");
        store.clear_from_keyring("discogs").unwrap();
        assert!(!store.has("discogs"));
    }

    #[test]
    fn load_from_keyring_populates_nothing_when_entries_missing() {
        install_mock_keyring();
        let store = CredentialStore::new();
        // MockCredential on fresh Entry::new has no stored password —
        // load should succeed with zero populated providers.
        store.load_from_keyring().unwrap();
        assert!(store.provider_names().is_empty());
    }

    #[test]
    fn provider_names_are_sorted_and_unique() {
        let store = CredentialStore::new();
        store.set("zulu", "t");
        store.set("alpha", "t");
        store.set("mike", "t");
        assert_eq!(store.provider_names(), vec!["alpha", "mike", "zulu"]);
    }
}
