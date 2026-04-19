use super::format;
use crate::json::ArtistCredit;

fn credit(name: &str, join: Option<&str>) -> ArtistCredit {
    ArtistCredit {
        name: name.to_string(),
        joinphrase: join.map(String::from),
    }
}

#[test]
fn single_artist_no_joinphrase() {
    let credits = vec![credit("Daft Punk", None)];
    assert_eq!(format(&credits), "Daft Punk");
}

#[test]
fn two_artists_with_ampersand_joinphrase() {
    let credits = vec![credit("Simon", Some(" & ")), credit("Garfunkel", None)];
    assert_eq!(format(&credits), "Simon & Garfunkel");
}

#[test]
fn featured_artist() {
    let credits = vec![credit("Kanye West", Some(" feat. ")), credit("Jay-Z", None)];
    assert_eq!(format(&credits), "Kanye West feat. Jay-Z");
}

#[test]
fn empty_credits_produces_empty_string() {
    assert_eq!(format(&[]), "");
}
