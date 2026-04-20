//! Library-level audits over `RipperProvenance`.
//!
//! The scan pipeline stamps each rip with a ripper-specific provenance
//! record (when a sidecar like redumper's `.log` exists) or `None` (when
//! nothing was detected). These audits surface the library-wide picture:
//! "how many rips are redumper-sourced?", "which ones aren't?".
//!
//! Shared by CLI (`phono-junk library audit`) and GUI (library-view
//! ripper badge / filter). No presentation logic here — just rows.

use std::collections::BTreeMap;

use junk_libs_disc::redumper::Ripper;
use phono_junk_db::{DbError, crud};
use rusqlite::Connection;

/// A single row in the "rips missing redumper" audit.
///
/// Populated from `RipFile` + its joined `RipperProvenance`. Holds only
/// what the audit cares about — the CLI renders these as one line each.
#[derive(Debug, Clone)]
pub struct AuditRow {
    pub rip_file_id: i64,
    /// CUE path when the rip is CUE-based; `None` for CHD rips (those
    /// intrinsically can't carry redumper sidecars today).
    pub cue_path: Option<std::path::PathBuf>,
    pub chd_path: Option<std::path::PathBuf>,
    pub disc_id: Option<i64>,
    /// The detected ripper, or `None` when no sidecar existed at all.
    pub ripper: Option<Ripper>,
    /// The sidecar log path (when any). Useful so a user can cat/grep it.
    pub log_path: Option<std::path::PathBuf>,
}

/// Summary of ripper-variant counts across the whole library.
#[derive(Debug, Clone, Default)]
pub struct AuditSummary {
    pub total: usize,
    /// Count per ripper. `Ripper::Unknown` captures "log was there but
    /// format unrecognised"; `None` captures "no sidecar at all."
    pub by_ripper: BTreeMap<Option<Ripper>, usize>,
}

impl AuditSummary {
    /// Count of rips sourced by redumper — the happy-path number.
    pub fn redumper_count(&self) -> usize {
        self.by_ripper.get(&Some(Ripper::Redumper)).copied().unwrap_or(0)
    }

    /// Rips without confirmed redumper provenance — the audit target.
    pub fn non_redumper_count(&self) -> usize {
        self.total.saturating_sub(self.redumper_count())
    }
}

/// List every rip that isn't sourced from redumper, sorted by rip_file id.
///
/// "Not redumper" includes both `Ripper::Unknown` / other rippers *and*
/// `provenance == None` (rips that pre-date sidecar ingestion or were
/// ripped with a tool that didn't leave a sibling log).
pub fn list_missing_redumper(conn: &Connection) -> Result<Vec<AuditRow>, DbError> {
    let rips = crud::list_all_rip_files(conn)?;
    let mut out = Vec::new();
    for r in rips {
        let ripper = r.provenance.as_ref().map(|p| p.ripper);
        if ripper == Some(Ripper::Redumper) {
            continue;
        }
        out.push(AuditRow {
            rip_file_id: r.id,
            cue_path: r.cue_path,
            chd_path: r.chd_path,
            disc_id: r.disc_id,
            ripper,
            log_path: r.provenance.as_ref().map(|p| p.log_path.clone()),
        });
    }
    Ok(out)
}

/// Summarise ripper-variant counts across the whole library.
pub fn summarize(conn: &Connection) -> Result<AuditSummary, DbError> {
    let rips = crud::list_all_rip_files(conn)?;
    let mut s = AuditSummary {
        total: rips.len(),
        ..Default::default()
    };
    for r in &rips {
        let key = r.provenance.as_ref().map(|p| p.ripper);
        *s.by_ripper.entry(key).or_insert(0) += 1;
    }
    Ok(s)
}

/// Render the human-readable label for a `Ripper` (or `None`) slot —
/// single spot so CLI and future GUI agree on phrasing.
pub fn ripper_label(r: Option<Ripper>) -> &'static str {
    match r {
        Some(Ripper::Redumper) => "redumper",
        Some(Ripper::Eac) => "EAC",
        Some(Ripper::Cueripper) => "CUERipper",
        Some(Ripper::DbPoweramp) => "dBpoweramp",
        Some(Ripper::Xld) => "XLD",
        Some(Ripper::Unknown) => "unknown (log present, unrecognised)",
        None => "no sidecar",
    }
}
