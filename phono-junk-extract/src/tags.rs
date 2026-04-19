//! Vorbis-comment tag set for the 12-tag spec (plus optional ISRC bonus).
//!
//! The canonical CD-rip tag set: `ALBUM`, `ALBUMARTIST`, `ARTIST`, `TITLE`,
//! `TRACKNUMBER`, `TOTALTRACKS`, `DISCNUMBER`, `TOTALDISCS`, `DATE`, `GENRE`,
//! `MUSICBRAINZ_ALBUMID`, `MUSICBRAINZ_RELEASETRACKID`. `ISRC` is emitted
//! when populated.
//!
//! Missing-field rule: tags with `None` values are dropped rather than
//! written as empty strings. Consumers expecting every tag to be present
//! should check `Option` on the source rows before constructing a
//! [`TrackTags`].

/// Typed tag set for one FLAC track.
///
/// Required fields are always written. Optional fields are only emitted
/// when `Some(..)`; `None` means the upstream catalog had no value for
/// that field and we decline to fabricate one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackTags {
    pub album: String,
    pub album_artist: String,
    pub artist: String,
    pub title: String,
    pub track_number: u8,
    pub total_tracks: u8,
    pub disc_number: u8,
    pub total_discs: u8,
    /// MusicBrainz `first-release-date` — either `YYYY` or `YYYY-MM-DD`.
    pub date: Option<String>,
    pub genre: Option<String>,
    pub musicbrainz_album_id: Option<String>,
    pub musicbrainz_release_track_id: Option<String>,
    pub isrc: Option<String>,
}

impl TrackTags {
    /// Emit the full `(key, value)` list for these tags in a stable order.
    ///
    /// Order follows the spec ordering: required fields first, then
    /// optional fields in the listed order. `None` values are skipped.
    pub fn to_vorbis_comments(&self) -> Vec<(&'static str, String)> {
        let mut out: Vec<(&'static str, String)> = Vec::with_capacity(13);
        out.push(("ALBUM", self.album.clone()));
        out.push(("ALBUMARTIST", self.album_artist.clone()));
        out.push(("ARTIST", self.artist.clone()));
        out.push(("TITLE", self.title.clone()));
        out.push(("TRACKNUMBER", self.track_number.to_string()));
        out.push(("TOTALTRACKS", self.total_tracks.to_string()));
        out.push(("DISCNUMBER", self.disc_number.to_string()));
        out.push(("TOTALDISCS", self.total_discs.to_string()));
        if let Some(d) = &self.date {
            out.push(("DATE", d.clone()));
        }
        if let Some(g) = &self.genre {
            out.push(("GENRE", g.clone()));
        }
        if let Some(m) = &self.musicbrainz_album_id {
            out.push(("MUSICBRAINZ_ALBUMID", m.clone()));
        }
        if let Some(m) = &self.musicbrainz_release_track_id {
            out.push(("MUSICBRAINZ_RELEASETRACKID", m.clone()));
        }
        if let Some(i) = &self.isrc {
            out.push(("ISRC", i.clone()));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TrackTags {
        TrackTags {
            album: "Weezer".into(),
            album_artist: "Weezer".into(),
            artist: "Weezer".into(),
            title: "Buddy Holly".into(),
            track_number: 4,
            total_tracks: 10,
            disc_number: 1,
            total_discs: 1,
            date: Some("1994-05-10".into()),
            genre: None,
            musicbrainz_album_id: Some("abc".into()),
            musicbrainz_release_track_id: Some("def".into()),
            isrc: Some("USSM10000123".into()),
        }
    }

    #[test]
    fn emits_required_and_present_optional_fields() {
        let tags = sample();
        let emitted = tags.to_vorbis_comments();
        let keys: Vec<_> = emitted.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            vec![
                "ALBUM",
                "ALBUMARTIST",
                "ARTIST",
                "TITLE",
                "TRACKNUMBER",
                "TOTALTRACKS",
                "DISCNUMBER",
                "TOTALDISCS",
                "DATE",
                "MUSICBRAINZ_ALBUMID",
                "MUSICBRAINZ_RELEASETRACKID",
                "ISRC",
            ]
        );
    }

    #[test]
    fn skips_none_optional_fields() {
        let mut tags = sample();
        tags.date = None;
        tags.musicbrainz_album_id = None;
        tags.musicbrainz_release_track_id = None;
        tags.isrc = None;
        let emitted = tags.to_vorbis_comments();
        let keys: Vec<_> = emitted.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            keys,
            vec![
                "ALBUM",
                "ALBUMARTIST",
                "ARTIST",
                "TITLE",
                "TRACKNUMBER",
                "TOTALTRACKS",
                "DISCNUMBER",
                "TOTALDISCS",
            ]
        );
    }

    #[test]
    fn numeric_fields_are_stringified() {
        let tags = sample();
        let map: std::collections::HashMap<&str, String> =
            tags.to_vorbis_comments().into_iter().collect();
        assert_eq!(map["TRACKNUMBER"], "4");
        assert_eq!(map["TOTALTRACKS"], "10");
        assert_eq!(map["DISCNUMBER"], "1");
        assert_eq!(map["TOTALDISCS"], "1");
    }
}
