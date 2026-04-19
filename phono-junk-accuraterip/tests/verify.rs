//! Pure verification-logic tests: compare computed [`TrackCrc`] values
//! against synthetic [`DbarFile`]s.

use phono_junk_accuraterip::{
    DbarFile, DbarResponse, ExpectedCrc, TrackCrc, verify_disc, verify_track,
};

fn dbar_with_responses(responses: Vec<DbarResponse>) -> DbarFile {
    DbarFile { responses }
}

fn entry(confidence: u8, v1: u32, v2: u32) -> ExpectedCrc {
    ExpectedCrc { confidence, v1, v2 }
}

fn response(confidence_v1_v2: &[(u8, u32, u32)]) -> DbarResponse {
    DbarResponse {
        track_count: confidence_v1_v2.len() as u8,
        ar_id1: 0,
        ar_id2: 0,
        cddb_id: 0,
        tracks: confidence_v1_v2
            .iter()
            .map(|&(c, v1, v2)| entry(c, v1, v2))
            .collect(),
    }
}

#[test]
fn v2_match_in_any_pressing_is_verified() {
    let dbar = dbar_with_responses(vec![
        response(&[(5, 0x1111, 0x2222), (5, 0x3333, 0x4444)]),
        response(&[(9, 0xaaaa, 0xbbbb), (9, 0xcccc, 0xdddd)]),
    ]);
    let got = verify_track(
        &dbar,
        2,
        TrackCrc {
            v1: 0xcccc,
            v2: 0xdddd,
        },
    );
    assert!(got.is_verified());
    assert_eq!(got.v1_matches.len(), 1);
    assert_eq!(got.v1_matches[0].pressing, 1);
    assert_eq!(got.v1_matches[0].confidence, 9);
    assert_eq!(got.v2_matches.len(), 1);
    assert_eq!(got.best_confidence(), Some(9));
    assert_eq!(got.status_string(), "v2 confidence 9");
}

#[test]
fn v1_only_match_reports_v2_no_match() {
    let dbar = dbar_with_responses(vec![response(&[(3, 0xdeadbeef, 0xcafef00d)])]);
    let got = verify_track(
        &dbar,
        1,
        TrackCrc {
            v1: 0xdeadbeef,
            v2: 0x0000_ffff,
        },
    );
    assert!(got.is_verified());
    assert_eq!(got.v1_matches.len(), 1);
    assert!(got.v2_matches.is_empty());
    assert_eq!(got.status_string(), "v1 confidence 3 (v2 no match)");
}

#[test]
fn no_match_reports_cleanly() {
    let dbar = dbar_with_responses(vec![response(&[(7, 0x1111_1111, 0x2222_2222)])]);
    let got = verify_track(
        &dbar,
        1,
        TrackCrc {
            v1: 0x0000_0000,
            v2: 0x0000_0000,
        },
    );
    assert!(!got.is_verified());
    assert!(got.best_confidence().is_none());
    assert_eq!(got.status_string(), "no match");
}

#[test]
fn legacy_v2_zero_sentinel_does_not_spuriously_match_zero_crc() {
    // Entry with v2 == 0 is legacy (pre-v2 submission). A computed v2 of 0
    // must NOT match — otherwise every computed zero would verify against
    // legacy data.
    let dbar = dbar_with_responses(vec![response(&[(4, 0xdeadbeef, 0)])]);
    let got = verify_track(
        &dbar,
        1,
        TrackCrc {
            v1: 0x0000_0000,
            v2: 0x0000_0000,
        },
    );
    assert!(!got.is_verified());
    assert!(got.v2_matches.is_empty());
}

#[test]
fn v1_match_on_legacy_entry_still_counts() {
    // Same legacy entry, but v1 is the real thing — should match.
    let dbar = dbar_with_responses(vec![response(&[(4, 0xdeadbeef, 0)])]);
    let got = verify_track(
        &dbar,
        1,
        TrackCrc {
            v1: 0xdeadbeef,
            v2: 0,
        },
    );
    assert_eq!(got.v1_matches.len(), 1);
    assert!(got.v2_matches.is_empty());
    assert_eq!(got.status_string(), "v1 confidence 4 (v2 no match)");
}

#[test]
fn best_confidence_picks_the_highest_across_versions() {
    let dbar = dbar_with_responses(vec![
        response(&[(2, 0xaaaa, 0xbbbb)]),
        response(&[(8, 0xaaaa, 0xcccc)]),
        response(&[(5, 0xaaaa, 0xbbbb)]),
    ]);
    let got = verify_track(
        &dbar,
        1,
        TrackCrc {
            v1: 0xaaaa,
            v2: 0xbbbb,
        },
    );
    // v1 matches all three pressings; v2 matches pressings 0 and 2.
    assert_eq!(got.v1_matches.len(), 3);
    assert_eq!(got.v2_matches.len(), 2);
    assert_eq!(got.best_confidence(), Some(8));
}

#[test]
fn verify_disc_handles_multi_track_in_one_pass() {
    let dbar = dbar_with_responses(vec![response(&[
        (3, 0x1111, 0x2222),
        (3, 0x3333, 0x4444),
        (3, 0x5555, 0x6666),
    ])]);
    let got = verify_disc(
        &dbar,
        &[
            (
                1,
                TrackCrc {
                    v1: 0x1111,
                    v2: 0x2222,
                },
            ),
            (
                2,
                TrackCrc {
                    v1: 0x9999,
                    v2: 0x4444,
                },
            ),
            (
                3,
                TrackCrc {
                    v1: 0x5555,
                    v2: 0x0000,
                },
            ),
        ],
    );
    assert_eq!(got.len(), 3);
    assert!(got[0].is_verified() && !got[0].v2_matches.is_empty());
    assert!(got[1].is_verified() && got[1].v1_matches.is_empty());
    assert!(got[2].is_verified() && got[2].v2_matches.is_empty());
}
