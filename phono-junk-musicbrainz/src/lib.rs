//! MusicBrainz identification + Cover Art Archive asset provider.
//!
//! Implements both [`IdentificationProvider`] (keyed on `mb_discid`) and
//! [`AssetProvider`] (front / back / booklet / tray / medium / obi-strip
//! images served by the Cover Art Archive).
//!
//! Both are unauthenticated. MusicBrainz enforces 1 req/sec and returns 403
//! without a descriptive User-Agent; Cover Art Archive is served off a
//! different host (`coverartarchive.org`) that gets its own 1 req/sec bucket.
//!
//! Parsing is split from network I/O: [`parse_discid_response`] and
//! [`parse_caa_response`] operate on raw bytes so unit tests can exercise
//! them with fixtures, and so Sprint 11's aggregator can re-parse cached
//! raw responses without redoing HTTP.

use governor::Quota;
use nonzero_ext::nonzero;
use phono_junk_core::{DiscIds, Toc};
use phono_junk_identify::{
    AlbumMeta, AssetCandidate, AssetConfidence, AssetLookupCtx, AssetProvider, AssetType,
    Credentials, DiscIdKind, HttpClient, HttpError, IdentificationProvider, ProviderError,
    ProviderResult, ReleaseMeta, TrackMeta,
};
use url::Url;

mod artist_credit;
mod json;

const PROVIDER_MB: &str = "musicbrainz";
const PROVIDER_CAA: &str = "cover-art-archive";

// -------------------- MusicBrainz --------------------

pub struct MusicBrainzProvider {
    http: HttpClient,
}

impl MusicBrainzProvider {
    pub fn new(user_agent: impl Into<String>) -> Result<Self, HttpError> {
        let http = HttpClient::builder()
            .user_agent(user_agent)
            .host_quota("musicbrainz.org", Quota::per_second(nonzero!(1u32)))
            .build()?;
        Ok(Self { http })
    }

    /// Inject a preconfigured client. Used by tests to point the provider at
    /// an httpmock server.
    #[doc(hidden)]
    pub fn with_http_client(http: HttpClient) -> Self {
        Self { http }
    }
}

impl IdentificationProvider for MusicBrainzProvider {
    fn name(&self) -> &'static str {
        PROVIDER_MB
    }

    fn supported_ids(&self) -> &[DiscIdKind] {
        &[DiscIdKind::MbDiscId]
    }

    fn lookup(
        &self,
        _toc: &Toc,
        ids: &DiscIds,
        _creds: &Credentials,
    ) -> Result<Option<ProviderResult>, ProviderError> {
        let Some(discid) = ids.mb_discid.as_ref() else {
            return Ok(None);
        };
        let url = format!(
            "https://musicbrainz.org/ws/2/discid/{discid}?inc=artists+recordings+release-groups&fmt=json"
        );
        let resp = self
            .http
            .get(&url)
            .map_err(map_http_err)?;
        match resp.status {
            200 => parse_discid_response(&resp.body),
            404 => Ok(None),
            code => Err(ProviderError::Other(format!(
                "musicbrainz returned HTTP {code}"
            ))),
        }
    }
}

/// Parse a MusicBrainz `/ws/2/discid/<id>` JSON body into a [`ProviderResult`].
///
/// Returns `Ok(None)` when the response has no releases. Otherwise picks the
/// first release and its first medium — multi-release disambiguation and
/// multi-disc medium selection are TODO-deferred. Logs a warning in both
/// cases so the drop isn't silent.
pub fn parse_discid_response(bytes: &[u8]) -> Result<Option<ProviderResult>, ProviderError> {
    let resp: json::DiscidResponse = serde_json::from_slice(bytes)
        .map_err(|e| ProviderError::Parse(format!("musicbrainz discid: {e}")))?;

    if resp.releases.is_empty() {
        return Ok(None);
    }
    if resp.releases.len() > 1 {
        log::warn!(
            "musicbrainz returned {} releases for DiscID; picking first ({})",
            resp.releases.len(),
            resp.releases[0].id,
        );
    }

    let raw_response = serde_json::from_slice::<serde_json::Value>(bytes).ok();

    let release = resp.releases.into_iter().next().expect("len>0 checked");
    let artist_str = artist_credit::format(&release.artist_credit);
    let year = release.date.as_deref().and_then(parse_year);

    let album = Some(AlbumMeta {
        title: Some(release.title.clone()),
        artist_credit: (!artist_str.is_empty()).then(|| artist_str.clone()),
        year,
        mbid: release.release_group.as_ref().map(|rg| rg.id.clone()),
    });

    let release_meta = Some(ReleaseMeta {
        country: release.country,
        date: release.date,
        label: release
            .label_info
            .iter()
            .find_map(|li| li.label.as_ref().map(|l| l.name.clone())),
        catalog_number: release
            .label_info
            .iter()
            .find_map(|li| li.catalog_number.clone()),
        barcode: release.barcode,
        mbid: Some(release.id.clone()),
    });

    let tracks = pick_medium(&release.media)
        .map(|m| {
            m.tracks
                .iter()
                .map(|t| TrackMeta {
                    position: t.position,
                    title: Some(t.title.clone()),
                    artist_credit: None,
                    length_frames: t.length.map(ms_to_frames),
                    isrc: None,
                    mbid: t.recording.as_ref().map(|r| r.id.clone()),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Some(ProviderResult {
        album,
        release: release_meta,
        tracks,
        cover_art_urls: Vec::new(),
        provider: PROVIDER_MB.to_string(),
        raw_response,
    }))
}

fn pick_medium(media: &[json::Medium]) -> Option<&json::Medium> {
    match media.len() {
        0 => None,
        1 => media.first(),
        n => {
            log::warn!(
                "musicbrainz release has {n} media; picking first (multi-disc handling deferred)",
            );
            media.first()
        }
    }
}

fn parse_year(date: &str) -> Option<u16> {
    date.split('-').next()?.parse().ok()
}

/// Milliseconds (MB) to CD frames (sectors, 1/75s). Floor division — the
/// at-most 1-frame error is tolerable for identification-time alignment.
fn ms_to_frames(ms: u64) -> u64 {
    ms * 75 / 1000
}

// -------------------- Cover Art Archive --------------------

pub struct CoverArtArchiveProvider {
    http: HttpClient,
}

impl CoverArtArchiveProvider {
    pub fn new(user_agent: impl Into<String>) -> Result<Self, HttpError> {
        let http = HttpClient::builder()
            .user_agent(user_agent)
            .host_quota("coverartarchive.org", Quota::per_second(nonzero!(1u32)))
            .build()?;
        Ok(Self { http })
    }

    #[doc(hidden)]
    pub fn with_http_client(http: HttpClient) -> Self {
        Self { http }
    }
}

impl AssetProvider for CoverArtArchiveProvider {
    fn name(&self) -> &'static str {
        PROVIDER_CAA
    }

    fn asset_types(&self) -> &[AssetType] {
        &[
            AssetType::FrontCover,
            AssetType::BackCover,
            AssetType::Booklet,
            AssetType::CdLabel,
            AssetType::TrayInsert,
            AssetType::ObiStrip,
        ]
    }

    fn lookup_art(
        &self,
        ctx: &AssetLookupCtx<'_>,
    ) -> Result<Vec<AssetCandidate>, ProviderError> {
        let Some(mbid) = ctx.release.mbid.as_ref() else {
            return Ok(Vec::new());
        };
        let url = format!("https://coverartarchive.org/release/{mbid}");
        let resp = self
            .http
            .get(&url)
            .map_err(map_http_err)?;
        match resp.status {
            200 => parse_caa_response(&resp.body),
            404 => Ok(Vec::new()),
            code => Err(ProviderError::Other(format!(
                "cover-art-archive returned HTTP {code}"
            ))),
        }
    }
}

/// Parse a Cover Art Archive `/release/<mbid>` JSON body into asset candidates.
pub fn parse_caa_response(bytes: &[u8]) -> Result<Vec<AssetCandidate>, ProviderError> {
    let resp: json::CaaResponse = serde_json::from_slice(bytes)
        .map_err(|e| ProviderError::Parse(format!("cover-art-archive: {e}")))?;
    let mut out = Vec::with_capacity(resp.images.len());
    for img in resp.images {
        let source_url = match Url::parse(&img.image) {
            Ok(u) => u,
            Err(e) => {
                log::warn!(
                    "cover-art-archive: skipping image with invalid URL {}: {e}",
                    img.image
                );
                continue;
            }
        };
        out.push(AssetCandidate {
            provider: PROVIDER_CAA.to_string(),
            asset_type: classify(&img),
            source_url,
            width: None,
            height: None,
            confidence: AssetConfidence::Exact,
        });
    }
    Ok(out)
}

/// Map a CAA image's flags + `types` array onto [`AssetType`]. `types`-based
/// categories (Booklet/Tray/Medium/Obi) take precedence over the back/front
/// flags — an image tagged "Booklet" stays a booklet even if `front=true`.
fn classify(img: &json::CaaImage) -> AssetType {
    let has = |needle: &str| img.types.iter().any(|t| t.eq_ignore_ascii_case(needle));
    if has("booklet") {
        AssetType::Booklet
    } else if has("tray") {
        AssetType::TrayInsert
    } else if has("medium") {
        AssetType::CdLabel
    } else if has("obi") {
        AssetType::ObiStrip
    } else if img.back {
        AssetType::BackCover
    } else if img.front {
        AssetType::FrontCover
    } else {
        AssetType::Other
    }
}

// -------------------- error mapping --------------------

fn map_http_err(e: HttpError) -> ProviderError {
    match e {
        HttpError::ServerRateLimited => ProviderError::RateLimited,
        HttpError::MissingUserAgent => ProviderError::Other(e.to_string()),
        other => ProviderError::Network(other.to_string()),
    }
}
