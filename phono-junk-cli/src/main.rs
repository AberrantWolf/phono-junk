mod env;
mod error;
mod output;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use phono_junk_catalog::Id;
use phono_junk_db::crud;
use phono_junk_lib::{
    ExportedDisc, IngestOutcome, ListFilters, ListRow, PhonoContext, ScanEvent, ScanOpts,
    ScanSummary, UnidentifiedRow, VerifySummary, VerifyTarget, YearSpec, filter_rows,
    ingest_path, load_list_rows,
};

use crate::env::{CliEnv, OutputFormat, open_env};
use crate::error::CliError;
use crate::output::emit;

#[derive(Parser)]
#[command(
    name = "phono-junk",
    about = "Audio CD rip identification and library management",
    version
)]
struct Cli {
    /// Path to the library database. Defaults to
    /// `$PHONO_JUNK_DB` then XDG `data_dir()/phono-junk/library.db`.
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    /// HTTP User-Agent used for every provider call. MusicBrainz requires
    /// a descriptive UA — the default identifies phono-junk and links to
    /// the repo.
    #[arg(long, global = true)]
    user_agent: Option<String>,

    /// Output shape: `human` (default) or `json`.
    #[arg(long, global = true, default_value = "human")]
    format: String,

    /// `-v` enables INFO logs, `-vv` DEBUG. Otherwise WARN and up.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan a directory tree for CD rips and identify them.
    Scan {
        root: PathBuf,
        #[arg(long)]
        force_refresh: bool,
        /// Walk + cache rip files without running identification.
        #[arg(long)]
        no_identify: bool,
    },
    /// Identify a single disc from its cue or chd, or drain the queue.
    ///
    /// Default: run identify against one cue/chd path (the legacy mode).
    /// With `--queued`, walk every `rip_files` row in `Queued` or `Failed`
    /// state and run identify on each serially. Used after a `scan
    /// --no-identify` pass, or to retry rows that hit transient provider
    /// errors.
    Identify {
        /// Path to the CUE/CHD to identify. Omit when using `--queued`.
        path: Option<PathBuf>,
        #[arg(long)]
        force_refresh: bool,
        /// Drain every row in Queued or Failed state instead of identifying
        /// one specific file. Mutually exclusive with `path`.
        #[arg(long, conflicts_with = "path")]
        queued: bool,
    },
    /// Verify a disc against AccurateRip.
    Verify {
        /// Path to a `.cue` or `.chd` already scanned. Mutually exclusive with `--disc-id`.
        path: Option<PathBuf>,
        /// Catalog disc id. Mutually exclusive with `path`.
        #[arg(long, conflicts_with = "path")]
        disc_id: Option<Id>,
    },
    /// Export selected discs as FLAC to a target library.
    Export {
        #[arg(long, required = true, value_delimiter = ',')]
        disc_ids: Vec<Id>,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
    /// Audit the library by ripper provenance.
    ///
    /// Default: print a count per ripper variant. With `--missing-redumper`,
    /// list every rip that isn't sourced from redumper (including rips with
    /// no sidecar at all) — useful when hunting re-rip candidates.
    Audit {
        /// Show only rips lacking confirmed redumper provenance.
        #[arg(long)]
        missing_redumper: bool,
    },
    /// Manage provider credentials stored in the OS keyring.
    ///
    /// Reads and writes happen against `service=phono-junk, user=<provider>`.
    /// `PHONO_DISCOGS_TOKEN` env var, if set, overrides the keyring value
    /// at runtime but is NOT written to the keyring. Env vars are readable
    /// by any same-uid process, may land in crash dumps, and appear in
    /// shell history if set inline — for daily use prefer `credentials set`.
    Credentials {
        #[command(subcommand)]
        action: CredentialsAction,
    },
    /// List cataloged albums.
    List {
        /// Substring match (case-insensitive) on album artist credit.
        #[arg(long)]
        artist: Option<String>,
        /// `1996` or a range `1990-1999`.
        #[arg(long)]
        year: Option<String>,
        /// Exact match on release country (ISO2).
        #[arg(long)]
        country: Option<String>,
        /// Substring match on release label.
        #[arg(long)]
        label: Option<String>,
        /// Show unidentified rip files instead of albums.
        #[arg(long)]
        unidentified: bool,
    },
}

#[derive(Subcommand)]
enum CredentialsAction {
    /// Store a token for a provider. Prompts for the token on stdin
    /// with no-echo — never pass tokens as CLI args (shell history leak).
    Set {
        /// Provider name (e.g. `discogs`).
        provider: String,
    },
    /// Remove a provider's token from the keyring and in-memory store.
    Clear {
        provider: String,
    },
    /// List provider names that currently have a token stored. Values
    /// are never printed.
    List,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logger(cli.verbose);
    match dispatch(&cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn init_logger(verbose: u8) {
    let level = match verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let mut builder = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level));
    builder.format_timestamp(None).init();
}

fn dispatch(cli: &Cli) -> Result<ExitCode, CliError> {
    let fmt = OutputFormat::parse(&cli.format)?;
    match &cli.command {
        Command::Scan {
            root,
            force_refresh,
            no_identify,
        } => run_scan(cli, fmt, root, *force_refresh, *no_identify),
        Command::Identify {
            path,
            force_refresh,
            queued,
        } => run_identify(cli, fmt, path.as_deref(), *force_refresh, *queued),
        Command::Verify { path, disc_id } => run_verify(cli, fmt, path.as_deref(), *disc_id),
        Command::Export {
            disc_ids,
            out,
            dry_run,
        } => run_export(cli, fmt, disc_ids, out, *dry_run),
        Command::Audit { missing_redumper } => run_audit(cli, fmt, *missing_redumper),
        Command::Credentials { action } => run_credentials(fmt, action),
        Command::List {
            artist,
            year,
            country,
            label,
            unidentified,
        } => run_list(
            cli,
            fmt,
            artist.as_deref(),
            year.as_deref(),
            country.as_deref(),
            label.as_deref(),
            *unidentified,
        ),
    }
}

// ---------------------------------------------------------------------------
// scan
// ---------------------------------------------------------------------------

fn run_scan(
    cli: &Cli,
    fmt: OutputFormat,
    root: &Path,
    force_refresh: bool,
    no_identify: bool,
) -> Result<ExitCode, CliError> {
    if !root.is_dir() {
        return Err(CliError::MissingPath(root.to_path_buf()));
    }
    let CliEnv { conn, ctx, .. } = open_env(cli.db.as_deref(), cli.user_agent.as_deref(), fmt, true)?;
    let opts = ScanOpts {
        force_refresh,
        identify: !no_identify,
    };
    let summary = ctx.scan_library(&conn, root, opts, |ev| print_scan_event(&ev))?;

    emit(fmt, &summary, format_scan_summary)?;
    Ok(ExitCode::SUCCESS)
}

fn print_scan_event(ev: &ScanEvent<'_>) {
    match ev {
        ScanEvent::Found { path, kind } => {
            log::info!("found {:?} at {}", kind, path.display());
        }
        ScanEvent::CacheHit { path, rip_file_id } => {
            log::info!("cache hit (rip_file={rip_file_id}): {}", path.display());
        }
        ScanEvent::Ingested {
            path,
            rip_file_id,
            state,
        } => {
            log::info!(
                "metadata ingested (rip_file={rip_file_id}, state={}): {}",
                state.as_str(),
                path.display(),
            );
        }
        ScanEvent::Identified { path, result } => {
            if result.identified {
                log::info!(
                    "identified {} (album={:?}, cached={}, assets={}, disagreements={})",
                    path.display(),
                    result.album_id,
                    result.cached,
                    result.asset_count,
                    result.any_disagreements,
                );
            } else {
                log::info!("unidentified {}", path.display());
            }
            for (name, msg) in &result.provider_errors {
                log::warn!("provider {name} error on {}: {msg}", path.display());
            }
        }
        ScanEvent::ScannedOnly { path, rip_file_id } => {
            log::info!("scanned (rip_file={rip_file_id}): {}", path.display());
        }
        ScanEvent::Failed { path, error } => {
            log::warn!("failed {}: {error}", path.display());
        }
    }
}

fn format_scan_summary(s: &ScanSummary) -> String {
    format!(
        "Scanned {total} files: {id} identified, {un} unidentified, {cached} cached, {only} scanned-only, {failed} failed, {dis} with disagreements.",
        total = s.total_files,
        id = s.identified,
        un = s.unidentified,
        cached = s.cached,
        only = s.scanned_only,
        failed = s.failed,
        dis = s.disagreements_flagged,
    )
}

// ---------------------------------------------------------------------------
// identify
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct IdentifyOutput {
    rip_file_id: Id,
    disc_id: Option<Id>,
    album_id: Option<Id>,
    release_id: Option<Id>,
    identified: bool,
    cached: bool,
    any_disagreements: bool,
    asset_count: usize,
    title: Option<String>,
    artist: Option<String>,
    year: Option<u16>,
    provider_errors: Vec<(String, String)>,
}

fn run_identify(
    cli: &Cli,
    fmt: OutputFormat,
    path: Option<&Path>,
    force_refresh: bool,
    queued: bool,
) -> Result<ExitCode, CliError> {
    let CliEnv { conn, ctx, .. } = open_env(cli.db.as_deref(), cli.user_agent.as_deref(), fmt, true)?;

    if queued {
        return run_identify_queued(&conn, &ctx, fmt, force_refresh);
    }

    let path = path.ok_or_else(|| {
        CliError::InvalidArg("`identify` requires <path> or --queued".into())
    })?;
    if !path.exists() {
        return Err(CliError::MissingPath(path.to_path_buf()));
    }
    let opts = ScanOpts {
        force_refresh,
        identify: true,
    };
    let outcome = ingest_path(&ctx, &conn, path, &opts)?;
    let output = identify_output_from_outcome(&conn, outcome)?;
    for (name, msg) in &output.provider_errors {
        log::warn!("provider {name}: {msg}");
    }
    emit(fmt, &output, format_identify_output)?;
    Ok(ExitCode::SUCCESS)
}

#[derive(serde::Serialize)]
struct QueueDrainSummary {
    processed: usize,
    identified: usize,
    unidentified: usize,
    failed: usize,
}

fn run_identify_queued(
    conn: &rusqlite::Connection,
    ctx: &PhonoContext,
    fmt: OutputFormat,
    force_refresh: bool,
) -> Result<ExitCode, CliError> {
    use phono_junk_core::IdentificationState;
    let queue = crud::list_rip_files_by_state(
        conn,
        &[IdentificationState::Queued, IdentificationState::Failed],
    )?;
    let total = queue.len();
    let mut identified = 0usize;
    let mut unidentified = 0usize;
    let mut failed = 0usize;
    for rip in queue {
        match phono_junk_lib::scan::identify_one(ctx, conn, rip.id, force_refresh) {
            Ok(disc) if disc.identified => identified += 1,
            Ok(_) => unidentified += 1,
            Err(e) => {
                failed += 1;
                log::warn!("identify queue rip_file={}: {e}", rip.id);
            }
        }
    }
    let summary = QueueDrainSummary {
        processed: total,
        identified,
        unidentified,
        failed,
    };
    emit(fmt, &summary, |s| {
        format!(
            "Queue drained: {} processed ({} identified, {} unidentified, {} failed).",
            s.processed, s.identified, s.unidentified, s.failed
        )
    })?;
    Ok(ExitCode::SUCCESS)
}

fn identify_output_from_outcome(
    conn: &rusqlite::Connection,
    outcome: IngestOutcome,
) -> Result<IdentifyOutput, CliError> {
    match outcome {
        IngestOutcome::Cached {
            rip_file_id,
            disc_id,
        } => {
            let (title, artist, year, album_id, release_id) = album_summary_for_disc(conn, disc_id)?;
            Ok(IdentifyOutput {
                rip_file_id,
                disc_id: Some(disc_id),
                album_id,
                release_id,
                identified: true,
                cached: true,
                any_disagreements: false,
                asset_count: 0,
                title,
                artist,
                year,
                provider_errors: Vec::new(),
            })
        }
        IngestOutcome::ScannedOnly { rip_file_id } => Ok(IdentifyOutput {
            rip_file_id,
            disc_id: None,
            album_id: None,
            release_id: None,
            identified: false,
            cached: false,
            any_disagreements: false,
            asset_count: 0,
            title: None,
            artist: None,
            year: None,
            provider_errors: Vec::new(),
        }),
        IngestOutcome::Identified {
            rip_file_id,
            disc,
        } => {
            let (title, artist, year) = if let Some(album_id) = disc.album_id {
                album_summary(conn, album_id)?
            } else {
                (None, None, None)
            };
            Ok(IdentifyOutput {
                rip_file_id,
                disc_id: disc.disc_id,
                album_id: disc.album_id,
                release_id: disc.release_id,
                identified: disc.identified,
                cached: disc.cached,
                any_disagreements: disc.any_disagreements,
                asset_count: disc.asset_count,
                title,
                artist,
                year,
                provider_errors: disc.provider_errors,
            })
        }
    }
}

fn album_summary_for_disc(
    conn: &rusqlite::Connection,
    disc_id: Id,
) -> Result<(Option<String>, Option<String>, Option<u16>, Option<Id>, Option<Id>), CliError> {
    let disc = crud::get_disc(conn, disc_id)?;
    let Some(disc) = disc else {
        return Ok((None, None, None, None, None));
    };
    let release = crud::get_release(conn, disc.release_id)?;
    let Some(release) = release else {
        return Ok((None, None, None, None, Some(disc.release_id)));
    };
    let album = crud::get_album(conn, release.album_id)?;
    match album {
        Some(a) => Ok((
            Some(a.title),
            a.artist_credit,
            a.year,
            Some(a.id),
            Some(release.id),
        )),
        None => Ok((None, None, None, Some(release.album_id), Some(release.id))),
    }
}

fn album_summary(
    conn: &rusqlite::Connection,
    album_id: Id,
) -> Result<(Option<String>, Option<String>, Option<u16>), CliError> {
    let album = crud::get_album(conn, album_id)?;
    Ok(match album {
        Some(a) => (Some(a.title), a.artist_credit, a.year),
        None => (None, None, None),
    })
}

fn format_identify_output(o: &IdentifyOutput) -> String {
    let title = o.title.as_deref().unwrap_or("<unknown>");
    let artist = o.artist.as_deref().unwrap_or("<unknown>");
    let year = o
        .year
        .map(|y| y.to_string())
        .unwrap_or_else(|| "----".into());
    let disc = o
        .disc_id
        .map(|d| d.to_string())
        .unwrap_or_else(|| "—".into());
    let state = if o.identified {
        if o.cached { "identified (cached)" } else { "identified" }
    } else {
        "unidentified"
    };
    format!(
        "disc {disc}: {title:?} by {artist} ({year}) [{state}, assets={a}, disagreements={d}]",
        a = o.asset_count,
        d = o.any_disagreements,
    )
}

// ---------------------------------------------------------------------------
// verify
// ---------------------------------------------------------------------------

fn run_verify(
    cli: &Cli,
    fmt: OutputFormat,
    path: Option<&Path>,
    disc_id: Option<Id>,
) -> Result<ExitCode, CliError> {
    let target = match (path, disc_id) {
        (Some(p), None) => {
            if !p.exists() {
                return Err(CliError::MissingPath(p.to_path_buf()));
            }
            VerifyTarget::Path(p.to_path_buf())
        }
        (None, Some(id)) => VerifyTarget::DiscId(id),
        (None, None) => {
            return Err(CliError::InvalidArg(
                "`verify` requires either <path> or --disc-id".into(),
            ));
        }
        (Some(_), Some(_)) => unreachable!("clap conflicts_with"),
    };
    let CliEnv { conn, ctx, .. } = open_env(cli.db.as_deref(), cli.user_agent.as_deref(), fmt, true)?;
    let summary = ctx.verify_disc(&conn, target)?;
    emit(fmt, &summary, format_verify_summary)?;
    Ok(ExitCode::SUCCESS)
}

fn format_verify_summary(s: &VerifySummary) -> String {
    if s.not_in_db {
        return format!(
            "Disc {}: not in AccurateRip database ({} audio tracks).",
            s.disc_id,
            s.per_track.len()
        );
    }
    let mut lines = vec![format!(
        "Disc {}: {}/{} accurate (max confidence {}), {} mismatched.",
        s.disc_id,
        s.accurate,
        s.per_track.len(),
        s.max_confidence,
        s.mismatched,
    )];
    for t in &s.per_track {
        if !t.verified {
            lines.push(format!(
                "  Track {:02}: MISMATCH ({}) v1={:08x} v2={:08x}",
                t.position, t.status, t.v1, t.v2
            ));
        }
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// export
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct ExportOutput {
    discs: Vec<ExportedDisc>,
    dry_run: bool,
}

fn run_export(
    cli: &Cli,
    fmt: OutputFormat,
    disc_ids: &[Id],
    out: &Path,
    dry_run: bool,
) -> Result<ExitCode, CliError> {
    let CliEnv { conn, ctx, .. } = open_env(cli.db.as_deref(), cli.user_agent.as_deref(), fmt, true)?;
    if !out.exists() {
        std::fs::create_dir_all(out)?;
    }

    if dry_run {
        let output = plan_export_dryrun(&conn, &ctx, disc_ids, out)?;
        emit(fmt, &output, format_export_output)?;
        return Ok(ExitCode::SUCCESS);
    }

    let mut discs = Vec::with_capacity(disc_ids.len());
    for id in disc_ids {
        let ed = ctx.export_disc(&conn, *id, out)?;
        discs.push(ed);
    }
    let output = ExportOutput { discs, dry_run };
    emit(fmt, &output, format_export_output)?;
    Ok(ExitCode::SUCCESS)
}

fn plan_export_dryrun(
    conn: &rusqlite::Connection,
    _ctx: &PhonoContext,
    disc_ids: &[Id],
    out: &Path,
) -> Result<ExportOutput, CliError> {
    use phono_junk_extract::{plan_disc_directory, plan_output_paths};
    let mut discs = Vec::with_capacity(disc_ids.len());
    for id in disc_ids {
        let disc = crud::get_disc(conn, *id)?
            .ok_or_else(|| CliError::InvalidArg(format!("disc {id} not found")))?;
        let release = crud::get_release(conn, disc.release_id)?
            .ok_or_else(|| CliError::InvalidArg(format!("release {} missing", disc.release_id)))?;
        let album = crud::get_album(conn, release.album_id)?
            .ok_or_else(|| CliError::InvalidArg(format!("album {} missing", release.album_id)))?;
        let tracks = crud::list_tracks_for_disc(conn, *id)?;
        let sibling_discs = crud::list_discs_for_release(conn, release.id)?;
        let total_discs = sibling_discs.len().max(1) as u8;
        let album_artist = album
            .artist_credit
            .clone()
            .unwrap_or_else(|| "Unknown Artist".into());
        let planned = plan_output_paths(
            out,
            &album,
            disc.disc_number,
            total_discs,
            &tracks,
            Some(&album_artist),
        );
        let disc_dir = plan_disc_directory(
            out,
            &album,
            disc.disc_number,
            total_discs,
            Some(&album_artist),
        );
        discs.push(ExportedDisc {
            disc_id: *id,
            written: [&[disc_dir][..], &planned].concat(),
            cover_written: false,
        });
    }
    Ok(ExportOutput {
        discs,
        dry_run: true,
    })
}

fn format_export_output(o: &ExportOutput) -> String {
    let mut lines = Vec::new();
    let verb = if o.dry_run { "would write" } else { "wrote" };
    for ed in &o.discs {
        lines.push(format!("disc {}: {} files", ed.disc_id, ed.written.len()));
        for p in &ed.written {
            lines.push(format!("  {verb} {}", p.display()));
        }
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// audit
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct AuditMissingRow {
    rip_file_id: i64,
    disc_id: Option<i64>,
    path: Option<String>,
    ripper: &'static str,
    log_path: Option<String>,
}

#[derive(serde::Serialize)]
struct AuditSummaryOutput {
    total: usize,
    redumper: usize,
    non_redumper: usize,
    by_ripper: Vec<(String, usize)>,
}

#[derive(serde::Serialize)]
#[serde(untagged)]
enum AuditOutput {
    MissingRedumper(Vec<AuditMissingRow>),
    Summary(AuditSummaryOutput),
}

fn run_audit(cli: &Cli, fmt: OutputFormat, missing_redumper: bool) -> Result<ExitCode, CliError> {
    use phono_junk_lib::audit;

    let CliEnv { conn, .. } = open_env(cli.db.as_deref(), cli.user_agent.as_deref(), fmt, false)?;
    let output = if missing_redumper {
        let rows = audit::list_missing_redumper(&conn)?
            .into_iter()
            .map(|r| AuditMissingRow {
                rip_file_id: r.rip_file_id,
                disc_id: r.disc_id,
                path: r
                    .cue_path
                    .or(r.chd_path)
                    .map(|p| p.display().to_string()),
                ripper: audit::ripper_label(r.ripper),
                log_path: r.log_path.map(|p| p.display().to_string()),
            })
            .collect();
        AuditOutput::MissingRedumper(rows)
    } else {
        let s = audit::summarize(&conn)?;
        AuditOutput::Summary(AuditSummaryOutput {
            total: s.total,
            redumper: s.redumper_count(),
            non_redumper: s.non_redumper_count(),
            by_ripper: s
                .by_ripper
                .iter()
                .map(|(r, n)| (audit::ripper_label(*r).to_string(), *n))
                .collect(),
        })
    };
    emit(fmt, &output, format_audit_output)?;
    Ok(ExitCode::SUCCESS)
}

fn format_audit_output(o: &AuditOutput) -> String {
    match o {
        AuditOutput::MissingRedumper(rows) => {
            if rows.is_empty() {
                return "(no rips lacking redumper provenance)".into();
            }
            let mut lines = vec![format!(
                "{:>4} {:<40} {:<20} path",
                "id", "ripper", "log"
            )];
            for r in rows {
                lines.push(format!(
                    "{:>4} {:<40.40} {:<20.20} {}",
                    r.rip_file_id,
                    r.ripper,
                    r.log_path.as_deref().unwrap_or("(none)"),
                    r.path.as_deref().unwrap_or("(no path)"),
                ));
            }
            lines.join("\n")
        }
        AuditOutput::Summary(s) => {
            let mut lines = vec![format!(
                "{} rips total: {} redumper, {} non-redumper",
                s.total, s.redumper, s.non_redumper
            )];
            for (label, n) in &s.by_ripper {
                lines.push(format!("  {n:>4}  {label}"));
            }
            lines.join("\n")
        }
    }
}

// ---------------------------------------------------------------------------
// credentials
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
struct CredentialsListOutput {
    providers: Vec<String>,
}

fn run_credentials(fmt: OutputFormat, action: &CredentialsAction) -> Result<ExitCode, CliError> {
    use phono_junk_lib::credentials::CredentialStore;

    // The credentials subcommand doesn't need a DB or provider set, just
    // a credential store loaded from the keyring.
    let store = CredentialStore::new();
    if let Err(e) = store.load_from_keyring() {
        log::warn!("credentials: {e}");
    }

    match action {
        CredentialsAction::Set { provider } => {
            let token = rpassword::prompt_password(format!("Enter token for {provider}: "))
                .map_err(|e| CliError::InvalidArg(format!("read token: {e}")))?;
            let token = token.trim();
            if token.is_empty() {
                return Err(CliError::InvalidArg("empty token".into()));
            }
            store
                .store_to_keyring(provider, token)
                .map_err(|e| CliError::InvalidArg(format!("keyring: {e}")))?;
            // Logs stay quiet on success — token length isn't sensitive but
            // silence is the better default for an interactive credential
            // write. The user pressed Enter; that's their confirmation.
            emit(fmt, &serde_json::json!({ "provider": provider, "stored": true }), |_| {
                format!("stored token for {provider}")
            })?;
        }
        CredentialsAction::Clear { provider } => {
            store
                .clear_from_keyring(provider)
                .map_err(|e| CliError::InvalidArg(format!("keyring: {e}")))?;
            emit(fmt, &serde_json::json!({ "provider": provider, "cleared": true }), |_| {
                format!("cleared token for {provider}")
            })?;
        }
        CredentialsAction::List => {
            let providers = store.provider_names();
            let output = CredentialsListOutput { providers };
            emit(fmt, &output, |o| {
                if o.providers.is_empty() {
                    "(no providers configured)".into()
                } else {
                    let mut lines = vec!["Configured providers:".to_string()];
                    for p in &o.providers {
                        lines.push(format!("  {p}: set"));
                    }
                    lines.join("\n")
                }
            })?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
enum ListOutput {
    Albums(Vec<ListRow>),
    Unidentified(Vec<UnidentifiedRow>),
}

fn run_list(
    cli: &Cli,
    fmt: OutputFormat,
    artist: Option<&str>,
    year: Option<&str>,
    country: Option<&str>,
    label: Option<&str>,
    unidentified: bool,
) -> Result<ExitCode, CliError> {
    let CliEnv { conn, .. } = open_env(cli.db.as_deref(), cli.user_agent.as_deref(), fmt, false)?;

    let output = if unidentified {
        let rows = crud::list_unidentified_rip_files(&conn)?
            .into_iter()
            .map(|r| UnidentifiedRow {
                rip_file_id: r.id,
                cue_path: r.cue_path,
                chd_path: r.chd_path,
                ripper: r.provenance.as_ref().map(|p| p.ripper),
                state: r.identification_state,
            })
            .collect();
        ListOutput::Unidentified(rows)
    } else {
        let year_spec = match year {
            None => None,
            Some(s) => Some(YearSpec::parse(s).map_err(CliError::InvalidArg)?),
        };
        let filters = ListFilters {
            artist: artist.map(String::from),
            year: year_spec,
            country: country.map(String::from),
            label: label.map(String::from),
            ..Default::default()
        };
        let rows = load_list_rows(&conn)?;
        ListOutput::Albums(filter_rows(rows, &filters))
    };

    emit(fmt, &output, format_list_output)?;
    Ok(ExitCode::SUCCESS)
}

fn format_list_output(o: &ListOutput) -> String {
    match o {
        ListOutput::Albums(rows) => {
            if rows.is_empty() {
                return "(no albums)".into();
            }
            let mut lines = vec![format!(
                "{:>4} {:<30} {:<25} {:<6} {:<5} {:<20} discs",
                "id", "title", "artist", "year", "cntry", "label"
            )];
            for r in rows {
                lines.push(format!(
                    "{:>4} {:<30.30} {:<25.25} {:<6} {:<5} {:<20.20} {}",
                    r.album_id,
                    r.title,
                    r.artist.as_deref().unwrap_or(""),
                    r.year.map(|y| y.to_string()).unwrap_or_else(|| "".into()),
                    r.country.as_deref().unwrap_or(""),
                    r.label.as_deref().unwrap_or(""),
                    r.disc_count,
                ));
            }
            lines.join("\n")
        }
        ListOutput::Unidentified(rows) => {
            if rows.is_empty() {
                return "(no unidentified rips)".into();
            }
            let mut lines = vec![format!("{:>4} path", "id")];
            for r in rows {
                let path = r
                    .display_path()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "(no path)".into());
                lines.push(format!("{:>4} {path}", r.rip_file_id));
            }
            lines.join("\n")
        }
    }
}
