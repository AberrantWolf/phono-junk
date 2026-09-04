//! Opt-in end-to-end oracle for the external 18-track Redumper package.
//!
//! The package itself is intentionally not committed. Set
//! `JUNK_LIBS_REDUMPER_AUDIO_PREFIX` to the package prefix (without an
//! extension), then run:
//!
//! `cargo test -p phono-junk-accuraterip --test redumper_fixture_live -- --ignored`

use std::path::PathBuf;

use junk_libs_disc::TrackPcmReader;
use phono_junk_accuraterip::{
    AccurateRipClient, DiscTrackSamples, VerificationOptions, VerificationStatus,
    verify_with_offsets,
};
use phono_junk_core::DiscIds;

#[test]
#[ignore = "requires the external Redumper audio package and live AccurateRip"]
fn real_18_track_redumper_package_matches_independent_dbar_oracle() {
    let prefix = std::env::var_os("JUNK_LIBS_REDUMPER_AUDIO_PREFIX")
        .map(PathBuf::from)
        .expect("set JUNK_LIBS_REDUMPER_AUDIO_PREFIX to the package prefix");
    let cue_path = prefix.with_extension("cue");
    let tracks = (1..=18)
        .map(|position| {
            let reader = TrackPcmReader::from_cue(&cue_path, position)
                .expect("external Redumper CUE/BIN layout should decode");
            let samples = reader
                .flat_map(|sector| sector.expect("external Redumper PCM sector should decode"))
                .collect();
            DiscTrackSamples { position, samples }
        })
        .collect::<Vec<_>>();

    let ids = DiscIds {
        ar_discid1: Some("0027c722".into()),
        ar_discid2: Some("020f8093".into()),
        cddb_id: Some("e70df812".into()),
        ..DiscIds::default()
    };
    let client =
        AccurateRipClient::new("phono-junk-fixture/0.1 ( test@example.com )").expect("HTTP client");
    let dbar = client
        .fetch_dbar(&ids, 18)
        .expect("live dBAR request should succeed")
        .expect("fixture disc should exist in AccurateRip");

    let result = verify_with_offsets(&dbar, &tracks, VerificationOptions::default());
    assert_eq!(result.status, VerificationStatus::Verified);
    assert_eq!(result.tracks.len(), 18);
    assert!(result.tracks.iter().all(|track| track.is_verified()));
    assert!(result.tracks.iter().all(|track| {
        track
            .v2_matches
            .iter()
            .any(|matched| matched.confidence >= 8)
    }));
}
