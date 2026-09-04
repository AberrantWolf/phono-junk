//! Rate-limited, blocking HTTP client shared by every provider crate.
//!
//! Per-host quotas use `governor` token buckets; hosts with no registered
//! quota pass through without limiting. Construction enforces a
//! [`HttpClientBuilder::user_agent`] — MusicBrainz returns 403 without one,
//! and a silent default would surface as a confusing network error.
//!
//! URL redaction is deliberately centralized here: callers may put API keys
//! in query parameters, while transport errors can otherwise echo the full
//! request URL. Retries and backoff policy remain caller concerns.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use governor::clock::{Clock, DefaultClock};
use governor::middleware::NoOpMiddleware;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};

type DirectLimiter<C> =
    RateLimiter<NotKeyed, InMemoryState, C, NoOpMiddleware<<C as Clock>::Instant>>;
use thiserror::Error;
use url::Url;

/// Default request-total timeout (connect + read).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Transport-layer errors. Providers map these into their own domain errors.
#[derive(Debug, Error)]
pub enum HttpError {
    #[error("missing User-Agent — the client must be built with .user_agent(...)")]
    MissingUserAgent,
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("server rate-limited the request (HTTP 429)")]
    ServerRateLimited,
    #[error("request timed out")]
    Timeout,
}

/// Result of a successful HTTP fetch. Non-2xx statuses are returned as-is
/// (apart from 429, which is promoted to [`HttpError::ServerRateLimited`]);
/// providers inspect the status themselves.
#[derive(Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
}

/// Type-erased per-host rate-limit bucket.
///
/// Prod path wraps a governor limiter with [`DefaultClock`]; tests can plug in
/// a [`governor::clock::FakeRelativeClock`] variant via
/// [`HttpClientBuilder::fake_host_quota`].
trait HostBucket: Send + Sync {
    /// Returns `Ok(())` when a request may proceed, or `Err(wait)` giving the
    /// duration the caller should wait (or fake-advance) before retrying.
    fn check(&self) -> Result<(), Duration>;
}

struct RealBucket {
    limiter: DirectLimiter<DefaultClock>,
    clock: DefaultClock,
}

impl HostBucket for RealBucket {
    fn check(&self) -> Result<(), Duration> {
        match self.limiter.check() {
            Ok(()) => Ok(()),
            Err(not_until) => Err(not_until.wait_time_from(self.clock.now())),
        }
    }
}

#[cfg(test)]
struct FakeBucket {
    limiter: DirectLimiter<governor::clock::FakeRelativeClock>,
    clock: governor::clock::FakeRelativeClock,
}

#[cfg(test)]
impl HostBucket for FakeBucket {
    fn check(&self) -> Result<(), Duration> {
        match self.limiter.check() {
            Ok(()) => Ok(()),
            Err(not_until) => Err(not_until.wait_time_from(self.clock.now())),
        }
    }
}

/// Rate-limited blocking HTTP client. Construct via [`HttpClient::builder`].
///
/// `Clone` is shallow — every field is already cheap to share (`reqwest`'s
/// blocking client is internally `Arc`, the per-host buckets are
/// `Arc<dyn HostBucket>`, and `sleep_fn` is an `Arc`). Cloning a client
/// therefore produces an alias that shares rate-limit state: two providers
/// handed clones of the same client compete for the same token buckets.
/// This is exactly what [`crate::Aggregator`]'s parallel fan-out needs to
/// avoid double-spending MusicBrainz's 1 req/sec quota across MB + CAA.
#[derive(Clone)]
pub struct HttpClient {
    http: reqwest::blocking::Client,
    user_agent: String,
    per_host: HashMap<String, Arc<dyn HostBucket>>,
    sleep_fn: Arc<dyn Fn(Duration) + Send + Sync>,
}

impl HttpClient {
    pub fn builder() -> HttpClientBuilder {
        HttpClientBuilder::new()
    }

    /// Issue a rate-limited GET. Waits on the per-host bucket (if registered)
    /// before sending. Injects the configured `User-Agent` header.
    pub fn get(&self, url: &Url) -> Result<HttpResponse, HttpError> {
        self.get_with_headers(url, &[])
    }

    /// GET with additional request headers (e.g. `Authorization`). The
    /// configured `User-Agent` is always injected; caller-supplied headers
    /// are additive. Header values are passed to `reqwest` verbatim and
    /// are **never** logged — callers owning secrets do not need to scrub
    /// them before passing in.
    pub fn get_with_headers(
        &self,
        url: &Url,
        headers: &[(reqwest::header::HeaderName, reqwest::header::HeaderValue)],
    ) -> Result<HttpResponse, HttpError> {
        let host = url
            .host_str()
            .ok_or_else(|| HttpError::InvalidUrl("URL has no host".into()))?
            .to_ascii_lowercase();
        let safe_url = redacted_url(url);

        if let Some(bucket) = self.per_host.get(&host) {
            loop {
                match bucket.check() {
                    Ok(()) => break,
                    Err(wait) => {
                        log::trace!("rate-limit wait for {host}: {wait:?}");
                        (self.sleep_fn)(wait);
                    }
                }
            }
        }

        log::debug!("HTTP GET {safe_url}");
        let mut builder = self
            .http
            .get(url.clone())
            .header(reqwest::header::USER_AGENT, &self.user_agent);
        for (name, value) in headers {
            builder = builder.header(name.clone(), value.clone());
        }
        let resp = builder.send().map_err(|e| {
            let err = map_reqwest_err(e);
            log::warn!("HTTP GET {safe_url} failed: {err}");
            err
        })?;

        let status = resp.status();
        log::debug!("HTTP GET {safe_url} → {}", status.as_u16());
        if status.as_u16() == 429 {
            log::warn!("HTTP 429 rate-limited by {host} ({safe_url})");
            return Err(HttpError::ServerRateLimited);
        }
        if !status.is_success() && status.as_u16() != 404 {
            log::warn!("HTTP {} from {safe_url}", status.as_u16());
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let body = resp.bytes().map_err(map_reqwest_err)?.to_vec();

        Ok(HttpResponse {
            status: status.as_u16(),
            body,
            content_type,
        })
    }
}

fn map_reqwest_err(e: reqwest::Error) -> HttpError {
    if e.is_timeout() {
        HttpError::Timeout
    } else {
        HttpError::Transport(e.without_url().to_string())
    }
}

const REDACTED_QUERY_VALUE: &str = "[REDACTED]";
const SENSITIVE_QUERY_KEYS: &[&str] = &["key", "token", "access_token", "api_key"];

/// Return a log-safe copy of a request URL.
///
/// Provider credentials occasionally have to travel in a query parameter.
/// Keeping the scrubber in the shared transport makes the safe behavior the
/// default for every current and future provider.
pub fn redacted_url(url: &Url) -> Url {
    let Some(_) = url.query() else {
        return url.clone();
    };

    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(key, value)| {
            let value = if SENSITIVE_QUERY_KEYS
                .iter()
                .any(|candidate| key.eq_ignore_ascii_case(candidate))
            {
                REDACTED_QUERY_VALUE.into()
            } else {
                value.into_owned()
            };
            (key.into_owned(), value)
        })
        .collect();

    let mut safe = url.clone();
    safe.query_pairs_mut().clear().extend_pairs(pairs);
    safe
}

/// Builder for [`HttpClient`]. The `user_agent` is mandatory; `build` returns
/// [`HttpError::MissingUserAgent`] otherwise.
pub struct HttpClientBuilder {
    user_agent: Option<String>,
    per_host: HashMap<String, Arc<dyn HostBucket>>,
    timeout: Duration,
    sleep_fn: Arc<dyn Fn(Duration) + Send + Sync>,
}

impl HttpClientBuilder {
    pub fn new() -> Self {
        Self {
            user_agent: None,
            per_host: HashMap::new(),
            timeout: DEFAULT_TIMEOUT,
            sleep_fn: Arc::new(std::thread::sleep),
        }
    }

    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }

    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// Register a per-host quota using the real monotonic clock. The `host`
    /// string is matched case-insensitively against the URL's host component.
    pub fn host_quota(mut self, host: impl Into<String>, quota: Quota) -> Self {
        let clock = DefaultClock::default();
        let limiter = RateLimiter::direct_with_clock(quota, clock.clone());
        self.per_host.insert(
            host.into().to_ascii_lowercase(),
            Arc::new(RealBucket { limiter, clock }),
        );
        self
    }

    /// Register the quota declared by a provider descriptor. Provider crates
    /// own policy; the application owns one shared scheduler/client.
    pub fn host_rate_policy(self, policy: crate::HostRatePolicy) -> Self {
        let requests = NonZeroU32::new(policy.requests)
            .expect("provider rate policy must permit at least one request");
        let quota = match policy.period_seconds {
            1 => Quota::per_second(requests),
            60 => Quota::per_minute(requests),
            seconds => {
                let interval = Duration::from_secs_f64(seconds as f64 / f64::from(policy.requests));
                Quota::with_period(interval)
                    .expect("provider rate policy period must be non-zero")
                    .allow_burst(requests)
            }
        };
        self.host_quota(policy.host, quota)
    }

    /// Test-only: register a host with a fake clock, and (critically) swap the
    /// sleep function to advance that same clock instead of blocking the
    /// thread. All hosts registered via this method should share one clock so
    /// their buckets tick together.
    #[cfg(test)]
    pub fn fake_host_quota(
        mut self,
        host: impl Into<String>,
        quota: Quota,
        clock: governor::clock::FakeRelativeClock,
    ) -> Self {
        let limiter = RateLimiter::direct_with_clock(quota, clock.clone());
        self.per_host.insert(
            host.into().to_ascii_lowercase(),
            Arc::new(FakeBucket {
                limiter,
                clock: clock.clone(),
            }),
        );
        let clock_for_sleep = clock;
        self.sleep_fn = Arc::new(move |d| clock_for_sleep.advance(d));
        self
    }

    pub fn build(self) -> Result<HttpClient, HttpError> {
        let ua = self.user_agent.ok_or(HttpError::MissingUserAgent)?;
        let http = reqwest::blocking::Client::builder()
            .timeout(self.timeout)
            .build()
            .map_err(|e| HttpError::Transport(e.without_url().to_string()))?;
        Ok(HttpClient {
            http,
            user_agent: ua,
            per_host: self.per_host,
            sleep_fn: self.sleep_fn,
        })
    }
}

impl Default for HttpClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "tests/http_tests.rs"]
mod tests;
