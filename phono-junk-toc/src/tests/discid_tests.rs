//! DiscID algorithm fixture tests.
//!
//! Every fixture commits with a `//` comment citing its upstream source URL.
//! External values (MB DiscIDs, FreeDB IDs, AccurateRip id1/id2/cddb triples)
//! come from authoritative reference implementations and test suites so
//! tautological round-trips are avoided.

use super::*;
use phono_junk_core::Toc;

fn toc(first: u8, last: u8, leadout: u32, offsets: &[u32]) -> Toc {
    Toc {
        first_track: first,
        last_track: last,
        leadout_sector: leadout,
        track_offsets: offsets.to_vec(),
    }
}

// ---------------------------------------------------------------------------
// MusicBrainz DiscID
// ---------------------------------------------------------------------------

// Canonical 6-track example from the MusicBrainz spec:
// https://musicbrainz.org/doc/Disc_ID_Calculation
// (cached in .claude/skills/phono-archive/formats/DiscID.md).
#[test]
fn mb_discid_canonical_6track() {
    let t = toc(1, 6, 95462, &[150, 15363, 32314, 46592, 63414, 80489]);
    assert_eq!(musicbrainz_discid(&t), "49HHV7Eb8UKF3aQiNmu1GR8vKTY-");
}

// 22-track fixture borrowed from libdiscid's test_put.c (commit d9e81a9):
// https://github.com/metabrainz/libdiscid/blob/d9e81a9a8af7679733907418a340c50e9fec8023/test/test_put.c
#[test]
fn mb_discid_libdiscid_22track() {
    let t = toc(
        1,
        22,
        303602,
        &[
            150, 9700, 25887, 39297, 53795, 63735, 77517, 94877, 107270, 123552, 135522, 148422,
            161197, 174790, 192022, 205545, 218010, 228700, 239590, 255470, 266932, 288750,
        ],
    );
    assert_eq!(musicbrainz_discid(&t), "xUp1F2NkfP8s8jaeFn_Av3jNEI4-");
}

// 3-track fixture borrowed from ARver's test_from_track_lengths:
// https://github.com/arcctgx/ARver/blob/master/tests/discinfo_test.py
// Track offsets derived from (tracks=[75258, 54815, 205880], pregap=0, data=0).
#[test]
fn mb_discid_arver_3track() {
    let t = toc(1, 3, 336103, &[150, 75408, 130223]);
    assert_eq!(musicbrainz_discid(&t), "dUmct3Sk4dAt1a98qUKYKC0ZjYU-");
}

// 1-track fixture borrowed from ARver's test_from_track_lengths:
// https://github.com/arcctgx/ARver/blob/master/tests/discinfo_test.py
// (tracks=[279037], pregap=0, data=0). Exercises the single-track edge case.
#[test]
fn mb_discid_arver_1track() {
    let t = toc(1, 1, 279187, &[150]);
    assert_eq!(musicbrainz_discid(&t), "8yz4363CdyKqNa45C30lZWon5jE-");
}

// 4-track with HTOA pregap borrowed from ARver's test_from_track_lengths:
// https://github.com/arcctgx/ARver/blob/master/tests/discinfo_test.py
// (tracks=[107450, 71470, 105737, 71600], pregap=33, data=0).
// Exercises the non-zero first-track offset case.
#[test]
fn mb_discid_arver_4track_pregap() {
    let t = toc(1, 4, 356440, &[183, 107633, 179103, 284840]);
    assert_eq!(musicbrainz_discid(&t), "Grk0WAJTlMgchS.Qilu8OSGvxGg-");
}

// ---------------------------------------------------------------------------
// FreeDB / CDDB ID
// ---------------------------------------------------------------------------

// Borrowed from libdiscid's test_put.c (commit d9e81a9):
// https://github.com/metabrainz/libdiscid/blob/d9e81a9a8af7679733907418a340c50e9fec8023/test/test_put.c
// This is libdiscid's authoritative reference value for discid_get_freedb_id().
#[test]
fn cddb_id_libdiscid_22track() {
    let t = toc(
        1,
        22,
        303602,
        &[
            150, 9700, 25887, 39297, 53795, 63735, 77517, 94877, 107270, 123552, 135522, 148422,
            161197, 174790, 192022, 205545, 218010, 228700, 239590, 255470, 266932, 288750,
        ],
    );
    assert_eq!(cddb_discid(&t), "370fce16");
}

// The following three come from ARver's test_from_track_lengths
// (https://github.com/arcctgx/ARver/blob/master/tests/discinfo_test.py).
// ARver's accuraterip_id string is "NNN-<id1>-<id2>-<cddb>"; we split out
// the cddb component here and the full triple in the AR tests below.

#[test]
fn cddb_id_arver_3track() {
    let t = toc(1, 3, 336103, &[150, 75408, 130223]);
    assert_eq!(cddb_discid(&t), "19117f03");
}

#[test]
fn cddb_id_arver_1track() {
    let t = toc(1, 1, 279187, &[150]);
    assert_eq!(cddb_discid(&t), "020e8801");
}

#[test]
fn cddb_id_arver_4track_pregap() {
    let t = toc(1, 4, 356440, &[183, 107633, 179103, 284840]);
    assert_eq!(cddb_discid(&t), "3e128e04");
}

// ---------------------------------------------------------------------------
// AccurateRip id1 / id2 / cddb triple
// ---------------------------------------------------------------------------

// All three triples below come from ARver's test_from_track_lengths:
// https://github.com/arcctgx/ARver/blob/master/tests/discinfo_test.py
// ARver's accuraterip_id format is "NNN-<id1>-<id2>-<cddb>".

#[test]
fn accuraterip_ids_arver_3track() {
    // accuraterip_id: '003-00084264-001cc184-19117f03'
    let t = toc(1, 3, 336103, &[150, 75408, 130223]);
    assert_eq!(
        accuraterip_ids(&t),
        (
            "00084264".to_string(),
            "001cc184".to_string(),
            "19117f03".to_string()
        )
    );
}

#[test]
fn accuraterip_ids_arver_1track() {
    // accuraterip_id: '001-000441fd-000883fb-020e8801'
    // Single-track edge case: track 1 starts at LSN 0, so id2 uses the
    // lsn_or_1 = 1 fallback.
    let t = toc(1, 1, 279187, &[150]);
    assert_eq!(
        accuraterip_ids(&t),
        (
            "000441fd".to_string(),
            "000883fb".to_string(),
            "020e8801".to_string()
        )
    );
}

#[test]
fn accuraterip_ids_arver_4track_pregap() {
    // accuraterip_id: '004-000e26d9-00380804-3e128e04'
    // Non-zero first-track offset (pregap=33): track 1 LSN=33, no lsn_or_1
    // fallback triggered for track 1 here.
    let t = toc(1, 4, 356440, &[183, 107633, 179103, 284840]);
    assert_eq!(
        accuraterip_ids(&t),
        (
            "000e26d9".to_string(),
            "00380804".to_string(),
            "3e128e04".to_string()
        )
    );
}

// ---------------------------------------------------------------------------
// compute_disc_ids wrapper (cross-module test)
// ---------------------------------------------------------------------------

#[test]
fn compute_disc_ids_populates_all_four_id_fields() {
    use crate::compute_disc_ids;
    let t = toc(1, 3, 336103, &[150, 75408, 130223]);
    let ids = compute_disc_ids(&t);
    assert_eq!(
        ids.mb_discid.as_deref(),
        Some("dUmct3Sk4dAt1a98qUKYKC0ZjYU-")
    );
    assert_eq!(ids.cddb_id.as_deref(), Some("19117f03"));
    assert_eq!(ids.ar_discid1.as_deref(), Some("00084264"));
    assert_eq!(ids.ar_discid2.as_deref(), Some("001cc184"));
    // Provider-supplied fields stay None.
    assert_eq!(ids.barcode, None);
    assert_eq!(ids.catalog_number, None);
}
