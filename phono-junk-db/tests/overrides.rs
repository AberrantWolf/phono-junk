use phono_junk_catalog::{Album, Disc, Override, Release, RipFile, Track};
use phono_junk_core::IdentificationConfidence;
use phono_junk_db::overrides::{OverrideError, OverrideTarget, apply, apply_override, parse_sub_path};

fn album() -> Album {
    Album {
        id: 1,
        title: "Original".into(),
        sort_title: None,
        artist_credit: None,
        year: Some(1999),
        mbid: None,
        primary_type: None,
        secondary_types: Vec::new(),
        first_release_date: None,
    }
}

fn disc_with_tracks(n: u8) -> (Disc, Vec<Track>) {
    let disc = Disc {
        id: 10,
        release_id: 1,
        disc_number: 1,
        format: "CD".into(),
        toc: None,
        mb_discid: None,
        cddb_id: None,
        ar_discid1: None,
        ar_discid2: None,
        dbar_raw: None,
    };
    let tracks = (1..=n)
        .map(|pos| Track {
            id: 100 + pos as i64,
            disc_id: disc.id,
            position: pos,
            title: Some(format!("T{pos}")),
            artist_credit: None,
            length_frames: None,
            isrc: None,
            mbid: None,
            recording_mbid: None,
        })
        .collect();
    (disc, tracks)
}

#[test]
fn flat_field_on_album() {
    let mut a = album();
    let path = parse_sub_path(None).unwrap();
    apply_override(OverrideTarget::Album(&mut a), &path, "title", "Corrected").unwrap();
    assert_eq!(a.title, "Corrected");
}

#[test]
fn clear_optional_field_with_empty_value() {
    let mut a = album();
    a.artist_credit = Some("X".into());
    apply_override(OverrideTarget::Album(&mut a), &[], "artist_credit", "").unwrap();
    assert_eq!(a.artist_credit, None);
}

#[test]
fn parse_u16_year() {
    let mut a = album();
    apply_override(OverrideTarget::Album(&mut a), &[], "year", "2001").unwrap();
    assert_eq!(a.year, Some(2001));
}

#[test]
fn bad_year_value_errors() {
    let mut a = album();
    let err =
        apply_override(OverrideTarget::Album(&mut a), &[], "year", "twothousand").unwrap_err();
    assert!(matches!(err, OverrideError::ValueParse { .. }));
}

#[test]
fn unknown_field_on_release_errors() {
    let mut r = Release {
        id: 1,
        album_id: 1,
        country: None,
        date: None,
        label: None,
        catalog_number: None,
        barcode: None,
        mbid: None,
        status: None,
        language: None,
        script: None,
    };
    let err = apply_override(OverrideTarget::Release(&mut r), &[], "weird", "v").unwrap_err();
    assert!(matches!(err, OverrideError::UnknownField { .. }));
}

#[test]
fn track_n_title_on_disc() {
    let (mut disc, mut tracks) = disc_with_tracks(5);
    let path = parse_sub_path(Some("track[3]")).unwrap();
    apply_override(
        OverrideTarget::Disc {
            disc: &mut disc,
            tracks: &mut tracks,
        },
        &path,
        "title",
        "Fixed",
    )
    .unwrap();
    assert_eq!(tracks[2].title.as_deref(), Some("Fixed"));
    assert_eq!(tracks[0].title.as_deref(), Some("T1"));
}

#[test]
fn track_index_out_of_range() {
    let (mut disc, mut tracks) = disc_with_tracks(2);
    let path = parse_sub_path(Some("track[5]")).unwrap();
    let err = apply_override(
        OverrideTarget::Disc {
            disc: &mut disc,
            tracks: &mut tracks,
        },
        &path,
        "title",
        "X",
    )
    .unwrap_err();
    assert!(matches!(err, OverrideError::IndexOutOfRange { index: 5, len: 2, .. }));
}

#[test]
fn track_index_zero_rejected() {
    let (mut disc, mut tracks) = disc_with_tracks(2);
    let path = parse_sub_path(Some("track[0]")).unwrap();
    let err = apply_override(
        OverrideTarget::Disc {
            disc: &mut disc,
            tracks: &mut tracks,
        },
        &path,
        "title",
        "X",
    )
    .unwrap_err();
    assert!(matches!(err, OverrideError::IndexOutOfRange { .. }));
}

#[test]
fn disc_flat_field_format() {
    let (mut disc, mut tracks) = disc_with_tracks(1);
    apply_override(
        OverrideTarget::Disc {
            disc: &mut disc,
            tracks: &mut tracks,
        },
        &[],
        "format",
        "HDCD",
    )
    .unwrap();
    assert_eq!(disc.format, "HDCD");
}

#[test]
fn sub_path_on_album_rejected() {
    let mut a = album();
    let path = parse_sub_path(Some("track[1]")).unwrap();
    let err = apply_override(OverrideTarget::Album(&mut a), &path, "title", "x").unwrap_err();
    assert!(matches!(err, OverrideError::TargetMismatch { .. }));
}

#[test]
fn apply_from_override_row() {
    let (mut disc, mut tracks) = disc_with_tracks(3);
    let ovr = Override {
        id: 1,
        entity_type: "Disc".into(),
        entity_id: 10,
        sub_path: Some("track[2]".into()),
        field: "title".into(),
        override_value: "National Anthem".into(),
        reason: None,
        created_at: None,
    };
    apply(
        OverrideTarget::Disc {
            disc: &mut disc,
            tracks: &mut tracks,
        },
        &ovr,
    )
    .unwrap();
    assert_eq!(tracks[1].title.as_deref(), Some("National Anthem"));
}

#[test]
fn rip_file_accuraterip_status() {
    let mut f = RipFile {
        id: 1,
        disc_id: None,
        cue_path: None,
        chd_path: None,
        bin_paths: Vec::new(),
        mtime: None,
        size: None,
        identification_confidence: IdentificationConfidence::Unidentified,
        identification_source: None,
        accuraterip_status: None,
        last_verified_at: None,
    };
    apply_override(
        OverrideTarget::RipFile(&mut f),
        &[],
        "accuraterip_status",
        "manual pass",
    )
    .unwrap();
    assert_eq!(f.accuraterip_status.as_deref(), Some("manual pass"));
}
