//! Audio-CD TOC-assembly tests.
//!
//! Fixture offsets come from ARver's test suite; CD-Extra handling is
//! verified against the -11,400 frame correction documented in the
//! MusicBrainz DiscID spec.
//!
//! Sources:
//! - <https://github.com/arcctgx/ARver/blob/master/tests/discinfo_test.py>
//! - <https://musicbrainz.org/doc/Disc_ID_Calculation>

use super::*;
use junk_libs_disc::{TrackKind, TrackLayout};

fn audio(number: u8, absolute_offset: u32, length: u32) -> TrackLayout {
    TrackLayout {
        number,
        absolute_offset,
        length_sectors: length,
        kind: TrackKind::Audio,
        mode: "AUDIO".into(),
    }
}

fn data(number: u8, absolute_offset: u32, length: u32) -> TrackLayout {
    TrackLayout {
        number,
        absolute_offset,
        length_sectors: length,
        kind: TrackKind::Data,
        mode: "MODE2/2352".into(),
    }
}

// Audio-only: produces the ARver 3-track Toc exactly.
#[test]
fn layout_to_toc_audio_only_arver_3track() {
    let layout = vec![
        audio(1, 150, 75258),
        audio(2, 75408, 54815),
        audio(3, 130223, 205880),
    ];
    let toc = layout_to_toc(&layout).unwrap();
    assert_eq!(toc.first_track, 1);
    assert_eq!(toc.last_track, 3);
    assert_eq!(toc.track_offsets, vec![150, 75408, 130223]);
    assert_eq!(toc.leadout_sector, 336103);
}

// CD-Extra: 3 audio + 1 data at data_offset = audio_leadout + 11400.
// Expected Toc is identical to the pure-audio version — that's the point
// of the correction.
#[test]
fn layout_to_toc_cd_extra_reproduces_audio_leadout() {
    let audio_leadout = 336103;
    let layout = vec![
        audio(1, 150, 75258),
        audio(2, 75408, 54815),
        audio(3, 130223, 205880),
        data(4, audio_leadout + 11_400, 600),
    ];
    let toc = layout_to_toc(&layout).unwrap();
    assert_eq!(toc.last_track, 3);
    assert_eq!(toc.track_offsets, vec![150, 75408, 130223]);
    assert_eq!(toc.leadout_sector, audio_leadout);
}

#[test]
fn layout_to_toc_leading_data_is_unsupported() {
    let layout = vec![data(1, 150, 1000), audio(2, 1150, 50000)];
    let err = layout_to_toc(&layout).unwrap_err();
    assert!(matches!(err, AudioError::Unsupported(_)));
}

#[test]
fn layout_to_toc_all_data_is_invalid() {
    let layout = vec![data(1, 150, 1000), data(2, 1150, 2000)];
    let err = layout_to_toc(&layout).unwrap_err();
    assert!(matches!(err, AudioError::InvalidToc(_)));
}

#[test]
fn layout_to_toc_empty_is_invalid() {
    let err = layout_to_toc(&[]).unwrap_err();
    assert!(matches!(err, AudioError::InvalidToc(_)));
}

#[test]
fn layout_to_toc_cd_extra_underflow_is_invalid() {
    // Data track at absolute 10,000 < 11,400 → underflow.
    let layout = vec![audio(1, 150, 500), data(2, 10_000, 100)];
    let err = layout_to_toc(&layout).unwrap_err();
    assert!(matches!(err, AudioError::InvalidToc(_)));
}

#[test]
fn layout_to_toc_non_monotonic_offsets_is_invalid() {
    // Track 2 absolute offset earlier than track 1 → caught by validator.
    let layout = vec![audio(1, 500, 100), audio(2, 300, 100)];
    let err = layout_to_toc(&layout).unwrap_err();
    assert!(matches!(err, AudioError::InvalidToc(_)));
}

#[test]
fn layout_to_toc_unknown_kind_treated_as_audio() {
    let layout = vec![TrackLayout {
        number: 1,
        absolute_offset: 150,
        length_sectors: 100,
        kind: TrackKind::Unknown,
        mode: "WEIRD".into(),
    }];
    let toc = layout_to_toc(&layout).unwrap();
    assert_eq!(toc.leadout_sector, 250);
}
