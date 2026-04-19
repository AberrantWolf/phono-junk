//! dBAR URL construction tests.
//!
//! Cross-checked against ARver's
//! [`arver/disc/database.py`](https://github.com/arcctgx/ARver/blob/master/arver/disc/database.py)
//! format string:
//!
//! ```python
//! 'http://www.accuraterip.com/accuraterip/{0}/{1}/{2}/dBAR-{3:03d}-{4}-{5}-{6}.bin'
//! .format(discid1[-1], discid1[-2], discid1[-3],
//!         num_tracks, discid1, discid2, freedb_id)
//! ```

use phono_junk_accuraterip::{AccurateRipError, dbar_url};
use phono_junk_core::DiscIds;

fn ids_with(id1: &str, id2: &str, cddb: &str) -> DiscIds {
    DiscIds {
        ar_discid1: Some(id1.into()),
        ar_discid2: Some(id2.into()),
        cddb_id: Some(cddb.into()),
        ..DiscIds::default()
    }
}

#[test]
fn arver_3_track_fixture_matches_reference_format() {
    // IDs from the phono-junk-toc 3-track fixture — known-good values
    // checked against libdiscid / ARver upstream.
    let ids = ids_with("00084264", "001cc184", "19117f03");
    let url = dbar_url(&ids, 3).expect("url");
    assert_eq!(
        url,
        "http://www.accuraterip.com/accuraterip/4/6/2/dBAR-003-00084264-001cc184-19117f03.bin"
    );
}

#[test]
fn track_count_is_zero_padded_to_three_digits() {
    let ids = ids_with("00084264", "001cc184", "19117f03");
    assert!(dbar_url(&ids, 1).unwrap().contains("dBAR-001-"));
    assert!(dbar_url(&ids, 10).unwrap().contains("dBAR-010-"));
    assert!(dbar_url(&ids, 99).unwrap().contains("dBAR-099-"));
    assert!(dbar_url(&ids, 100).unwrap().contains("dBAR-100-"));
}

#[test]
fn missing_ar_id1_is_structured_error() {
    let mut ids = ids_with("00084264", "001cc184", "19117f03");
    ids.ar_discid1 = None;
    match dbar_url(&ids, 3) {
        Err(AccurateRipError::MissingId("ar_discid1")) => {}
        other => panic!("expected MissingId(ar_discid1), got {other:?}"),
    }
}

#[test]
fn missing_ar_id2_is_structured_error() {
    let mut ids = ids_with("00084264", "001cc184", "19117f03");
    ids.ar_discid2 = None;
    match dbar_url(&ids, 3) {
        Err(AccurateRipError::MissingId("ar_discid2")) => {}
        other => panic!("expected MissingId(ar_discid2), got {other:?}"),
    }
}

#[test]
fn missing_cddb_is_structured_error() {
    let mut ids = ids_with("00084264", "001cc184", "19117f03");
    ids.cddb_id = None;
    match dbar_url(&ids, 3) {
        Err(AccurateRipError::MissingId("cddb_id")) => {}
        other => panic!("expected MissingId(cddb_id), got {other:?}"),
    }
}

#[test]
fn short_id1_rejected_rather_than_panic() {
    let ids = ids_with("ab", "001cc184", "19117f03");
    assert!(matches!(dbar_url(&ids, 3), Err(AccurateRipError::Parse(_))));
}
