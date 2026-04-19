//! Scan pipeline: walk a directory tree, cache-check each file, dispatch
//! to [`PhonoContext::identify_disc`].
//!
//! Invariant: a rip file's `(mtime, size)` pair is the short-circuit key.
//! Identical pair + existing `disc_id` → skip everything. Identical pair
//! but `disc_id = NULL` (previously unidentified) → re-run identify so
//! new provider state can pick it up. Mismatched pair or missing row →
//! full ingest.
//!
//! [`ingest_path`] is exposed directly for CLI `identify` reuse; the
//! same entry point drives [`PhonoContext::scan_library`]'s inner loop so
//! the two stay bit-identical.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use phono_junk_catalog::{Id, RipFile};
use phono_junk_core::IdentificationConfidence;
use phono_junk_db::{DbError, cache};
use phono_junk_toc::{compute_disc_ids, read_toc_from_chd, read_toc_from_cue};
use rusqlite::Connection;
use serde::Serialize;
use walkdir::WalkDir;

use crate::PhonoContext;
use crate::identify::{IdentifiedDisc, IdentifyError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ScanKind {
    Cue,
    Chd,
}

#[derive(Debug, Clone)]
pub struct ScanOpts {
    pub force_refresh: bool,
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
    /// `(mtime, size)` matched an already-identified rip. Skipped.
    CacheHit { path: &'a Path, rip_file_id: Id },
    /// Re-ran identify; `result` carries identified/unidentified/disagreements.
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

/// Outcome of [`ingest_path`]. Three shapes for three fast/slow paths.
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

/// Full ingest for one file — cache-check, TOC read, rip_file upsert, optional identify.
///
/// Reused by [`PhonoContext::scan_library`]'s inner loop and CLI `identify`.
pub fn ingest_path(
    ctx: &PhonoContext,
    conn: &Connection,
    path: &Path,
    opts: &ScanOpts,
) -> Result<IngestOutcome, ScanError> {
    let meta = std::fs::metadata(path)?;
    let mtime = mtime_from_meta(&meta);
    let size = meta.len();

    if !opts.force_refresh
        && let Some(existing) = cache::lookup_cached(conn, path, mtime, size)?
        && let Some(disc_id) = existing.disc_id
    {
        return Ok(IngestOutcome::Cached {
            rip_file_id: existing.id,
            disc_id,
        });
    }

    let kind = classify_path(path).ok_or_else(|| ScanError::UnsupportedFile(path.to_path_buf()))?;
    let toc = match kind {
        ScanKind::Cue => read_toc_from_cue(path)?,
        ScanKind::Chd => read_toc_from_chd(path)?,
    };
    let ids = compute_disc_ids(&toc);

    let (cue_path, chd_path) = match kind {
        ScanKind::Cue => (Some(path.to_path_buf()), None),
        ScanKind::Chd => (None, Some(path.to_path_buf())),
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
    };
    let rip_id = cache::upsert_rip_file(conn, &rip)?;

    if !opts.identify {
        return Ok(IngestOutcome::ScannedOnly { rip_file_id: rip_id });
    }

    let disc = ctx.identify_disc(conn, &toc, &ids, Some(rip_id), opts.force_refresh)?;
    Ok(IngestOutcome::Identified {
        rip_file_id: rip_id,
        disc,
    })
}

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

impl PhonoContext {
    /// Walk `root`, identify every `*.cue` / `*.chd` under it.
    ///
    /// Non-fatal per-file errors (bad CUE, TOC parse failure, provider HTTP
    /// flake) are emitted as [`ScanEvent::Failed`] and counted in
    /// [`ScanSummary::failed`] — the walk continues. Only walker-level or DB
    /// errors abort the whole scan.
    ///
    /// Shape is `fn(&Connection, …)` on purpose so Sprint 15's GUI
    /// background-op dispatcher can run it on a worker thread with the
    /// worker's own connection.
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

            match ingest_path(self, conn, path, &opts) {
                Ok(IngestOutcome::Cached { rip_file_id, .. }) => {
                    summary.cached += 1;
                    progress(ScanEvent::CacheHit { path, rip_file_id });
                }
                Ok(IngestOutcome::ScannedOnly { rip_file_id }) => {
                    summary.scanned_only += 1;
                    progress(ScanEvent::ScannedOnly { path, rip_file_id });
                }
                Ok(IngestOutcome::Identified { disc, .. }) => {
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
