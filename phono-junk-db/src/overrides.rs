//! `Override.sub_path` grammar, parser, and applier.
//!
//! Grammar (dot-separated segments):
//! - `field` — direct field on the target (e.g. `title`).
//! - `name[index]` — indexed segment into a sibling collection
//!   (e.g. `track[6]` targets the 6th track under a [`OverrideTarget::Disc`];
//!   1-indexed per CLAUDE.md's `track[6].title` example).
//!
//! Supported paths today:
//! - Empty path + any target → set a flat field on that entity.
//! - `track[N]` + [`OverrideTarget::Disc`] → select `tracks[N-1]`, then set a
//!   flat field on it.
//!
//! Deeper nesting (`release[0].disc[1].track[2].title`) is deferred until a
//! user workflow actually needs it; MVP overrides are flat corrections.

use phono_junk_catalog::{Album, Disc, Override, Release, RipFile, Track};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OverrideError {
    #[error("malformed sub_path: {0}")]
    MalformedPath(String),
    #[error("unknown segment `{segment}` on {target}")]
    UnknownSegment { target: &'static str, segment: String },
    #[error("index {index} out of range on `{segment}` (len {len})")]
    IndexOutOfRange {
        segment: String,
        index: usize,
        len: usize,
    },
    #[error("unknown field `{field}` on {target}")]
    UnknownField { target: &'static str, field: String },
    #[error("cannot parse `{value}` as {expected} for field `{field}`")]
    ValueParse {
        field: String,
        expected: &'static str,
        value: String,
    },
    #[error("sub_path `{path}` is not applicable to target {target}")]
    TargetMismatch { target: &'static str, path: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Field(String),
    Indexed { name: String, index: usize },
}

/// Parse a dot-separated sub_path. Returns an empty Vec for `None`/empty input.
pub fn parse_sub_path(input: Option<&str>) -> Result<Vec<Segment>, OverrideError> {
    let s = match input {
        None => return Ok(Vec::new()),
        Some(s) if s.is_empty() => return Ok(Vec::new()),
        Some(s) => s,
    };
    s.split('.').map(parse_segment).collect()
}

fn parse_segment(raw: &str) -> Result<Segment, OverrideError> {
    if raw.is_empty() {
        return Err(OverrideError::MalformedPath(
            "empty segment (leading/trailing/duplicate dot)".into(),
        ));
    }
    match raw.find('[') {
        None => {
            if raw.contains(']') {
                return Err(OverrideError::MalformedPath(format!(
                    "unmatched `]` in `{raw}`"
                )));
            }
            Ok(Segment::Field(raw.to_string()))
        }
        Some(open) => {
            let close = raw
                .rfind(']')
                .ok_or_else(|| OverrideError::MalformedPath(format!("unterminated `[` in `{raw}`")))?;
            if close != raw.len() - 1 {
                return Err(OverrideError::MalformedPath(format!(
                    "trailing text after `]` in `{raw}`"
                )));
            }
            let name = &raw[..open];
            let idx_str = &raw[open + 1..close];
            if name.is_empty() {
                return Err(OverrideError::MalformedPath(format!(
                    "missing name before `[` in `{raw}`"
                )));
            }
            let index: usize = idx_str.parse().map_err(|_| {
                OverrideError::MalformedPath(format!("non-numeric index in `{raw}`"))
            })?;
            Ok(Segment::Indexed {
                name: name.to_string(),
                index,
            })
        }
    }
}

/// Target of an override application. Entities with sibling collections
/// (currently just `Disc`) carry those slices alongside the parent struct so
/// the applier can navigate `track[N]` without re-loading from DB.
pub enum OverrideTarget<'a> {
    Album(&'a mut Album),
    Release(&'a mut Release),
    Disc {
        disc: &'a mut Disc,
        tracks: &'a mut [Track],
    },
    Track(&'a mut Track),
    RipFile(&'a mut RipFile),
}

/// Apply an override to the given target. `path` is the parsed `sub_path`;
/// `field` and `value` come from the [`Override`] row.
pub fn apply_override(
    target: OverrideTarget<'_>,
    path: &[Segment],
    field: &str,
    value: &str,
) -> Result<(), OverrideError> {
    match target {
        OverrideTarget::Album(album) => apply_flat_album(album, path, field, value),
        OverrideTarget::Release(release) => apply_flat_release(release, path, field, value),
        OverrideTarget::Disc { disc, tracks } => apply_disc(disc, tracks, path, field, value),
        OverrideTarget::Track(track) => apply_flat_track(track, path, field, value),
        OverrideTarget::RipFile(file) => apply_flat_rip_file(file, path, field, value),
    }
}

/// Convenience: parse + apply in one call.
pub fn apply(target: OverrideTarget<'_>, ovr: &Override) -> Result<(), OverrideError> {
    let path = parse_sub_path(ovr.sub_path.as_deref())?;
    apply_override(target, &path, &ovr.field, &ovr.override_value)
}

// ---------------------------------------------------------------------------
// Per-target appliers. Each handles empty path + flat field. Disc additionally
// handles `track[N]` navigation.
// ---------------------------------------------------------------------------

fn reject_nonempty(path: &[Segment], target: &'static str) -> Result<(), OverrideError> {
    if path.is_empty() {
        Ok(())
    } else {
        let rendered = path
            .iter()
            .map(|s| match s {
                Segment::Field(f) => f.clone(),
                Segment::Indexed { name, index } => format!("{name}[{index}]"),
            })
            .collect::<Vec<_>>()
            .join(".");
        Err(OverrideError::TargetMismatch {
            target,
            path: rendered,
        })
    }
}

fn apply_flat_album(
    album: &mut Album,
    path: &[Segment],
    field: &str,
    value: &str,
) -> Result<(), OverrideError> {
    reject_nonempty(path, "Album")?;
    match field {
        "title" => album.title = value.to_string(),
        "sort_title" => album.sort_title = nullable(value),
        "artist_credit" => album.artist_credit = nullable(value),
        "year" => album.year = parse_opt_u16(field, value)?,
        "mbid" => album.mbid = nullable(value),
        "primary_type" => album.primary_type = nullable(value),
        "first_release_date" => album.first_release_date = nullable(value),
        other => {
            return Err(OverrideError::UnknownField {
                target: "Album",
                field: other.to_string(),
            });
        }
    }
    Ok(())
}

fn apply_flat_release(
    release: &mut Release,
    path: &[Segment],
    field: &str,
    value: &str,
) -> Result<(), OverrideError> {
    reject_nonempty(path, "Release")?;
    match field {
        "country" => release.country = nullable(value),
        "date" => release.date = nullable(value),
        "label" => release.label = nullable(value),
        "catalog_number" => release.catalog_number = nullable(value),
        "barcode" => release.barcode = nullable(value),
        "mbid" => release.mbid = nullable(value),
        "status" => release.status = nullable(value),
        "language" => release.language = nullable(value),
        "script" => release.script = nullable(value),
        other => {
            return Err(OverrideError::UnknownField {
                target: "Release",
                field: other.to_string(),
            });
        }
    }
    Ok(())
}

fn apply_disc(
    disc: &mut Disc,
    tracks: &mut [Track],
    path: &[Segment],
    field: &str,
    value: &str,
) -> Result<(), OverrideError> {
    match path.split_first() {
        None => apply_flat_disc(disc, field, value),
        Some((Segment::Indexed { name, index }, rest)) if name == "track" => {
            let idx_one_based = *index;
            if idx_one_based == 0 {
                return Err(OverrideError::IndexOutOfRange {
                    segment: format!("track[{idx_one_based}]"),
                    index: idx_one_based,
                    len: tracks.len(),
                });
            }
            let zero = idx_one_based - 1;
            let len = tracks.len();
            let track = tracks.get_mut(zero).ok_or(OverrideError::IndexOutOfRange {
                segment: format!("track[{idx_one_based}]"),
                index: idx_one_based,
                len,
            })?;
            apply_flat_track(track, rest, field, value)
        }
        Some((seg, _)) => Err(OverrideError::UnknownSegment {
            target: "Disc",
            segment: segment_display(seg),
        }),
    }
}

fn apply_flat_disc(disc: &mut Disc, field: &str, value: &str) -> Result<(), OverrideError> {
    match field {
        "format" => disc.format = value.to_string(),
        "mb_discid" => disc.mb_discid = nullable(value),
        "cddb_id" => disc.cddb_id = nullable(value),
        "ar_discid1" => disc.ar_discid1 = nullable(value),
        "ar_discid2" => disc.ar_discid2 = nullable(value),
        other => {
            return Err(OverrideError::UnknownField {
                target: "Disc",
                field: other.to_string(),
            });
        }
    }
    Ok(())
}

fn apply_flat_track(
    track: &mut Track,
    path: &[Segment],
    field: &str,
    value: &str,
) -> Result<(), OverrideError> {
    reject_nonempty(path, "Track")?;
    match field {
        "title" => track.title = nullable(value),
        "artist_credit" => track.artist_credit = nullable(value),
        "length_frames" => track.length_frames = parse_opt_u64(field, value)?,
        "isrc" => track.isrc = nullable(value),
        "mbid" => track.mbid = nullable(value),
        "recording_mbid" => track.recording_mbid = nullable(value),
        other => {
            return Err(OverrideError::UnknownField {
                target: "Track",
                field: other.to_string(),
            });
        }
    }
    Ok(())
}

fn apply_flat_rip_file(
    file: &mut RipFile,
    path: &[Segment],
    field: &str,
    value: &str,
) -> Result<(), OverrideError> {
    reject_nonempty(path, "RipFile")?;
    match field {
        "accuraterip_status" => file.accuraterip_status = nullable(value),
        "last_verified_at" => file.last_verified_at = nullable(value),
        other => {
            return Err(OverrideError::UnknownField {
                target: "RipFile",
                field: other.to_string(),
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Treat empty string as `None`; anything else as `Some(value)`. Overrides
/// without an explicit null marker use empty-string to clear a field.
fn nullable(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_opt_u16(field: &str, value: &str) -> Result<Option<u16>, OverrideError> {
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<u16>()
        .map(Some)
        .map_err(|_| OverrideError::ValueParse {
            field: field.to_string(),
            expected: "u16",
            value: value.to_string(),
        })
}

fn parse_opt_u64(field: &str, value: &str) -> Result<Option<u64>, OverrideError> {
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<u64>()
        .map(Some)
        .map_err(|_| OverrideError::ValueParse {
            field: field.to_string(),
            expected: "u64",
            value: value.to_string(),
        })
}

fn segment_display(seg: &Segment) -> String {
    match seg {
        Segment::Field(f) => f.clone(),
        Segment::Indexed { name, index } => format!("{name}[{index}]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty() {
        assert_eq!(parse_sub_path(None).unwrap(), Vec::<Segment>::new());
        assert_eq!(parse_sub_path(Some("")).unwrap(), Vec::<Segment>::new());
    }

    #[test]
    fn parse_flat_field() {
        assert_eq!(
            parse_sub_path(Some("title")).unwrap(),
            vec![Segment::Field("title".into())]
        );
    }

    #[test]
    fn parse_indexed() {
        assert_eq!(
            parse_sub_path(Some("track[6]")).unwrap(),
            vec![Segment::Indexed {
                name: "track".into(),
                index: 6,
            }]
        );
    }

    #[test]
    fn parse_compound() {
        assert_eq!(
            parse_sub_path(Some("track[6].title")).unwrap(),
            vec![
                Segment::Indexed {
                    name: "track".into(),
                    index: 6,
                },
                Segment::Field("title".into()),
            ]
        );
    }

    #[test]
    fn parse_unterminated_bracket() {
        assert!(matches!(
            parse_sub_path(Some("track[6")).unwrap_err(),
            OverrideError::MalformedPath(_)
        ));
    }

    #[test]
    fn parse_non_numeric_index() {
        assert!(matches!(
            parse_sub_path(Some("track[abc]")).unwrap_err(),
            OverrideError::MalformedPath(_)
        ));
    }

    #[test]
    fn parse_empty_segment() {
        assert!(matches!(
            parse_sub_path(Some("track[6]..title")).unwrap_err(),
            OverrideError::MalformedPath(_)
        ));
    }

    #[test]
    fn parse_trailing_after_close() {
        assert!(matches!(
            parse_sub_path(Some("track[6]x")).unwrap_err(),
            OverrideError::MalformedPath(_)
        ));
    }

    #[test]
    fn parse_missing_name_before_bracket() {
        assert!(matches!(
            parse_sub_path(Some("[6]")).unwrap_err(),
            OverrideError::MalformedPath(_)
        ));
    }
}
