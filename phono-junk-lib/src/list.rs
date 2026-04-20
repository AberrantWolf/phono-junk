//! Catalog listing: flatten `Album × Release` into [`ListRow`] for the CLI
//! `list` subcommand, and enumerate unidentified rips as [`UnidentifiedRow`]
//! so the GUI can surface them alongside identified albums.
//!
//! [`load_list_entries`] returns the unified [`ListEntry`] stream that the
//! GUI consumes; [`load_list_rows`] stays for callers (CLI, unit tests)
//! that only want identified albums.
//!
//! Filtering is client-side over the full row set — fine at the
//! thousands-of-albums scale MVP targets. SQL-side filtering is a later
//! optimisation (see TODO.md).
//!
//! `genre` / `language` are deliberately absent: the schema has no
//! columns for them. See TODO.md Open Questions.

use std::path::PathBuf;

use phono_junk_catalog::{Album, Id};
use phono_junk_db::{DbError, crud};
use rusqlite::Connection;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ListRow {
    pub album_id: Id,
    pub title: String,
    pub artist: Option<String>,
    pub year: Option<u16>,
    pub mbid: Option<String>,
    /// Country of the first matching release (if any).
    pub country: Option<String>,
    /// Label of the first matching release (if any).
    pub label: Option<String>,
    /// ISO 639-3 language of the first matching release (if any).
    /// Drives region-aware CJK font selection in the GUI.
    pub language: Option<String>,
    /// ISO 15924 script of the first matching release (if any).
    pub script: Option<String>,
    pub disc_count: usize,
    pub release_count: usize,
}

/// A scanned rip file with no matched provider — `rip_files.disc_id IS NULL`.
/// Surfaced alongside identified albums in the GUI and as a disjoint mode in
/// the CLI.
#[derive(Debug, Clone, Serialize)]
pub struct UnidentifiedRow {
    pub rip_file_id: Id,
    pub cue_path: Option<PathBuf>,
    pub chd_path: Option<PathBuf>,
}

impl UnidentifiedRow {
    /// Preferred display path: `cue_path` first, falling back to `chd_path`.
    /// `None` only if the `rip_files` row is malformed (both missing).
    pub fn display_path(&self) -> Option<&PathBuf> {
        self.cue_path.as_ref().or(self.chd_path.as_ref())
    }
}

/// One row in the unified album-list view. GUI renders both kinds; CLI's
/// `list` keeps its disjoint output modes and consumes either
/// [`load_list_rows`] or [`crud::list_unidentified_rip_files`] directly.
#[derive(Debug, Clone, Serialize)]
pub enum ListEntry {
    Album(ListRow),
    Unidentified(UnidentifiedRow),
}

#[derive(Debug, Clone)]
pub enum YearSpec {
    Exact(u16),
    Range(u16, u16),
}

impl YearSpec {
    pub fn contains(&self, y: u16) -> bool {
        match self {
            YearSpec::Exact(e) => *e == y,
            YearSpec::Range(lo, hi) => y >= *lo && y <= *hi,
        }
    }

    /// Parse `"1996"` or `"1990-1999"`. Trailing whitespace tolerated.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if let Some((lo, hi)) = s.split_once('-') {
            let lo: u16 = lo
                .trim()
                .parse()
                .map_err(|_| format!("invalid year range lo: {lo:?}"))?;
            let hi: u16 = hi
                .trim()
                .parse()
                .map_err(|_| format!("invalid year range hi: {hi:?}"))?;
            if lo > hi {
                return Err(format!("year range reversed: {lo}-{hi}"));
            }
            Ok(YearSpec::Range(lo, hi))
        } else {
            let y: u16 = s.parse().map_err(|_| format!("invalid year: {s:?}"))?;
            Ok(YearSpec::Exact(y))
        }
    }
}

#[derive(Debug, Clone)]
pub struct ListFilters {
    /// Case-insensitive substring match on `Album.artist_credit`.
    pub artist: Option<String>,
    pub year: Option<YearSpec>,
    /// Exact match on `Release.country` (any release).
    pub country: Option<String>,
    /// Case-insensitive substring on `Release.label` (any release).
    pub label: Option<String>,
    /// When `false`, [`filter_entries`] drops every [`ListEntry::Unidentified`].
    /// Defaults to `true` so unmatched rips are visible by default.
    pub include_unidentified: bool,
}

impl Default for ListFilters {
    fn default() -> Self {
        Self {
            artist: None,
            year: None,
            country: None,
            label: None,
            include_unidentified: true,
        }
    }
}

/// Flatten every album into a [`ListRow`]. One DB query per album for
/// its releases + per release for its discs, so O(albums + releases)
/// queries total. Fine at catalog scale; not intended for huge libraries.
pub fn load_list_rows(conn: &Connection) -> Result<Vec<ListRow>, DbError> {
    let albums = crud::list_albums(conn)?;
    let mut rows = Vec::with_capacity(albums.len());
    for album in albums {
        rows.push(row_for_album(conn, album)?);
    }
    Ok(rows)
}

fn row_for_album(conn: &Connection, album: Album) -> Result<ListRow, DbError> {
    let releases = crud::list_releases_for_album(conn, album.id)?;
    let mut disc_count = 0;
    for r in &releases {
        disc_count += crud::list_discs_for_release(conn, r.id)?.len();
    }
    let (country, label) = releases
        .iter()
        .find_map(|r| {
            let c = r.country.clone();
            let l = r.label.clone();
            if c.is_some() || l.is_some() {
                Some((c, l))
            } else {
                None
            }
        })
        .unwrap_or((None, None));
    let (language, script) = releases
        .iter()
        .find_map(|r| {
            let l = r.language.clone();
            let s = r.script.clone();
            if l.is_some() || s.is_some() {
                Some((l, s))
            } else {
                None
            }
        })
        .unwrap_or((None, None));

    Ok(ListRow {
        album_id: album.id,
        title: album.title,
        artist: album.artist_credit,
        year: album.year,
        mbid: album.mbid,
        country,
        label,
        language,
        script,
        disc_count,
        release_count: releases.len(),
    })
}

/// Apply every populated filter field; empty filters pass everything.
///
/// `ListFilters::include_unidentified` is ignored here — row-only callers
/// (CLI) don't deal with unidentified entries. Use [`filter_entries`] for
/// the mixed stream.
pub fn filter_rows(rows: Vec<ListRow>, f: &ListFilters) -> Vec<ListRow> {
    rows.into_iter().filter(|r| matches(r, f)).collect()
}

/// Load every album row plus every unidentified rip file as a single
/// [`ListEntry`] stream. Identified albums come first (ordered by album id),
/// then unidentified rips (ordered by rip_file id). The GUI consumes this;
/// CLI sticks with [`load_list_rows`] + `crud::list_unidentified_rip_files`.
pub fn load_list_entries(conn: &Connection) -> Result<Vec<ListEntry>, DbError> {
    let mut out: Vec<ListEntry> = load_list_rows(conn)?.into_iter().map(ListEntry::Album).collect();
    for rf in crud::list_unidentified_rip_files(conn)? {
        out.push(ListEntry::Unidentified(UnidentifiedRow {
            rip_file_id: rf.id,
            cue_path: rf.cue_path,
            chd_path: rf.chd_path,
        }));
    }
    Ok(out)
}

/// Apply every populated filter field to the mixed entry stream.
///
/// Unidentified rows pass iff `include_unidentified` is true *and* no
/// populated text/year filter would exclude them. They have no artist /
/// year / country / label, so any set filter of those kinds drops them.
pub fn filter_entries(entries: Vec<ListEntry>, f: &ListFilters) -> Vec<ListEntry> {
    entries
        .into_iter()
        .filter(|e| match e {
            ListEntry::Album(r) => matches(r, f),
            ListEntry::Unidentified(_) => {
                if !f.include_unidentified {
                    return false;
                }
                f.artist.is_none()
                    && f.year.is_none()
                    && f.country.is_none()
                    && f.label.is_none()
            }
        })
        .collect()
}

fn matches(row: &ListRow, f: &ListFilters) -> bool {
    if let Some(needle) = f.artist.as_deref() {
        let hay = row.artist.as_deref().unwrap_or("");
        if !contains_ignore_ascii_case(hay, needle) {
            return false;
        }
    }
    if let Some(spec) = f.year.as_ref() {
        match row.year {
            Some(y) if spec.contains(y) => {}
            _ => return false,
        }
    }
    if let Some(want) = f.country.as_deref() {
        match row.country.as_deref() {
            Some(c) if c.eq_ignore_ascii_case(want) => {}
            _ => return false,
        }
    }
    if let Some(needle) = f.label.as_deref() {
        let hay = row.label.as_deref().unwrap_or("");
        if !contains_ignore_ascii_case(hay, needle) {
            return false;
        }
    }
    true
}

fn contains_ignore_ascii_case(hay: &str, needle: &str) -> bool {
    hay.to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_row(
        id: Id,
        title: &str,
        artist: Option<&str>,
        year: Option<u16>,
        country: Option<&str>,
        label: Option<&str>,
    ) -> ListRow {
        ListRow {
            album_id: id,
            title: title.into(),
            artist: artist.map(Into::into),
            year,
            mbid: None,
            country: country.map(Into::into),
            label: label.map(Into::into),
            language: None,
            script: None,
            disc_count: 1,
            release_count: 1,
        }
    }

    #[test]
    fn year_spec_parse_exact_and_range() {
        assert!(matches!(YearSpec::parse("1996"), Ok(YearSpec::Exact(1996))));
        assert!(matches!(
            YearSpec::parse("1990-1999"),
            Ok(YearSpec::Range(1990, 1999))
        ));
        assert!(YearSpec::parse("abc").is_err());
        assert!(YearSpec::parse("1999-1990").is_err());
    }

    #[test]
    fn filters_artist_case_insensitive() {
        let rows = vec![
            mk_row(1, "A", Some("Weezer"), None, None, None),
            mk_row(2, "B", Some("Blur"), None, None, None),
        ];
        let f = ListFilters {
            artist: Some("weez".into()),
            ..Default::default()
        };
        let out = filter_rows(rows, &f);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].album_id, 1);
    }

    #[test]
    fn filters_year_range_excludes_outside() {
        let rows = vec![
            mk_row(1, "A", None, Some(1995), None, None),
            mk_row(2, "B", None, Some(2005), None, None),
        ];
        let f = ListFilters {
            year: Some(YearSpec::Range(1990, 1999)),
            ..Default::default()
        };
        let out = filter_rows(rows, &f);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].album_id, 1);
    }

    #[test]
    fn filters_country_exact_case_insensitive() {
        let rows = vec![
            mk_row(1, "A", None, None, Some("JP"), None),
            mk_row(2, "B", None, None, Some("US"), None),
        ];
        let f = ListFilters {
            country: Some("jp".into()),
            ..Default::default()
        };
        let out = filter_rows(rows, &f);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn null_year_rejected_when_filter_present() {
        let rows = vec![mk_row(1, "A", None, None, None, None)];
        let f = ListFilters {
            year: Some(YearSpec::Exact(1996)),
            ..Default::default()
        };
        assert_eq!(filter_rows(rows, &f).len(), 0);
    }

    fn mk_unid(id: Id, cue: &str) -> UnidentifiedRow {
        UnidentifiedRow {
            rip_file_id: id,
            cue_path: Some(PathBuf::from(cue)),
            chd_path: None,
        }
    }

    #[test]
    fn filter_entries_default_keeps_unidentified() {
        let entries = vec![
            ListEntry::Album(mk_row(1, "A", Some("Weezer"), Some(1996), None, None)),
            ListEntry::Unidentified(mk_unid(7, "/tmp/a.cue")),
        ];
        let out = filter_entries(entries, &ListFilters::default());
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn filter_entries_hides_unidentified_when_flag_off() {
        let entries = vec![
            ListEntry::Album(mk_row(1, "A", Some("Weezer"), Some(1996), None, None)),
            ListEntry::Unidentified(mk_unid(7, "/tmp/a.cue")),
        ];
        let f = ListFilters {
            include_unidentified: false,
            ..Default::default()
        };
        let out = filter_entries(entries, &f);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], ListEntry::Album(_)));
    }

    #[test]
    fn filter_entries_populated_text_filter_drops_unidentified() {
        let entries = vec![
            ListEntry::Album(mk_row(1, "A", Some("Weezer"), Some(1996), None, None)),
            ListEntry::Unidentified(mk_unid(7, "/tmp/a.cue")),
        ];
        let f = ListFilters {
            artist: Some("weez".into()),
            ..Default::default()
        };
        let out = filter_entries(entries, &f);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], ListEntry::Album(_)));
    }

    #[test]
    fn unidentified_row_display_path_prefers_cue() {
        let r = UnidentifiedRow {
            rip_file_id: 1,
            cue_path: Some(PathBuf::from("/tmp/a.cue")),
            chd_path: Some(PathBuf::from("/tmp/a.chd")),
        };
        assert_eq!(r.display_path().unwrap(), &PathBuf::from("/tmp/a.cue"));

        let r = UnidentifiedRow {
            rip_file_id: 1,
            cue_path: None,
            chd_path: Some(PathBuf::from("/tmp/a.chd")),
        };
        assert_eq!(r.display_path().unwrap(), &PathBuf::from("/tmp/a.chd"));

        let r = UnidentifiedRow {
            rip_file_id: 1,
            cue_path: None,
            chd_path: None,
        };
        assert!(r.display_path().is_none());
    }
}
