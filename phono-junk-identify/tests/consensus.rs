//! Consensus merge behaviour: plurality + priority, MBID cohort, per-track
//! positional merge.

use phono_junk_core::Toc;
use phono_junk_identify::{
    AlbumMeta, DisagreementEntity, ProviderResult, ReleaseMeta, TrackMeta, merge,
    merge_with_toc_fallback,
};

fn provider_result(
    name: &str,
    album: Option<AlbumMeta>,
    release: Option<ReleaseMeta>,
    tracks: Vec<TrackMeta>,
) -> ProviderResult {
    ProviderResult {
        album,
        release,
        tracks,
        cover_art_urls: Vec::new(),
        provider: name.into(),
        raw_response: None,
    }
}

fn album_with_title(title: &str) -> AlbumMeta {
    AlbumMeta {
        title: Some(title.into()),
        artist_credit: Some("Some Artist".into()),
        year: Some(2020),
        mbid: None,
    }
}

#[test]
fn agreement_no_disagreements() {
    let a = provider_result("mb", Some(album_with_title("X")), None, vec![]);
    let b = provider_result("itunes", Some(album_with_title("X")), None, vec![]);
    let m = merge(&[a, b]);
    assert_eq!(m.album.title.as_deref(), Some("X"));
    assert!(
        m.disagreements.is_empty(),
        "expected no disagreements, got {:#?}",
        m.disagreements
    );
}

#[test]
fn two_providers_title_conflict_first_wins() {
    let a = provider_result("mb", Some(album_with_title("Correct")), None, vec![]);
    let b = provider_result("itunes", Some(album_with_title("Wrong")), None, vec![]);
    let m = merge(&[a, b]);
    assert_eq!(m.album.title.as_deref(), Some("Correct"));
    assert_eq!(m.disagreements.len(), 1);
    let d = &m.disagreements[0];
    assert_eq!(d.entity, DisagreementEntity::Album);
    assert_eq!(d.field, "album.title");
    assert_eq!(d.source_a, "mb");
    assert_eq!(d.value_a.as_str(), Some("Correct"));
    assert_eq!(d.source_b, "itunes");
    assert_eq!(d.value_b.as_str(), Some("Wrong"));
}

#[test]
fn mbid_split_excludes_losing_cohort() {
    // MB claims one album+mbid, a second provider claims another album+mbid.
    // The MBID winner (MB, by priority) keeps its fields; the loser's fields
    // are excluded from merging but MBID disagreement is recorded.
    let a = provider_result(
        "mb",
        Some(AlbumMeta {
            title: Some("Real Album".into()),
            artist_credit: Some("Real Artist".into()),
            year: Some(2020),
            mbid: Some("mbid-winner".into()),
        }),
        None,
        vec![TrackMeta {
            position: 1,
            title: Some("Real Track".into()),
            ..TrackMeta::default()
        }],
    );
    let b = provider_result(
        "other",
        Some(AlbumMeta {
            title: Some("Wrong Album".into()),
            artist_credit: Some("Wrong Artist".into()),
            year: Some(1999),
            mbid: Some("mbid-loser".into()),
        }),
        None,
        vec![TrackMeta {
            position: 1,
            title: Some("Wrong Track".into()),
            ..TrackMeta::default()
        }],
    );
    let m = merge(&[a, b]);
    assert_eq!(m.album.mbid.as_deref(), Some("mbid-winner"));
    assert_eq!(m.album.title.as_deref(), Some("Real Album"));
    assert_eq!(m.album.artist_credit.as_deref(), Some("Real Artist"));
    assert_eq!(m.tracks.len(), 1);
    assert_eq!(m.tracks[0].title.as_deref(), Some("Real Track"));
    // Exactly one disagreement — album.mbid — not title/artist/etc. The
    // excluded-cohort provider doesn't generate per-field conflicts.
    let fields: Vec<&str> = m.disagreements.iter().map(|d| d.field).collect();
    assert_eq!(fields, vec!["album.mbid"]);
}

#[test]
fn track_present_in_one_provider_only_no_disagreement() {
    let a = provider_result(
        "mb",
        Some(album_with_title("Album")),
        None,
        vec![
            TrackMeta {
                position: 1,
                title: Some("One".into()),
                ..TrackMeta::default()
            },
            TrackMeta {
                position: 2,
                title: Some("Two".into()),
                ..TrackMeta::default()
            },
        ],
    );
    let b = provider_result(
        "other",
        Some(album_with_title("Album")),
        None,
        vec![TrackMeta {
            position: 1,
            title: Some("One".into()),
            ..TrackMeta::default()
        }],
    );
    let m = merge(&[a, b]);
    assert_eq!(m.tracks.len(), 2);
    assert!(m.disagreements.is_empty());
}

#[test]
fn track_title_conflict_records_disagreement_with_position() {
    let a = provider_result(
        "mb",
        Some(album_with_title("Album")),
        None,
        vec![TrackMeta {
            position: 3,
            title: Some("MB Title".into()),
            ..TrackMeta::default()
        }],
    );
    let b = provider_result(
        "other",
        Some(album_with_title("Album")),
        None,
        vec![TrackMeta {
            position: 3,
            title: Some("Other Title".into()),
            ..TrackMeta::default()
        }],
    );
    let m = merge(&[a, b]);
    assert_eq!(m.tracks[0].title.as_deref(), Some("MB Title"));
    assert_eq!(m.disagreements.len(), 1);
    let d = &m.disagreements[0];
    assert_eq!(d.entity, DisagreementEntity::Track { position: 3 });
    assert_eq!(d.field, "track.title");
}

#[test]
fn three_providers_plurality_beats_priority() {
    // Provider "a" (highest priority) says "Wrong"; "b" and "c" agree on
    // "Right". Plurality should win, not priority.
    let a = provider_result("a", Some(album_with_title("Wrong")), None, vec![]);
    let b = provider_result("b", Some(album_with_title("Right")), None, vec![]);
    let c = provider_result("c", Some(album_with_title("Right")), None, vec![]);
    let m = merge(&[a, b, c]);
    assert_eq!(m.album.title.as_deref(), Some("Right"));
}

#[test]
fn empty_results_returns_default_merged() {
    let m = merge(&[]);
    assert!(m.album.title.is_none());
    assert!(m.tracks.is_empty());
    assert!(m.disagreements.is_empty());
    assert!(m.sources.is_empty());
}

fn toc_3track() -> Toc {
    Toc {
        first_track: 1,
        last_track: 3,
        leadout_sector: 30_000,
        track_offsets: vec![150, 10_000, 20_500],
    }
}

#[test]
fn merge_with_toc_fallback_fills_from_toc_when_tracks_empty() {
    // Mimics a Discogs-only match: album + release metadata populated,
    // tracks empty. Fallback should synthesise 3 stubs.
    let discogs_hit = provider_result(
        "discogs",
        Some(AlbumMeta {
            title: Some("8cm Single".into()),
            artist_credit: Some("Some Artist".into()),
            year: Some(1995),
            mbid: None,
        }),
        Some(ReleaseMeta {
            barcode: Some("4945123000185".into()),
            ..ReleaseMeta::default()
        }),
        Vec::new(),
    );

    let toc = toc_3track();
    let merged = merge_with_toc_fallback(&[discogs_hit], &toc);

    assert_eq!(merged.tracks.len(), 3);
    assert_eq!(merged.tracks[0].position, 1);
    assert_eq!(merged.tracks[0].length_frames, Some(9_850));
    assert!(merged.tracks[0].title.is_none());
    assert!(merged.tracks[0].artist_credit.is_none());
    assert!(merged.tracks[0].isrc.is_none());
    assert!(merged.tracks[0].mbid.is_none());

    assert_eq!(merged.tracks[2].position, 3);
    // Last track uses the leadout for its length.
    assert_eq!(merged.tracks[2].length_frames, Some(9_500));

    // Album/release stay untouched by the fallback.
    assert_eq!(merged.album.title.as_deref(), Some("8cm Single"));
}

#[test]
fn merge_with_toc_fallback_does_not_overwrite_existing_tracks() {
    // MB returns 2 of 3 tracks (contrived partial return). The fallback
    // must NOT fire — silently padding would mask the real gap.
    let mb = provider_result(
        "musicbrainz",
        Some(album_with_title("Partial")),
        None,
        vec![
            TrackMeta {
                position: 1,
                title: Some("One".into()),
                ..TrackMeta::default()
            },
            TrackMeta {
                position: 2,
                title: Some("Two".into()),
                ..TrackMeta::default()
            },
        ],
    );
    let toc = toc_3track();
    let merged = merge_with_toc_fallback(&[mb], &toc);

    assert_eq!(merged.tracks.len(), 2);
    assert_eq!(merged.tracks[0].title.as_deref(), Some("One"));
    assert_eq!(merged.tracks[1].title.as_deref(), Some("Two"));
}
