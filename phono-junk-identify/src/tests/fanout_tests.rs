//! Internal unit test for parallel fan-out + shared rate-limit
//! coordination. Lives inside the crate because
//! [`crate::HttpClientBuilder::fake_host_quota`] is `#[cfg(test)]`-gated
//! and therefore only reachable from within.

use std::time::Duration;

use governor::Quota;
use governor::clock::{Clock, FakeRelativeClock};
use httpmock::prelude::*;
use nonzero_ext::nonzero;
use phono_junk_core::{DiscIds, Toc};

use crate::{
    Credentials, DiscIdKind, HttpClient, IdentificationProvider, ProviderError, ProviderResult,
    identify_parallel,
};

struct HttpProbe {
    name: &'static str,
    http: HttpClient,
    url: String,
}

impl IdentificationProvider for HttpProbe {
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
        let _ = self
            .http
            .get(&self.url)
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        Ok(None)
    }
}

#[test]
fn shared_host_bucket_serializes_requests_across_cloned_clients() {
    // Three providers share one HttpClient. All three hit the same host.
    // The per-host bucket should serialize their requests — three calls
    // at 1 req/sec should advance the fake clock by ~2s total (one free
    // token, then two 1-s waits).
    let server = MockServer::start();
    let _mock = server.mock(|when, then| {
        when.method(GET).path("/probe");
        then.status(200).body("ok");
    });

    let clock = FakeRelativeClock::default();
    let http = HttpClient::builder()
        .user_agent("test/1.0")
        .fake_host_quota(
            "127.0.0.1",
            Quota::per_second(nonzero!(1u32)),
            clock.clone(),
        )
        .build()
        .unwrap();

    let url = server.url("/probe");
    let providers: Vec<Box<dyn IdentificationProvider>> = vec![
        Box::new(HttpProbe {
            name: "p1",
            http: http.clone(),
            url: url.clone(),
        }),
        Box::new(HttpProbe {
            name: "p2",
            http: http.clone(),
            url: url.clone(),
        }),
        Box::new(HttpProbe {
            name: "p3",
            http,
            url,
        }),
    ];

    let toc = Toc {
        first_track: 1,
        last_track: 1,
        leadout_sector: 100,
        track_offsets: vec![0],
    };
    let ids = DiscIds {
        mb_discid: Some("d".into()),
        ..Default::default()
    };

    let t0 = Duration::from(clock.now());
    let _ = identify_parallel(&providers, &toc, &ids, &Credentials::new());
    let elapsed = Duration::from(clock.now()) - t0;
    assert!(
        elapsed >= Duration::from_millis(1990),
        "shared per-host bucket did not throttle across cloned clients: elapsed={elapsed:?}"
    );
}
