//! End-to-end live network test for dBAR fetch + parse.
//!
//! `#[ignore]`-gated so default `cargo test` runs are offline. Invoke
//! with `cargo test -p phono-junk-accuraterip -- --ignored` to fetch a
//! real dBAR from `www.accuraterip.com`.
//!
//! The disc ID triple below is the same 3-track ARver fixture used
//! elsewhere in this workspace for URL construction. Whether
//! AccurateRip has submissions for it is up to the real database — the
//! test asserts either "got some responses back" or "clean 404", not a
//! specific album.

use phono_junk_accuraterip::AccurateRipClient;
use phono_junk_core::DiscIds;

fn fixture_ids() -> DiscIds {
    DiscIds {
        ar_discid1: Some("00084264".into()),
        ar_discid2: Some("001cc184".into()),
        cddb_id: Some("19117f03".into()),
        ..DiscIds::default()
    }
}

#[test]
#[ignore = "network: fetches from www.accuraterip.com"]
fn fetch_and_parse_real_dbar() {
    let client =
        AccurateRipClient::new("phono-junk-test/0.1 ( test@example.com )").expect("http client");
    let result = client.fetch_dbar(&fixture_ids(), 3).expect("network/parse");
    match result {
        Some(dbar) => {
            assert!(
                !dbar.responses.is_empty(),
                "200 OK should yield at least one response"
            );
            for r in &dbar.responses {
                assert_eq!(
                    r.tracks.len(),
                    r.track_count as usize,
                    "parsed tracks count must match header count"
                );
            }
        }
        None => {
            // 404 is a valid outcome — not every synthetic TOC has been
            // submitted. The test still proves URL construction + HTTP
            // plumbing work end-to-end.
        }
    }
}
