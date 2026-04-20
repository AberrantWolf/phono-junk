use std::path::{Path, PathBuf};

use phono_junk_lib::PhonoContext;
use rusqlite::Connection;

use crate::error::CliError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    pub fn parse(s: &str) -> Result<Self, CliError> {
        match s.to_ascii_lowercase().as_str() {
            "human" | "text" | "pretty" => Ok(OutputFormat::Human),
            "json" => Ok(OutputFormat::Json),
            other => Err(CliError::InvalidArg(format!(
                "unknown --format: {other:?} (want human|json)"
            ))),
        }
    }
}

pub struct CliEnv {
    pub conn: Connection,
    pub ctx: PhonoContext,
    #[allow(dead_code)]
    pub fmt: OutputFormat,
    #[allow(dead_code)]
    pub db_path: PathBuf,
}

/// Resolve the DB path from the CLI flag, the `PHONO_JUNK_DB` env var,
/// or the shared XDG default.
pub fn resolve_db_path(flag: Option<&Path>) -> Result<PathBuf, CliError> {
    if let Some(p) = flag {
        return Ok(p.to_path_buf());
    }
    phono_junk_lib::env::default_db_path().ok_or(CliError::NoDbPath)
}

pub fn resolve_user_agent(flag: Option<&str>) -> String {
    if let Some(ua) = flag {
        return ua.to_string();
    }
    phono_junk_lib::env::default_user_agent()
}

/// Build a full environment: ensure parent dir exists, open DB, register
/// providers, load credentials from keyring + env.
///
/// Credential precedence (lowest to highest):
///
/// 1. Keyring entries under service `"phono-junk"` (loaded by
///    `PhonoContext::with_default_providers`).
/// 2. Environment variables. Currently honoured: `PHONO_DISCOGS_TOKEN`.
///    Env wins because CI and one-off runs occasionally need to override
///    a stored keyring entry.
///
/// Note the env-var caveats: process environment is readable by any
/// same-uid process (`/proc/<pid>/environ` on Linux), may land in crash
/// dumps, and enters shell history if set inline (`DISCOGS_TOKEN=… cmd`).
/// For regular use, prefer `phono-junk credentials set` which writes
/// the OS keyring.
pub fn open_env(
    db_flag: Option<&Path>,
    ua_flag: Option<&str>,
    fmt: OutputFormat,
    need_network: bool,
) -> Result<CliEnv, CliError> {
    let db_path = resolve_db_path(db_flag)?;
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let conn = phono_junk_db::open_database(&db_path)?;
    let ctx = if need_network {
        let ctx = PhonoContext::with_default_providers(resolve_user_agent(ua_flag))?;
        overlay_env_credentials(&ctx);
        ctx
    } else {
        let ctx = PhonoContext::new();
        if let Err(e) = ctx.credentials.load_from_keyring() {
            log::debug!("credentials: {e}");
        }
        overlay_env_credentials(&ctx);
        ctx
    };
    Ok(CliEnv {
        conn,
        ctx,
        fmt,
        db_path,
    })
}

/// Overlay recognised `PHONO_*_TOKEN` env vars on top of the keyring-loaded
/// credential store. Empty strings are ignored (avoids accidentally clearing
/// a keyring-stored token with `PHONO_DISCOGS_TOKEN=`).
fn overlay_env_credentials(ctx: &PhonoContext) {
    if let Ok(token) = std::env::var("PHONO_DISCOGS_TOKEN") {
        if !token.is_empty() {
            ctx.credentials.set("discogs", token);
        }
    }
}
