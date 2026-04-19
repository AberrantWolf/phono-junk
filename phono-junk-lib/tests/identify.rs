//! End-to-end identify-pipeline tests with an in-memory SQLite catalog and
//! mock providers. Covers cache hits, disagreement persistence, override
//! application, asset deduplication, and MBID-cohort isolation.

use phono_junk_catalog::Override;
use phono_junk_core::{DiscIds, Toc};
use phono_junk_db::{crud, open_memory};
use phono_junk_identify::{
    AlbumMeta, AssetCandidate, AssetConfidence, AssetLookupCtx, AssetProvider, AssetType,
    Credentials, DiscIdKind, IdentificationProvider, ProviderError, ProviderResult, ReleaseMeta,
    TrackMeta,
};
use phono_junk_lib::PhonoContext;
use rusqlite::Connection;
use url::Url;

// -----------------------------------------------------------------------
// Mock providers
// -----------------------------------------------------------------------

struct MockIdentifier {
    name: &'static str,
    result: Option<ProviderResult>,
}
impl IdentificationProvider for MockIdentifier {
    fn name(&self) -> &'static str {
        self.name
    }
    fn supported_ids(&self) -> &[DiscIdKind] {
        &[DiscIdKind::MbDiscId]
    }
    fn lookup(
        &self,
        _toc: &Toc,
        _ids: &DiscIds,
        _creds: &Credentials,
    ) -> Result<Option<ProviderResult>, ProviderError> {
        Ok(self.result.clone())
    }
}

struct CountingMock {
    name: &'static str,
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    result: Option<ProviderResult>,
}
impl IdentificationProvider for CountingMock {
    fn name(&self) -> &'static str {
        self.name
    }
    fn supported_ids(&self) -> &[DiscIdKind] {
        &[DiscIdKind::MbDiscId]
    }
    fn lookup(
        &self,
        _toc: &Toc,
        _ids: &DiscIds,
        _creds: &Credentials,
    ) -> Result<Option<ProviderResult>, ProviderError> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self.result.clone())
    }
}

struct MockAssetProvider {
    name: &'static str,
    candidates: Vec<AssetCandidate>,
}
impl AssetProvider for MockAssetProvider {
    fn name(&self) -> &'static str {
        self.name
    }
    fn asset_types(&self) -> &[AssetType] {
        &[AssetType::FrontCover]
    }
    fn lookup_art(&self, _ctx: &AssetLookupCtx<'_>) -> Result<Vec<AssetCandidate>, ProviderError> {
        Ok(self.candidates.clone())
    }
}

// -----------------------------------------------------------------------
// Fixtures
// -----------------------------------------------------------------------

fn sample_toc() -> Toc {
    Toc {
        first_track: 1,
        last_track: 3,
        leadout_sector: 200_000,
        track_offsets: vec![0, 50_000, 100_000],
    }
}

fn sample_ids() -> DiscIds {
    DiscIds {
        mb_discid: Some("disc-abc".into()),
        cddb_id: Some("1a2b3c4d".into()),
        ar_discid1: Some("deadbeef".into()),
        ar_discid2: Some("cafebabe".into()),
        ..Default::default()
    }
}

fn mb_result() -> ProviderResult {
    ProviderResult {
        album: Some(AlbumMeta {
            title: Some("Real Album".into()),
            artist_credit: Some("Real Artist".into()),
            year: Some(2020),
            mbid: Some("album-mbid".into()),
        }),
        release: Some(ReleaseMeta {
            country: Some("US".into()),
            mbid: Some("release-mbid".into()),
            ..Default::default()
        }),
        tracks: vec![
            TrackMeta {
                position: 1,
                title: Some("Track One".into()),
                ..Default::default()
            },
            TrackMeta {
                position: 2,
                title: Some("Track Two".into()),
                ..Default::default()
            },
        ],
        cover_art_urls: Vec::new(),
        provider: "mb".into(),
        raw_response: None,
    }
}

fn front_cover(provider: &'static str, url: &str) -> AssetCandidate {
    AssetCandidate {
        provider: provider.into(),
        asset_type: AssetType::FrontCover,
        source_url: Url::parse(url).unwrap(),
        width: None,
        height: None,
        confidence: AssetConfidence::Exact,
    }
}

fn context_with_identifier(provider: MockIdentifier) -> PhonoContext {
    let mut ctx = PhonoContext::new();
    ctx.aggregator.register_identifier(Box::new(provider));
    ctx
}

fn open_conn() -> Connection {
    open_memory().expect("in-memory db")
}

// -----------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------

#[test]
fn identify_inserts_album_release_disc_tracks() {
    let conn = open_conn();
    let ctx = context_with_identifier(MockIdentifier {
        name: "mb",
        result: Some(mb_result()),
    });

    let out = ctx
        .identify_disc(&conn, &sample_toc(), &sample_ids(), None, false)
        .expect("identify");
    assert!(out.identified);
    assert!(!out.cached);

    let disc = crud::get_disc(&conn, out.disc_id.unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(disc.mb_discid.as_deref(), Some("disc-abc"));
    assert_eq!(disc.ar_discid1.as_deref(), Some("deadbeef"));

    let tracks = crud::list_tracks_for_disc(&conn, disc.id).unwrap();
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0].title.as_deref(), Some("Track One"));

    let album = crud::get_album(&conn, out.album_id.unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(album.title, "Real Album");
    assert_eq!(album.mbid.as_deref(), Some("album-mbid"));
}

#[test]
fn identify_cache_hit_by_mb_discid_skips_providers() {
    let conn = open_conn();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    // First identify — real call, populates DB.
    let mut ctx1 = PhonoContext::new();
    ctx1.aggregator.register_identifier(Box::new(CountingMock {
        name: "mb",
        calls: calls.clone(),
        result: Some(mb_result()),
    }));
    let first = ctx1
        .identify_disc(&conn, &sample_toc(), &sample_ids(), None, false)
        .unwrap();
    assert!(first.identified);
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Second identify — mb_discid matches cached disc, provider must not
    // be called.
    let mut ctx2 = PhonoContext::new();
    ctx2.aggregator.register_identifier(Box::new(CountingMock {
        name: "mb",
        calls: calls.clone(),
        result: Some(mb_result()),
    }));
    let second = ctx2
        .identify_disc(&conn, &sample_toc(), &sample_ids(), None, false)
        .unwrap();
    assert!(second.cached);
    assert_eq!(second.disc_id, first.disc_id);
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "second identify must not call providers"
    );
}

#[test]
fn force_refresh_bypasses_cache() {
    let conn = open_conn();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let mut ctx = PhonoContext::new();
    ctx.aggregator.register_identifier(Box::new(CountingMock {
        name: "mb",
        calls: calls.clone(),
        result: Some(mb_result()),
    }));
    ctx.identify_disc(&conn, &sample_toc(), &sample_ids(), None, false)
        .unwrap();
    ctx.identify_disc(&conn, &sample_toc(), &sample_ids(), None, true)
        .unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[test]
fn unidentified_marks_rip_file_without_creating_album() {
    let conn = open_conn();
    // Seed a rip_file row that should be marked Unidentified.
    let rip_id = crud::insert_rip_file(
        &conn,
        &phono_junk_catalog::RipFile {
            id: 0,
            disc_id: None,
            cue_path: Some(std::path::PathBuf::from("/tmp/x.cue")),
            chd_path: None,
            bin_paths: vec![],
            mtime: None,
            size: None,
            identification_confidence: phono_junk_core::IdentificationConfidence::Likely,
            identification_source: None,
            accuraterip_status: None,
            last_verified_at: None,
        },
    )
    .unwrap();

    let ctx = context_with_identifier(MockIdentifier {
        name: "mb",
        result: None,
    });
    let out = ctx
        .identify_disc(&conn, &sample_toc(), &sample_ids(), Some(rip_id), false)
        .unwrap();
    assert!(!out.identified);
    assert!(out.album_id.is_none());
    assert!(crud::list_albums(&conn).unwrap().is_empty());

    let rf = crud::get_rip_file(&conn, rip_id).unwrap().unwrap();
    assert_eq!(
        rf.identification_confidence,
        phono_junk_core::IdentificationConfidence::Unidentified
    );
}

#[test]
fn disagreement_persisted_with_entity_ids() {
    let conn = open_conn();
    let mut ctx = PhonoContext::new();
    ctx.aggregator.register_identifier(Box::new(MockIdentifier {
        name: "a",
        result: Some(ProviderResult {
            album: Some(AlbumMeta {
                title: Some("Correct".into()),
                artist_credit: Some("Artist".into()),
                ..Default::default()
            }),
            provider: "a".into(),
            ..Default::default()
        }),
    }));
    ctx.aggregator.register_identifier(Box::new(MockIdentifier {
        name: "b",
        result: Some(ProviderResult {
            album: Some(AlbumMeta {
                title: Some("Wrong".into()),
                artist_credit: Some("Artist".into()),
                ..Default::default()
            }),
            provider: "b".into(),
            ..Default::default()
        }),
    }));
    let out = ctx
        .identify_disc(&conn, &sample_toc(), &sample_ids(), None, false)
        .unwrap();
    assert!(out.any_disagreements);

    let disagreements =
        crud::list_disagreements_for(&conn, "Album", out.album_id.unwrap()).unwrap();
    assert_eq!(disagreements.len(), 1);
    assert_eq!(disagreements[0].field, "album.title");
    assert_eq!(disagreements[0].value_a, "Correct");
    assert_eq!(disagreements[0].value_b, "Wrong");
    assert!(!disagreements[0].resolved);
}

#[test]
fn pre_existing_override_wins_over_consensus_on_persist() {
    let conn = open_conn();

    // Seed the album first — identify will find it by MBID and reuse it.
    let album_id = crud::insert_album(
        &conn,
        &phono_junk_catalog::Album {
            id: 0,
            title: "Placeholder".into(),
            sort_title: None,
            artist_credit: None,
            year: None,
            mbid: Some("album-mbid".into()),
            primary_type: None,
            secondary_types: vec![],
            first_release_date: None,
        },
    )
    .unwrap();
    // Pre-authored override for this album.
    crud::insert_override(
        &conn,
        &Override {
            id: 0,
            entity_type: "Album".into(),
            entity_id: album_id,
            sub_path: None,
            field: "title".into(),
            override_value: "User-Corrected".into(),
            reason: Some("User preferred title".into()),
            created_at: None,
        },
    )
    .unwrap();

    let ctx = context_with_identifier(MockIdentifier {
        name: "mb",
        result: Some(mb_result()),
    });
    let out = ctx
        .identify_disc(&conn, &sample_toc(), &sample_ids(), None, false)
        .unwrap();
    assert_eq!(out.album_id, Some(album_id));
    let album = crud::get_album(&conn, album_id).unwrap().unwrap();
    assert_eq!(album.title, "User-Corrected");
}

#[test]
fn override_does_not_mark_disagreement_resolved() {
    let conn = open_conn();
    let mut ctx = PhonoContext::new();
    ctx.aggregator.register_identifier(Box::new(MockIdentifier {
        name: "a",
        result: Some(ProviderResult {
            album: Some(AlbumMeta {
                title: Some("MB Title".into()),
                mbid: Some("album-mbid".into()),
                ..Default::default()
            }),
            provider: "a".into(),
            ..Default::default()
        }),
    }));
    ctx.aggregator.register_identifier(Box::new(MockIdentifier {
        name: "b",
        result: Some(ProviderResult {
            album: Some(AlbumMeta {
                title: Some("Alt Title".into()),
                mbid: Some("album-mbid".into()),
                ..Default::default()
            }),
            provider: "b".into(),
            ..Default::default()
        }),
    }));

    // Seed album + override before identify runs.
    let album_id = crud::insert_album(
        &conn,
        &phono_junk_catalog::Album {
            id: 0,
            title: "Placeholder".into(),
            sort_title: None,
            artist_credit: None,
            year: None,
            mbid: Some("album-mbid".into()),
            primary_type: None,
            secondary_types: vec![],
            first_release_date: None,
        },
    )
    .unwrap();
    crud::insert_override(
        &conn,
        &Override {
            id: 0,
            entity_type: "Album".into(),
            entity_id: album_id,
            sub_path: None,
            field: "title".into(),
            override_value: "Overridden".into(),
            reason: None,
            created_at: None,
        },
    )
    .unwrap();

    let _ = ctx
        .identify_disc(&conn, &sample_toc(), &sample_ids(), None, false)
        .unwrap();
    // Disagreement is recorded (the provider conflict exists independently
    // of the override) and its `resolved` flag is not flipped.
    let ds = crud::list_disagreements_for(&conn, "Album", album_id).unwrap();
    assert_eq!(ds.len(), 1);
    assert!(!ds[0].resolved);

    // Persisted title is the override value, not the consensus winner.
    let album = crud::get_album(&conn, album_id).unwrap().unwrap();
    assert_eq!(album.title, "Overridden");
}

#[test]
fn assets_inserted_for_release_and_deduped_across_providers() {
    let conn = open_conn();
    let mut ctx = PhonoContext::new();
    ctx.aggregator.register_identifier(Box::new(MockIdentifier {
        name: "mb",
        result: Some(mb_result()),
    }));
    ctx.aggregator
        .register_asset_provider(Box::new(MockAssetProvider {
            name: "caa",
            candidates: vec![front_cover("caa", "https://example.com/cover.jpg")],
        }));
    ctx.aggregator
        .register_asset_provider(Box::new(MockAssetProvider {
            name: "itunes",
            candidates: vec![front_cover("itunes", "https://example.com/cover.jpg")],
        }));
    let out = ctx
        .identify_disc(&conn, &sample_toc(), &sample_ids(), None, false)
        .unwrap();
    assert_eq!(out.asset_count, 1, "duplicates must be folded");

    let assets = crud::list_assets_for_release(&conn, out.release_id.unwrap()).unwrap();
    assert_eq!(assets.len(), 1);
    assert_eq!(
        assets[0].source_url.as_deref(),
        Some("https://example.com/cover.jpg")
    );
}

#[test]
fn mbid_split_does_not_mix_track_lists() {
    let conn = open_conn();
    let mut ctx = PhonoContext::new();
    ctx.aggregator.register_identifier(Box::new(MockIdentifier {
        name: "mb",
        result: Some(ProviderResult {
            album: Some(AlbumMeta {
                title: Some("Correct Album".into()),
                mbid: Some("winner-mbid".into()),
                ..Default::default()
            }),
            tracks: vec![TrackMeta {
                position: 1,
                title: Some("Correct Track".into()),
                ..Default::default()
            }],
            provider: "mb".into(),
            ..Default::default()
        }),
    }));
    ctx.aggregator.register_identifier(Box::new(MockIdentifier {
        name: "other",
        result: Some(ProviderResult {
            album: Some(AlbumMeta {
                title: Some("Wrong Album".into()),
                mbid: Some("loser-mbid".into()),
                ..Default::default()
            }),
            tracks: vec![TrackMeta {
                position: 1,
                title: Some("Wrong Track".into()),
                ..Default::default()
            }],
            provider: "other".into(),
            ..Default::default()
        }),
    }));
    let out = ctx
        .identify_disc(&conn, &sample_toc(), &sample_ids(), None, false)
        .unwrap();
    let tracks = crud::list_tracks_for_disc(&conn, out.disc_id.unwrap()).unwrap();
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].title.as_deref(), Some("Correct Track"));
}
