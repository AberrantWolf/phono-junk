//! End-to-end integration: real `.cue` file on disk → `Toc` →
//! `compute_disc_ids` → published DiscID strings.
//!
//! The accompanying `.bin` files are created as sparse files on first
//! run (see `tests/fixtures/README.md`) so no large binary is committed.
//!
//! Expected DiscID values come from Sprint 1's fixtures, which in turn
//! come from ARver's authoritative `discinfo_test` suite:
//! <https://github.com/arcctgx/ARver/blob/master/tests/discinfo_test.py>.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use phono_junk_toc::{compute_disc_ids, read_toc_from_cue};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Create `path` as a sparse file of exactly `len` bytes if it doesn't
/// already exist at that size. Subsequent calls are no-ops.
fn ensure_sparse_file(path: &Path, len: u64) -> io::Result<()> {
    if let Ok(meta) = std::fs::metadata(path) {
        if meta.len() == len {
            return Ok(());
        }
    }
    let file: File = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    file.set_len(len)?;
    Ok(())
}

fn setup_arver_3track_bin() -> &'static io::Result<()> {
    static ONCE: OnceLock<io::Result<()>> = OnceLock::new();
    ONCE.get_or_init(|| {
        ensure_sparse_file(
            &fixtures_dir().join("arver_3track.bin"),
            335_953 * 2352,
        )
    })
}

fn setup_cd_extra_bin() -> &'static io::Result<()> {
    static ONCE: OnceLock<io::Result<()>> = OnceLock::new();
    ONCE.get_or_init(|| {
        ensure_sparse_file(
            &fixtures_dir().join("cd_extra_synth.bin"),
            347_953 * 2352,
        )
    })
}

#[test]
fn cue_to_toc_to_discids_matches_arver_3track() {
    setup_arver_3track_bin()
        .as_ref()
        .expect("sparse BIN setup failed");

    let toc = read_toc_from_cue(&fixtures_dir().join("arver_3track.cue")).unwrap();

    assert_eq!(toc.first_track, 1);
    assert_eq!(toc.last_track, 3);
    assert_eq!(toc.track_offsets, vec![150, 75408, 130223]);
    assert_eq!(toc.leadout_sector, 336103);

    let ids = compute_disc_ids(&toc);
    assert_eq!(
        ids.mb_discid.as_deref(),
        Some("dUmct3Sk4dAt1a98qUKYKC0ZjYU-")
    );
    assert_eq!(ids.cddb_id.as_deref(), Some("19117f03"));
    assert_eq!(ids.ar_discid1.as_deref(), Some("00084264"));
    assert_eq!(ids.ar_discid2.as_deref(), Some("001cc184"));
}

#[test]
fn cue_cd_extra_reproduces_audio_discids() {
    setup_cd_extra_bin()
        .as_ref()
        .expect("sparse BIN setup failed");

    let toc = read_toc_from_cue(&fixtures_dir().join("cd_extra_synth.cue")).unwrap();

    // After the -11,400 correction, the audio portion of a CD-Extra disc
    // must produce exactly the same Toc as the pure-audio variant.
    assert_eq!(toc.last_track, 3);
    assert_eq!(toc.track_offsets, vec![150, 75408, 130223]);
    assert_eq!(toc.leadout_sector, 336103);

    let ids = compute_disc_ids(&toc);
    assert_eq!(
        ids.mb_discid.as_deref(),
        Some("dUmct3Sk4dAt1a98qUKYKC0ZjYU-")
    );
    assert_eq!(ids.cddb_id.as_deref(), Some("19117f03"));
    assert_eq!(ids.ar_discid1.as_deref(), Some("00084264"));
    assert_eq!(ids.ar_discid2.as_deref(), Some("001cc184"));
}
