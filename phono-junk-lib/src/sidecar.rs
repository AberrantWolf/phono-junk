//! Redumper-sidecar ingestion for the scan pipeline.
//!
//! Called during [`crate::scan::ingest_path`] when a CUE is encountered.
//! Looks for redumper's sibling `.log` / `.cdtext` files, parses them,
//! and surfaces the extracted data in two halves:
//!
//! 1. **Pre-identify** enrichment: mirror MCN into `DiscIds.barcode` (so
//!    barcode-keyed providers like Discogs can use it this pass) and
//!    stamp [`RipFile.provenance`](phono_junk_catalog::RipFile::provenance)
//!    before the rip-file row is written.
//! 2. **Post-identify** catalog stamping: once the disc + tracks exist,
//!    write MCN onto `Disc.mcn`, per-track ISRCs onto `Track.isrc`, and
//!    flag any UPC↔MCN mismatch via the disagreement machinery.
//!
//! Absence of sidecars is never an error — redumper-ripped libraries are
//! the best case, but CUE-only dumps are valid input and flow through
//! with no provenance stamped.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use junk_libs_disc::redumper::{self, CdText, RedumperLog, Ripper, Sidecars};
use phono_junk_catalog::{
    Disagreement, Id, RipperProvenance,
};
use phono_junk_core::{DiscIds, IdentificationSource};
use phono_junk_db::{DbError, crud};
use rusqlite::Connection;

/// Collected facts from redumper sidecars next to a CUE.
///
/// Every field is optional — a sidecar directory may carry a log without
/// an MCN, or CD-TEXT without UPC/EAN, and the parser returns what it
/// finds without gap-filling.
#[derive(Debug, Default, Clone)]
pub struct SidecarData {
    pub mcn: Option<String>,
    pub isrcs: BTreeMap<u8, String>,
    pub cdtext_upc: Option<String>,
    /// Per-track titles from CD-TEXT, 1-based track number → title.
    pub cdtext_titles: BTreeMap<u8, String>,
    /// Per-track performer/artist from CD-TEXT.
    pub cdtext_performers: BTreeMap<u8, String>,
    /// Ripper provenance. `Some(Ripper::Unknown, …)` when a log existed
    /// but couldn't be parsed; `None` when no log was found at all.
    pub provenance: Option<RipperProvenance>,
}

impl SidecarData {
    pub fn is_empty(&self) -> bool {
        self.mcn.is_none()
            && self.isrcs.is_empty()
            && self.cdtext_upc.is_none()
            && self.cdtext_titles.is_empty()
            && self.cdtext_performers.is_empty()
            && self.provenance.is_none()
    }
}

/// Look for redumper sidecars next to `cue_path` and collect their data.
///
/// Parse failures are non-fatal and logged: a corrupt log produces a
/// `Ripper::Unknown` provenance stamp (distinct from "no log at all"),
/// and a malformed CD-TEXT simply omits the CD-TEXT fields.
pub fn collect_redumper_sidecars(cue_path: &Path) -> SidecarData {
    let sidecars: Sidecars = redumper::find_sidecars(cue_path);
    let mut data = SidecarData::default();

    if let Some(log_path) = &sidecars.log {
        match redumper::parse_log(log_path) {
            Ok(log) => {
                data.mcn = log.mcn.clone();
                data.isrcs = log.isrcs.clone();
                data.provenance = Some(log_to_provenance(&log, log_path.clone()));
            }
            Err(e) => {
                log::warn!(
                    "redumper log at {} failed to parse ({e}); stamping Unknown provenance",
                    log_path.display()
                );
                data.provenance = Some(RipperProvenance {
                    ripper: Ripper::Unknown,
                    version: None,
                    drive: None,
                    read_offset: None,
                    log_path: log_path.clone(),
                    rip_date: None,
                });
            }
        }
    }

    if let Some(cdtext_path) = &sidecars.cdtext {
        match redumper::parse_cdtext(cdtext_path) {
            Ok(ct) => absorb_cdtext(&mut data, &ct),
            Err(e) => log::warn!(
                "CD-TEXT at {} failed to parse ({e}); ignoring",
                cdtext_path.display()
            ),
        }
    }

    data
}

fn log_to_provenance(log: &RedumperLog, log_path: std::path::PathBuf) -> RipperProvenance {
    RipperProvenance {
        ripper: Ripper::Redumper,
        version: log.version.clone(),
        drive: log.drive.clone(),
        read_offset: log.read_offset,
        log_path,
        rip_date: log.rip_date.as_deref().and_then(parse_rip_date),
    }
}

/// Parse redumper's rip-date string (`YYYY-MM-DD HH:MM:SS`, UTC assumed).
fn parse_rip_date(s: &str) -> Option<DateTime<Utc>> {
    // Try RFC3339 first (accepts the `T` separator); fall back to
    // naive `YYYY-MM-DD HH:MM:SS` treated as UTC.
    if let Ok(d) = DateTime::parse_from_rfc3339(s) {
        return Some(d.with_timezone(&Utc));
    }
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|dt| dt.and_utc())
}

fn absorb_cdtext(data: &mut SidecarData, ct: &CdText) {
    let Some(block) = ct.primary() else {
        return;
    };
    data.cdtext_upc = block.upc_ean.clone();
    for t in &block.tracks {
        if let Some(title) = &t.title {
            data.cdtext_titles.insert(t.track_number, title.clone());
        }
        if let Some(perf) = &t.performer {
            data.cdtext_performers.insert(t.track_number, perf.clone());
        }
        // CD-TEXT per-track ISRCs back-fill the log's map when the log
        // didn't carry them (older redumper logs omit ISRCs entirely).
        if let Some(isrc) = &t.isrc
            && !data.isrcs.contains_key(&t.track_number)
        {
            data.isrcs.insert(t.track_number, isrc.clone());
        }
    }
}

/// Mirror redumper-sourced facts into `DiscIds` so barcode-keyed providers
/// (Discogs et al.) can use them on this identify pass. Does not overwrite
/// values already present — a provider-supplied barcode from a previous
/// cache entry wins.
pub fn enrich_disc_ids(ids: &mut DiscIds, data: &SidecarData) {
    if ids.barcode.is_none() {
        // Prefer the log's MCN; fall back to CD-TEXT's UPC/EAN.
        ids.barcode = data
            .mcn
            .clone()
            .or_else(|| data.cdtext_upc.clone());
    }
}

/// Write redumper-sourced facts onto the catalog after identify_disc has
/// created the `Disc` and `Track` rows.
///
/// Rules:
/// - `Disc.mcn` is written if the log carried one AND the row doesn't
///   already have one (respect earlier sources).
/// - `Track.isrc` is written per-position from either the log or CD-TEXT;
///   existing non-empty ISRCs are preserved (higher-trust sources win).
/// - When both the log's MCN and CD-TEXT's UPC/EAN are present and they
///   disagree, a `Disagreement` row is written against
///   `entity_type = "disc"`, `field = "mcn"`, sources tagged `Redumper`
///   (log) and `Redumper` (cdtext). The tag matches today's enum but
///   future sprints may split it.
pub fn apply_sidecar_to_catalog(
    conn: &Connection,
    disc_id: Id,
    data: &SidecarData,
) -> Result<(), DbError> {
    // --- Disc.mcn ---
    if let Some(mcn) = &data.mcn
        && let Some(mut disc) = crud::get_disc(conn, disc_id)?
        && disc.mcn.is_none()
    {
        disc.mcn = Some(mcn.clone());
        crud::update_disc(conn, &disc)?;
    }

    // --- Track.isrc (per position) ---
    if !data.isrcs.is_empty() {
        let tracks = crud::list_tracks_for_disc(conn, disc_id)?;
        for mut track in tracks {
            let Some(code) = data.isrcs.get(&track.position) else {
                continue;
            };
            if track.isrc.as_deref().is_some_and(|s| !s.is_empty()) {
                continue;
            }
            track.isrc = Some(code.clone());
            crud::update_track(conn, &track)?;
        }
    }

    // --- UPC vs MCN cross-check (Disagreement) ---
    if let (Some(log_mcn), Some(upc)) = (&data.mcn, &data.cdtext_upc)
        && log_mcn != upc
    {
        crud::insert_disagreement(
            conn,
            &Disagreement {
                id: 0,
                entity_type: "disc".into(),
                entity_id: disc_id,
                field: "mcn".into(),
                source_a: source_tag(&IdentificationSource::Redumper, "log"),
                value_a: log_mcn.clone(),
                source_b: source_tag(&IdentificationSource::Redumper, "cdtext"),
                value_b: upc.clone(),
                resolved: false,
                created_at: None,
            },
        )?;
    }

    Ok(())
}

/// Cache-hit sidecar refresh — re-collect sibling `.log` / `.cdtext` and
/// push any newly-detected facts to the already-identified rip.
///
/// Called from [`crate::scan::ingest_metadata`] when the rip's
/// `(mtime, size)` pair already matches the cached row. Without this hook
/// a user who drops a redumper log next to an identified CUE would never
/// see their library's "missing redumper provenance" audit shrink — the
/// old scan short-circuited before ever looking for sidecars on cache hit.
///
/// Policy is conservative: provenance is written only when the existing
/// row has none (or a strictly less-informative one); `apply_sidecar_to_
/// catalog` already preserves higher-trust values on `Disc.mcn` / per-
/// track `Track.isrc`. Returns true when anything actually changed, so
/// scan stats can surface "N sidecars refreshed."
pub fn refresh_for_cache_hit(
    conn: &Connection,
    rip_file_id: Id,
    disc_id: Option<Id>,
    cue_path: &std::path::Path,
) -> Result<bool, DbError> {
    let data = collect_redumper_sidecars(cue_path);
    if data.is_empty() {
        return Ok(false);
    }
    let mut changed = false;

    // Provenance: only write when the existing row has none. A cached
    // identified rip that already carries a redumper stamp shouldn't be
    // clobbered by a re-read of the same log (we already trust it).
    if let Some(new_prov) = &data.provenance {
        let existing = crud::load_rip_file_provenance(conn, rip_file_id)?;
        if existing.is_none() {
            crud::upsert_rip_file_provenance(conn, rip_file_id, new_prov)?;
            changed = true;
        }
    }

    if let Some(disc_id) = disc_id {
        // `apply_sidecar_to_catalog` is idempotent: writes only when the
        // target field is empty. We can't easily detect "nothing changed"
        // without re-reading before/after, so we conservatively report
        // changed=true when any catalog-facing sidecar fact exists.
        let has_catalog_facts = data.mcn.is_some()
            || !data.isrcs.is_empty()
            || data.cdtext_upc.is_some();
        if has_catalog_facts {
            apply_sidecar_to_catalog(conn, disc_id, &data)?;
            changed = true;
        }
    }

    Ok(changed)
}

fn source_tag(s: &IdentificationSource, subtag: &str) -> String {
    // `Redumper/log` / `Redumper/cdtext` — a convention, not a parser
    // contract. The disagreement consumer renders these verbatim.
    match s {
        IdentificationSource::Redumper => format!("Redumper/{subtag}"),
        other => format!("{other:?}/{subtag}"),
    }
}
