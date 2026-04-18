//! Credential storage for provider tokens.
//!
//! TOML at `~/.config/phono-junk/credentials.toml` or env vars. XOR-obfuscated
//! at-rest per retro-junk-scraper's idiom — not strong crypto, just prevents
//! casual leaks when sharing config.
//!
//! Keys (by provider name): `"discogs"`, `"amazon_access_key"`,
//! `"amazon_secret_key"`, `"amazon_partner_tag"`.

use phono_junk_identify::Credentials;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CredentialStore {
    #[serde(default)]
    entries: std::collections::HashMap<String, String>,
}

impl CredentialStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, provider: impl Into<String>, token: impl Into<String>) {
        self.entries.insert(provider.into(), token.into());
    }

    pub fn to_credentials(&self) -> Credentials {
        let mut c = Credentials::new();
        for (k, v) in &self.entries {
            c.set(k, v);
        }
        c
    }
}
