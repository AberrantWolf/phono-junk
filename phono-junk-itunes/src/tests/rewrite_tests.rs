use super::rewrite_artwork_size;

#[test]
fn rewrites_jpg_100_to_1000() {
    let input =
        "https://is1-ssl.mzstatic.com/image/thumb/Music/v4/00/source/100x100bb.jpg";
    let out = rewrite_artwork_size(input);
    assert!(out.ends_with("1000x1000bb.jpg"), "got {out}");
}

#[test]
fn rewrites_png_100_to_1000() {
    let input =
        "https://is1-ssl.mzstatic.com/image/thumb/Music/v4/00/source/100x100bb.png";
    let out = rewrite_artwork_size(input);
    assert!(out.ends_with("1000x1000bb.png"), "got {out}");
}

#[test]
fn unrelated_url_passes_through() {
    let input = "https://example.com/some/other/path.jpg";
    assert_eq!(rewrite_artwork_size(input), input);
}

#[test]
fn rewrite_is_idempotent() {
    let input =
        "https://is1-ssl.mzstatic.com/image/thumb/Music/v4/00/source/100x100bb.jpg";
    let once = rewrite_artwork_size(input);
    let twice = rewrite_artwork_size(&once);
    assert_eq!(once, twice);
}
