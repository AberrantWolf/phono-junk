//! Scan pipeline: walk a directory tree and ingest every rip file in two
//! phases — fast metadata (TOC + sidecar + rip_file upsert) then slow
//! identification (provider fan-out, rate-limited).
//!
//! The two phases live in separate public functions so GUI and CLI share
//! the same building blocks but compose them differently:
//!
//! - [`ingest_metadata`] — walk one file, read its TOC, collect sidecars,
//!   upsert the `rip_files` row. Sets `identification_state = Queued` when
//!   `opts.identify` is true, `Unscanned` otherwise. Always runs sidecar
//!   collection — even on cache hit — so a `.log` dropped next to a
//!   previously-identified CUE gets picked up without forcing a full
//!   re-scan.
//! - [`identify_one`] — pull a `rip_files` row by id, transition state to
//!   `Working`, run provider fan-out via [`PhonoContext::identify_disc`],
//!   transition state to `Identified` / `Unidentified` / `Failed`.
//! - [`ingest_path`] — back-compat composition (metadata then identify
//!   inline) used by CLI `identify` and any other callers that want the
//!   old single-file fast/slow path.
//!
//! [`PhonoContext::scan_library`] walks a directory tree and emits a
//! [`ScanEvent::Ingested`] per file after metadata lands (so GUI can
//! stream `LibraryChanged` per row); inline identification is run after
//! each metadata ingest when `opts.identify` is true, or skipped when
//! false (GUI calls this with `identify = false` and drains the queue
//! via its own worker afterwards).

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::Utc;
use phono_junk_catalog::{Id, RipFile};
use phono_junk_core::{IdentificationConfidence, IdentificationState};
use phono_junk_db::{DbError, cache, crud};
use phono_junk_toc::{compute_disc_ids, read_toc_from_chd, read_toc_from_cue};
use rusqlite::Connection;
use serde::Serialize;
use walkdir::WalkDir;

use crate::PhonoContext;
use crate::identify::{IdentifiedDisc, IdentifyError};
use crate::sidecar::{self, SidecarData};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ScanKind {
    Cue,
    Chd,
}

#[derive(Debug, Clone)]
pub struct ScanOpts {
    pub force_refresh: bool,
    /// When `true`, [`PhonoContext::scan_library`] and [`ingest_path`]
    /// run provider identification inline after metadata lands. When
    /// `false`, metadata is ingested and the row is left in
    /// [`IdentificationState::Queued`] for a later identify pass (the
    /// GUI's background worker drains this state; CLI's `--no-identify`
    /// flag leaves rows for a future `identify --queued` call).
    pub identify: bool,
}

impl Default for ScanOpts {
    fn default() -> Self {
        Self {
            force_refresh: false,
            identify: true,
        }
    }
}

/// One step of the scan pipeline, surfaced to the progress callback.
///
/// Borrows are `'a`-bound against the callback's invocation — no heap
/// allocations and no work on the hot path the caller doesn't ask for.
pub enum ScanEvent<'a> {
    /// Walker found a candidate file (pre cache-check).
    Found { path: &'a Path, kind: ScanKind },
    /// `(mtime, size)` matched an already-identified rip — no network
    /// call needed. Sidecar refresh still ran, so a newly-added `.log`
    /// is reflected in `rip_file_provenance`.
    CacheHit { path: &'a Path, rip_file_id: Id },
    /// Metadata phase completed — `rip_files` row upserted with state
    /// `Queued` (pending identify) or `Unscanned`. Emitted whether or
    /// not inline identify runs afterwards so GUIs can redraw the list.
    Ingested {
        path: &'a Path,
        rip_file_id: Id,
        state: IdentificationState,
    },
    /// Inline identify ran; `result` carries identified/unidentified/disagreements.
    Identified {
        path: &'a Path,
        result: &'a IdentifiedDisc,
    },
    /// `opts.identify == false` — rip_file row upserted, identify skipped.
    ScannedOnly { path: &'a Path, rip_file_id: Id },
    /// A non-fatal error. Scan continues with the next file.
    Failed {
        path: &'a Path,
        error: &'a dyn std::error::Error,
    },
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanSummary {
    pub total_files: usize,
    pub identified: usize,
    pub unidentified: usize,
    pub cached: usize,
    pub scanned_only: usize,
    pub failed: usize,
    pub disagreements_flagged: usize,
    /// Count of rows whose sidecar was refreshed on cache hit (a newly-
    /// dropped `.log` next to an already-identified CUE). Informational —
    /// affects neither `cached` nor `identified` counts.
    pub sidecars_refreshed: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("root path is not a directory: {0}")]
    NotADirectory(PathBuf),
    #[error("unsupported file extension: {0} (expected .cue or .chd)")]
    UnsupportedFile(PathBuf),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Walk(#[from] walkdir::Error),
    #[error(transparent)]
    Identify(#[from] IdentifyError),
    #[error(transparent)]
    Audio(#[from] phono_junk_core::AudioError),
}

/// Outcome of [`ingest_metadata`] — two paths depending on cache state.
#[derive(Debug)]
pub enum MetadataOutcome {
    /// Rip was already identified and the `(mtime, size)` pair matched;
    /// sidecar refresh may still have run. `sidecar_refreshed` is true
    /// when a newly-detected `.log` or CD-TEXT file wrote something to
    /// `rip_file_provenance` or the catalog.
    Cached {
        rip_file_id: Id,
        disc_id: Id,
        sidecar_refreshed: bool,
    },
    /// Metadata was (re)ingested — `rip_files` row upserted. `state`
    /// reflects what was just written (Queued / Unscanned / preserved
    /// Identified-or-Unidentified when the row already existed in a
    /// terminal state). Callers can push a `Queued` id onto an identify
    /// queue.
    Ingested {
        rip_file_id: Id,
        state: IdentificationState,
    },
}

/// Outcome of [`ingest_path`] — the composed metadata+identify path used
/// by CLI `identify` and any callers that want a single-file fast/slow
/// path without rolling the two-phase composition themselves.
#[derive(Debug)]
pub enum IngestOutcome {
    /// Rip file was already identified; nothing re-run.
    Cached { rip_file_id: Id, disc_id: Id },
    /// `opts.identify == false`: rip_file row upserted but no providers called.
    ScannedOnly { rip_file_id: Id },
    /// Providers ran (either first-ever or because `force_refresh`).
    Identified {
        rip_file_id: Id,
        disc: IdentifiedDisc,
    },
}

// ---------------------------------------------------------------------------
// Phase 1: metadata
// ---------------------------------------------------------------------------

/// Fast metadata ingest for one file. Never runs providers; returns quickly
/// so GUI can stream row-level `LibraryChanged` updates.
///
/// Always re-runs sidecar collection, including on cache hit: a `.log`
/// dropped next to a previously-identified CUE would otherwise be invisible
/// to the library (old ingest short-circuited before sidecar work).
pub fn ingest_metadata(
    ctx: &PhonoContext,
    conn: &Connection,
    path: &Path,
    opts: &ScanOpts,
) -> Result<MetadataOutcome, ScanError> {
    let _ = ctx; // reserved for future use (e.g. per-host HTTP cache warmup)
    let meta = std::fs::metadata(path)?;
    let mtime = mtime_from_meta(&meta);
    let size = meta.len();

    let kind = classify_path(path).ok_or_else(|| ScanError::UnsupportedFile(path.to_path_buf()))?;

    // Cache hit (file unchanged AND already identified): run the sidecar
    // refresh path and bail. Bug 2 fix — the old early-return skipped
    // sidecar collection entirely, which meant a log added after the
    // initial scan was never surfaced.
    if !opts.force_refresh
        && let Some(existing) = cache::lookup_cached(conn, path, mtime, size)?
        && let Some(disc_id) = existing.disc_id
    {
        let sidecar_refreshed = match kind {
            ScanKind::Cue => {
                sidecar::refresh_for_cache_hit(conn, existing.id, Some(disc_id), path)?
            }
            // CHD sidecars live inside the container — nothing to refresh
            // from siblings on disk.
            ScanKind::Chd => false,
        };
        return Ok(MetadataOutcome::Cached {
            rip_file_id: existing.id,
            disc_id,
            sidecar_refreshed,
        });
    }

    let toc = match kind {
        ScanKind::Cue => read_toc_from_cue(path)?,
        ScanKind::Chd => read_toc_from_chd(path)?,
    };
    let mut ids = compute_disc_ids(&toc);

    let sidecar_data: SidecarData = match kind {
        ScanKind::Cue => sidecar::collect_redumper_sidecars(path),
        ScanKind::Chd => SidecarData::default(),
    };
    sidecar::enrich_disc_ids(&mut ids, &sidecar_data);

    let (cue_path, chd_path) = match kind {
        ScanKind::Cue => (Some(path.to_path_buf()), None),
        ScanKind::Chd => (None, Some(path.to_path_buf())),
    };
    // Preserve an existing row's terminal state (Identified / Unidentified)
    // when the file changed but we don't want to bump it back to Queued
    // unless the user actually asked for a re-identify via force_refresh.
    let existing_state = cache::lookup_cached(conn, path, mtime, size)
        .ok()
        .flatten()
        .map(|rf| rf.identification_state);
    let new_state = if opts.identify || opts.force_refresh {
        IdentificationState::Queued
    } else {
        // Keep whatever state the row is in if it's already been identified
        // or marked unidentified; otherwise surface it as Unscanned so the
        // user can see that no identify attempt has been requested.
        existing_state.unwrap_or(IdentificationState::Unscanned)
    };
    let rip = RipFile {
        id: 0,
        disc_id: None,
        cue_path,
        chd_path,
        bin_paths: Vec::new(),
        mtime: Some(mtime),
        size: Some(size),
        identification_confidence: IdentificationConfidence::Unidentified,
        identification_source: None,
        accuraterip_status: None,
        last_verified_at: None,
        last_identify_errors: None,
        last_identify_at: None,
        provenance: sidecar_data.provenance.clone(),
        identification_state: new_state,
        last_state_change_at: Some(Utc::now().to_rfc3339()),
    };
    let rip_id = cache::upsert_rip_file(conn, &rip)?;
    Ok(MetadataOutcome::Ingested {
        rip_file_id: rip_id,
        state: new_state,
    })
}

// ---------------------------------------------------------------------------
// Phase 2: identify
// ---------------------------------------------------------------------------

/// Run provider fan-out for a single rip-file row. Transitions state
/// through Working → Identified / Unidentified / Failed, so the GUI's
/// Status column reflects the current phase in real time.
///
/// Re-reads the rip's CUE/CHD to rebuild `Toc` + `DiscIds` (the row only
/// stores the ids; the TOC lives on `Disc` once identified, or nowhere yet
/// for first-time identifies). Sidecars next to the CUE are re-collected
/// so barcode-keyed providers see the MCN on this pass.
pub fn identify_one(
    ctx: &PhonoContext,
    conn: &Connection,
    rip_file_id: Id,
    force_refresh: bool,
) -> Result<IdentifiedDisc, ScanError> {
    let rip =
        crud::get_rip_file(conn, rip_file_id)?.ok_or_else(|| ScanError::Db(DbError::Migration(
            format!("identify_one: rip_file {rip_file_id} missing"),
        )))?;

    let path = rip
        .cue_path
        .clone()
        .or_else(|| rip.chd_path.clone())
        .ok_or_else(|| {
            ScanError::Db(DbError::Migration(format!(
                "identify_one: rip_file {rip_file_id} has no path"
            )))
        })?;
    let kind = classify_path(&path).ok_or_else(|| ScanError::UnsupportedFile(path.clone()))?;

    // Working: visible to the GUI Status column for the duration of
    // provider fan-out.
    crud::set_rip_file_identification_state(
        conn,
        rip_file_id,
        IdentificationState::Working,
        &Utc::now().to_rfc3339(),
    )?;

    // Do the heavy lifting behind a closure so we can always write a
    // terminal state regardless of which branch errors.
    let res: Result<IdentifiedDisc, ScanError> = (|| {
        let toc = match kind {
            ScanKind::Cue => read_toc_from_cue(&path)?,
            ScanKind::Chd => read_toc_from_chd(&path)?,
        };
        let mut ids = compute_disc_ids(&toc);
        let sidecar_data: SidecarData = match kind {
            ScanKind::Cue => sidecar::collect_redumper_sidecars(&path),
            ScanKind::Chd => SidecarData::default(),
        };
        sidecar::enrich_disc_ids(&mut ids, &sidecar_data);
        let disc =
            ctx.identify_disc(conn, &toc, &ids, Some(rip_file_id), force_refresh)?;
        if let Some(disc_id) = disc.disc_id
            && !sidecar_data.is_empty()
        {
            sidecar::apply_sidecar_to_catalog(conn, disc_id, &sidecar_data)
                .map_err(ScanError::Db)?;
        }
        Ok(disc)
    })();

    let now = Utc::now().to_rfc3339();
    match &res {
        Ok(disc) if disc.identified => {
            crud::set_rip_file_identification_state(
                conn,
                rip_file_id,
                IdentificationState::Identified,
                &now,
            )?;
        }
        Ok(_) => {
            crud::set_rip_file_identification_state(
                conn,
                rip_file_id,
                IdentificationState::Unidentified,
                &now,
            )?;
        }
        Err(_) => {
            // Failed is distinct from Unidentified — retrying via
            // `identify_one` will flip this back to Working then succeed
            // on the next run (same DiscIds, cached Disc if the previous
            // attempt managed to persist one).
            crud::set_rip_file_identification_state(
                conn,
                rip_file_id,
                IdentificationState::Failed,
                &now,
            )?;
        }
    }
    res
}

// ---------------------------------------------------------------------------
// Composed single-file path
// ---------------------------------------------------------------------------

/// Full ingest for one file — metadata phase then inline identify when
/// `opts.identify` is true. Back-compat wrapper used by CLI `identify` and
/// any remaining callers who want the single-file fast/slow path.
pub fn ingest_path(
    ctx: &PhonoContext,
    conn: &Connection,
    path: &Path,
    opts: &ScanOpts,
) -> Result<IngestOutcome, ScanError> {
    match ingest_metadata(ctx, conn, path, opts)? {
        MetadataOutcome::Cached {
            rip_file_id,
            disc_id,
            ..
        } => Ok(IngestOutcome::Cached {
            rip_file_id,
            disc_id,
        }),
        MetadataOutcome::Ingested { rip_file_id, .. } => {
            if !opts.identify {
                return Ok(IngestOutcome::ScannedOnly { rip_file_id });
            }
            let disc = identify_one(ctx, conn, rip_file_id, opts.force_refresh)?;
            Ok(IngestOutcome::Identified {
                rip_file_id,
                disc,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn classify_path(p: &Path) -> Option<ScanKind> {
    let ext = p.extension()?.to_string_lossy().to_ascii_lowercase();
    match ext.as_str() {
        "cue" => Some(ScanKind::Cue),
        "chd" => Some(ScanKind::Chd),
        _ => None,
    }
}

fn mtime_from_meta(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Directory walker
// ---------------------------------------------------------------------------

impl PhonoContext {
    /// Walk `root`, metadata-ingest every `*.cue` / `*.chd` under it. When
    /// `opts.identify` is true, identify inline per row after its metadata
    /// lands. When false, leave every row in `Queued` state for a later
    /// identify pass (CLI `identify --queued` or the GUI's background
    /// worker).
    ///
    /// Emits `ScanEvent::Ingested` per row so GUIs can refresh the list as
    /// metadata appears rather than waiting for the whole walk to finish.
    ///
    /// Non-fatal per-file errors (bad CUE, TOC parse failure, provider HTTP
    /// flake) are emitted as [`ScanEvent::Failed`] and counted in
    /// [`ScanSummary::failed`] — the walk continues.
    pub fn scan_library<F>(
        &self,
        conn: &Connection,
        root: &Path,
        opts: ScanOpts,
        mut progress: F,
    ) -> Result<ScanSummary, ScanError>
    where
        F: FnMut(ScanEvent<'_>),
    {
        if !root.is_dir() {
            return Err(ScanError::NotADirectory(root.to_path_buf()));
        }
        let mut summary = ScanSummary::default();
        for entry_r in WalkDir::new(root).follow_links(false) {
            let entry = entry_r?;
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let Some(kind) = classify_path(path) else {
                continue;
            };
            summary.total_files += 1;
            progress(ScanEvent::Found { path, kind });

            let metadata_result = ingest_metadata(self, conn, path, &opts);
            let (rip_file_id, maybe_identify) = match metadata_result {
                Ok(MetadataOutcome::Cached {
                    rip_file_id,
                    sidecar_refreshed,
                    ..
                }) => {
                    summary.cached += 1;
                    if sidecar_refreshed {
                        summary.sidecars_refreshed += 1;
                    }
                    progress(ScanEvent::CacheHit { path, rip_file_id });
                    continue;
                }
                Ok(MetadataOutcome::Ingested {
                    rip_file_id,
                    state,
                }) => {
                    progress(ScanEvent::Ingested {
                        path,
                        rip_file_id,
                        state,
                    });
                    (rip_file_id, state == IdentificationState::Queued && opts.identify)
                }
                Err(e) => {
                    summary.failed += 1;
                    progress(ScanEvent::Failed { path, error: &e });
                    continue;
                }
            };

            if !maybe_identify {
                summary.scanned_only += 1;
                progress(ScanEvent::ScannedOnly { path, rip_file_id });
                continue;
            }

            match identify_one(self, conn, rip_file_id, opts.force_refresh) {
                Ok(disc) => {
                    if disc.identified {
                        summary.identified += 1;
                    } else {
                        summary.unidentified += 1;
                    }
                    if disc.any_disagreements {
                        summary.disagreements_flagged += 1;
                    }
                    progress(ScanEvent::Identified {
                        path,
                        result: &disc,
                    });
                }
                Err(e) => {
                    summary.failed += 1;
                    progress(ScanEvent::Failed { path, error: &e });
                }
            }
        }
        Ok(summary)
    }
}
