//! Unit tests for the `Toc` arithmetic helpers.
//!
//! These replace three hand-rolled copies of the `next_offset - start (or
//! leadout)` math — one each in the GUI detail panel and the consensus-merge
//! TOC fallback. Worth pinning once at the owner so a future refactor can
//! change the implementation in one place.

use super::*;

fn toc_3track() -> Toc {
    // 3-track audio CD. Standard 150-frame (2s) pregap on track 1.
    // Track 1: sectors 150..10_000  → length 9_850
    // Track 2: sectors 10_000..20_500 → length 10_500
    // Track 3: sectors 20_500..30_000 → length 9_500
    // Leadout: 30_000
    Toc {
        first_track: 1,
        last_track: 3,
        leadout_sector: 30_000,
        track_offsets: vec![150, 10_000, 20_500],
    }
}

#[test]
fn track_length_frames_middle_uses_next_offset() {
    let toc = toc_3track();
    assert_eq!(toc.track_length_frames(0), Some(9_850));
    assert_eq!(toc.track_length_frames(1), Some(10_500));
}

#[test]
fn track_length_frames_last_uses_leadout() {
    let toc = toc_3track();
    assert_eq!(toc.track_length_frames(2), Some(9_500));
}

#[test]
fn total_length_frames_matches_manual_math() {
    let toc = toc_3track();
    // 30_000 leadout - 150 first_track_start = 29_850 frames.
    assert_eq!(toc.total_length_frames(), 29_850);
    assert_eq!(toc.track_count(), 3);

    let spans: Vec<TrackSpan> = toc.iter_track_spans().collect();
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].position, 1);
    assert_eq!(spans[0].start_sector, 150);
    assert_eq!(spans[0].length_frames, 9_850);
    assert_eq!(spans[2].position, 3);
    assert_eq!(spans[2].length_frames, 9_500);
}
