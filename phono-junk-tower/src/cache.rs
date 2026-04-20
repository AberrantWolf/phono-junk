//! On-disk response cache for Tower MDB pages.
//!
//! Tower is a scrape target, not an API — release pages change rarely
//! (30-day TTL) and barcode searches are cheap to re-fetch (7-day TTL,
//! incl. negative-cache on empty results). 5xx / transport errors
//! bypass the cache entirely so transient failures aren't persisted.
//!
//! Cache key is the SHA-1 of the request URL, rendered as hex. Sidecar
//! `.meta` JSON carries fetch timestamp, status, and URL for forensic
//! inspection. TTL is checked against the meta file's mtime.
//!
//! Scope: deliberately inside this crate until a second scraper provider
//! needs the same mechanism. See TODO.md Cross-repo section for the
//! extraction trigger.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use phono_junk_identify::{HttpError, HttpResponse};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

/// Which kind of page is being cached — drives TTL and subdir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheKind {
    Release,
    Search,
}

impl CacheKind {
    fn subdir(self) -> &'static str {
        match self {
            CacheKind::Release => "release",
            CacheKind::Search => "search",
        }
    }
}

/// On-disk response cache with per-kind TTL.
#[derive(Debug, Clone)]
pub struct ResponseCache {
    root: PathBuf,
    release_ttl: Duration,
    search_ttl: Duration,
}

const DEFAULT_RELEASE_TTL: Duration = Duration::from_secs(60 * 60 * 24 * 30);
const DEFAULT_SEARCH_TTL: Duration = Duration::from_secs(60 * 60 * 24 * 7);

#[derive(Serialize, Deserialize)]
struct CacheMeta {
    url: String,
    status: u16,
    fetched_at_unix: u64,
    content_type: Option<String>,
}

impl ResponseCache {
    /// Construct a cache rooted at a platform-appropriate cache dir under
    /// `{cache_home}/phono-junk/{provider}/`. Honors `XDG_CACHE_HOME`;
    /// falls back to `$HOME/.cache` on Unix, `~/Library/Caches` on macOS,
    /// `%LOCALAPPDATA%` on Windows.
    pub fn default_for(provider: &str) -> io::Result<Self> {
        let base = platform_cache_root()?;
        let root = base.join("phono-junk").join(provider);
        Ok(Self::with_root(root))
    }

    /// Construct with an explicit root. Mainly for tests pointing at
    /// `tempfile::tempdir()`.
    pub fn with_root(root: PathBuf) -> Self {
        Self {
            root,
            release_ttl: DEFAULT_RELEASE_TTL,
            search_ttl: DEFAULT_SEARCH_TTL,
        }
    }

    /// Override TTLs. Useful for tests and for tuning.
    pub fn with_ttls(mut self, release: Duration, search: Duration) -> Self {
        self.release_ttl = release;
        self.search_ttl = search;
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn ttl_for(&self, kind: CacheKind) -> Duration {
        match kind {
            CacheKind::Release => self.release_ttl,
            CacheKind::Search => self.search_ttl,
        }
    }

    fn body_path(&self, kind: CacheKind, key: &str) -> PathBuf {
        self.root.join(kind.subdir()).join(format!("{key}.html"))
    }

    fn meta_path(&self, kind: CacheKind, key: &str) -> PathBuf {
        self.root.join(kind.subdir()).join(format!("{key}.meta"))
    }

    /// Fetch or serve from cache. On miss or expired entry, calls `fetch`
    /// and (on success) persists the body + meta. HTTP 5xx and transport
    /// errors bypass the cache entirely. 4xx responses (incl. 404) are
    /// cached under the entry's TTL so repeated misses during a session
    /// don't re-hit the network.
    pub fn get_or_fetch<F>(
        &self,
        url: &str,
        kind: CacheKind,
        fetch: F,
    ) -> Result<HttpResponse, HttpError>
    where
        F: FnOnce() -> Result<HttpResponse, HttpError>,
    {
        let key = hash_key(url);
        if let Some(resp) = self.load_fresh(kind, &key, url) {
            return Ok(resp);
        }

        let resp = fetch()?;
        if resp.status < 500
            && let Err(e) = self.store(kind, &key, url, &resp) {
                log::warn!("tower cache: failed to write {}: {e}", self.root.display());
            }
        Ok(resp)
    }

    fn load_fresh(&self, kind: CacheKind, key: &str, url: &str) -> Option<HttpResponse> {
        let meta_path = self.meta_path(kind, key);
        let body_path = self.body_path(kind, key);
        let meta_bytes = fs::read(&meta_path).ok()?;
        let meta: CacheMeta = serde_json::from_slice(&meta_bytes).ok()?;
        if meta.url != url {
            // Hash collision — treat as miss and overwrite on re-fetch.
            log::warn!("tower cache: hash collision for {url} vs {}", meta.url);
            return None;
        }
        let fetched = SystemTime::UNIX_EPOCH + Duration::from_secs(meta.fetched_at_unix);
        let age = SystemTime::now().duration_since(fetched).ok()?;
        if age > self.ttl_for(kind) {
            return None;
        }
        let body = fs::read(&body_path).ok()?;
        Some(HttpResponse {
            status: meta.status,
            body,
            content_type: meta.content_type.clone(),
        })
    }

    fn store(
        &self,
        kind: CacheKind,
        key: &str,
        url: &str,
        resp: &HttpResponse,
    ) -> io::Result<()> {
        let dir = self.root.join(kind.subdir());
        fs::create_dir_all(&dir)?;
        let body_path = self.body_path(kind, key);
        let meta_path = self.meta_path(kind, key);
        let meta = CacheMeta {
            url: url.to_string(),
            status: resp.status,
            fetched_at_unix: SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            content_type: resp.content_type.clone(),
        };
        write_atomic(&body_path, &resp.body)?;
        let meta_bytes = serde_json::to_vec(&meta).map_err(io::Error::other)?;
        write_atomic(&meta_path, &meta_bytes)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(tmp, path)
}

fn hash_key(url: &str) -> String {
    let mut h = Sha1::new();
    h.update(url.as_bytes());
    let digest = h.finalize();
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn platform_cache_root() -> io::Result<PathBuf> {
    if let Some(v) = std::env::var_os("XDG_CACHE_HOME").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(v));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "$HOME not set"))?;
    #[cfg(target_os = "macos")]
    {
        Ok(PathBuf::from(home).join("Library").join("Caches"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(PathBuf::from(home).join(".cache"))
    }
}

#[path = "tests/cache_tests.rs"]
#[cfg(test)]
mod tests;
