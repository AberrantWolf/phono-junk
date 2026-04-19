//! Output-path planning and filename sanitisation.
//!
//! A single sanitiser drives every path component the extractor writes. If
//! Sprint 15's GUI (or a future CLI dry-run mode) needs to preview where a
//! file will land, it imports the same function instead of reimplementing
//! the rules.

use std::path::{Path, PathBuf};

use phono_junk_catalog::{Album, Track};

/// Replace characters illegal on the union of POSIX + Windows filesystems,
/// trim trailing dots/spaces (Windows cannot address those), collapse runs
/// of whitespace, and return a safe component. Empty results become
/// `"Unknown"` so we never return a bare empty string.
pub fn sanitize_path_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_space = false;
    for c in s.chars() {
        // Whitespace (tabs, newlines, spaces, &c.) collapses to a single
        // ASCII space. Checked before the illegal-char / control-char
        // categories so `\n` and `\t` don't get replaced with `_`.
        if c.is_whitespace() {
            if !last_was_space {
                out.push(' ');
            }
            last_was_space = true;
            continue;
        }
        let ch = match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if (c as u32) < 0x20 => '_',
            _ => c,
        };
        out.push(ch);
        last_was_space = false;
    }
    let trimmed = out.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if trimmed.is_empty() {
        "Unknown".into()
    } else {
        trimmed.to_string()
    }
}

/// Album-level folder name: `<Album> (<Year>)`, or `<Album>` if no year.
pub fn album_folder_name(album: &Album) -> String {
    let title = sanitize_path_component(&album.title);
    match album.year {
        Some(y) => format!("{title} ({y})"),
        None => title,
    }
}

/// Album-artist component (the first directory level inside `library_root`).
///
/// Resolution order:
/// 1. Album's `artist_credit` if set.
/// 2. `"Unknown Artist"` otherwise.
///
/// Caller-side VA detection decides whether to substitute
/// `"Various Artists"` before this runs; this function just sanitises.
pub fn album_artist_component(album_artist: Option<&str>) -> String {
    let raw = album_artist.unwrap_or("Unknown Artist");
    sanitize_path_component(raw)
}

/// Per-track filename stem: `NN - Title`.
pub fn track_file_name(track: &Track) -> String {
    let title = track
        .title
        .as_deref()
        .map(sanitize_path_component)
        .unwrap_or_else(|| "Unknown Track".to_string());
    format!("{:02} - {}.flac", track.position, title)
}

/// Compute every FLAC output path for a disc, rooted at `library_root`.
///
/// For single-disc albums (`total_discs == 1`): tracks sit directly under
/// the album folder. For multi-disc albums (`total_discs > 1`): tracks go
/// under a `Disc N/` subdirectory so disc 1 track 1 and disc 2 track 1
/// don't collide on filename.
///
/// `album_artist_override` lets the caller pre-resolve Various Artists or
/// a user override before hitting the sanitiser. When `None`, falls back
/// to `album.artist_credit`.
pub fn plan_output_paths(
    library_root: &Path,
    album: &Album,
    disc_number: u8,
    total_discs: u8,
    tracks: &[Track],
    album_artist_override: Option<&str>,
) -> Vec<PathBuf> {
    let artist_dir = album_artist_component(
        album_artist_override.or(album.artist_credit.as_deref()),
    );
    let album_dir = album_folder_name(album);
    let base = library_root.join(artist_dir).join(album_dir);
    let disc_dir = if total_discs > 1 {
        base.join(sanitize_path_component(&format!("Disc {disc_number}")))
    } else {
        base
    };
    tracks
        .iter()
        .map(|t| disc_dir.join(track_file_name(t)))
        .collect()
}

/// Same resolution as [`plan_output_paths`] but returns only the
/// album-folder path (single-disc) or the per-disc subfolder path
/// (multi-disc). Used by the orchestrator to know where to drop
/// `cover.jpg`.
pub fn plan_disc_directory(
    library_root: &Path,
    album: &Album,
    disc_number: u8,
    total_discs: u8,
    album_artist_override: Option<&str>,
) -> PathBuf {
    let artist_dir = album_artist_component(
        album_artist_override.or(album.artist_credit.as_deref()),
    );
    let album_dir = album_folder_name(album);
    let base = library_root.join(artist_dir).join(album_dir);
    if total_discs > 1 {
        base.join(sanitize_path_component(&format!("Disc {disc_number}")))
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_album(title: &str, artist: Option<&str>, year: Option<u16>) -> Album {
        Album {
            id: 0,
            title: title.into(),
            sort_title: None,
            artist_credit: artist.map(String::from),
            year,
            mbid: None,
            primary_type: None,
            secondary_types: Vec::new(),
            first_release_date: None,
        }
    }

    fn mk_track(position: u8, title: &str) -> Track {
        Track {
            id: 0,
            disc_id: 0,
            position,
            title: Some(title.into()),
            artist_credit: None,
            length_frames: None,
            isrc: None,
            mbid: None,
            recording_mbid: None,
        }
    }

    #[test]
    fn sanitizer_replaces_posix_and_windows_illegals() {
        assert_eq!(sanitize_path_component("a/b"), "a_b");
        assert_eq!(sanitize_path_component("a:b?c"), "a_b_c");
        assert_eq!(sanitize_path_component("<foo>"), "_foo_");
        assert_eq!(sanitize_path_component("a\\b"), "a_b");
        assert_eq!(sanitize_path_component("quote\"me"), "quote_me");
        assert_eq!(sanitize_path_component("pipe|sep"), "pipe_sep");
    }

    #[test]
    fn sanitizer_strips_control_chars() {
        assert_eq!(sanitize_path_component("a\u{0001}b"), "a_b");
        assert_eq!(sanitize_path_component("a\nb"), "a b");
    }

    #[test]
    fn sanitizer_trims_trailing_dots_and_spaces() {
        assert_eq!(sanitize_path_component("hello..."), "hello");
        assert_eq!(sanitize_path_component("  spaced  "), "spaced");
        assert_eq!(sanitize_path_component(". dotty . "), "dotty");
    }

    #[test]
    fn sanitizer_collapses_whitespace_runs() {
        assert_eq!(sanitize_path_component("a    b\t\tc"), "a b c");
    }

    #[test]
    fn sanitizer_returns_unknown_for_empty() {
        assert_eq!(sanitize_path_component(""), "Unknown");
        assert_eq!(sanitize_path_component("...  "), "Unknown");
    }

    #[test]
    fn album_folder_includes_year_when_set() {
        let a = mk_album("Pinkerton", Some("Weezer"), Some(1996));
        assert_eq!(album_folder_name(&a), "Pinkerton (1996)");
    }

    #[test]
    fn album_folder_omits_year_when_none() {
        let a = mk_album("Pinkerton", Some("Weezer"), None);
        assert_eq!(album_folder_name(&a), "Pinkerton");
    }

    #[test]
    fn track_filename_zero_pads_position() {
        let t = mk_track(3, "Tired of Sex");
        assert_eq!(track_file_name(&t), "03 - Tired of Sex.flac");
    }

    #[test]
    fn plan_single_disc_puts_tracks_in_album_dir() {
        let root = PathBuf::from("/lib");
        let a = mk_album("Pinkerton", Some("Weezer"), Some(1996));
        let tracks = vec![mk_track(1, "Tired of Sex"), mk_track(2, "Getchoo")];
        let paths = plan_output_paths(&root, &a, 1, 1, &tracks, None);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/lib/Weezer/Pinkerton (1996)/01 - Tired of Sex.flac"),
                PathBuf::from("/lib/Weezer/Pinkerton (1996)/02 - Getchoo.flac"),
            ]
        );
    }

    #[test]
    fn plan_multi_disc_adds_disc_subdirs() {
        let root = PathBuf::from("/lib");
        let a = mk_album("The Wall", Some("Pink Floyd"), Some(1979));
        let t1 = vec![mk_track(1, "In the Flesh?")];
        let t2 = vec![mk_track(1, "Hey You")];
        let p1 = plan_output_paths(&root, &a, 1, 2, &t1, None);
        let p2 = plan_output_paths(&root, &a, 2, 2, &t2, None);
        assert_eq!(
            p1[0],
            PathBuf::from("/lib/Pink Floyd/The Wall (1979)/Disc 1/01 - In the Flesh_.flac")
        );
        assert_eq!(
            p2[0],
            PathBuf::from("/lib/Pink Floyd/The Wall (1979)/Disc 2/01 - Hey You.flac")
        );
    }

    #[test]
    fn plan_uses_unknown_artist_when_none() {
        let root = PathBuf::from("/lib");
        let a = mk_album("Mystery", None, None);
        let tracks = vec![mk_track(1, "Track One")];
        let paths = plan_output_paths(&root, &a, 1, 1, &tracks, None);
        assert_eq!(
            paths[0],
            PathBuf::from("/lib/Unknown Artist/Mystery/01 - Track One.flac")
        );
    }

    #[test]
    fn plan_honors_album_artist_override() {
        let root = PathBuf::from("/lib");
        let a = mk_album("Mixtape", Some("DJ Shadow"), Some(2000));
        let tracks = vec![mk_track(1, "Track One")];
        let paths =
            plan_output_paths(&root, &a, 1, 1, &tracks, Some("Various Artists"));
        assert_eq!(
            paths[0],
            PathBuf::from("/lib/Various Artists/Mixtape (2000)/01 - Track One.flac")
        );
    }

    #[test]
    fn plan_disc_directory_matches_plan_output_paths_parent() {
        let root = PathBuf::from("/lib");
        let a = mk_album("The Wall", Some("Pink Floyd"), Some(1979));
        let tracks = vec![mk_track(1, "Hey You")];
        let expected_dir = plan_disc_directory(&root, &a, 2, 2, None);
        let expected_file = plan_output_paths(&root, &a, 2, 2, &tracks, None)[0].clone();
        assert_eq!(expected_file.parent().unwrap(), expected_dir);
    }
}
