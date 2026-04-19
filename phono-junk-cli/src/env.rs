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
/// or the XDG default (creating the parent dir if needed).
pub fn resolve_db_path(flag: Option<&Path>) -> Result<PathBuf, CliError> {
    if let Some(p) = flag {
        return Ok(p.to_path_buf());
    }
    if let Ok(env) = std::env::var("PHONO_JUNK_DB") {
        return Ok(PathBuf::from(env));
    }
    let base = dirs::data_dir().ok_or(CliError::NoDbPath)?;
    Ok(base.join("phono-junk").join("library.db"))
}

pub fn resolve_user_agent(flag: Option<&str>) -> String {
    if let Some(ua) = flag {
        return ua.to_string();
    }
    if let Ok(ua) = std::env::var("PHONO_JUNK_USER_AGENT") {
        return ua;
    }
    concat!(
        "phono-junk/",
        env!("CARGO_PKG_VERSION"),
        " ( https://github.com/AberrantWolf/phono-junk )"
    )
    .to_string()
}

/// Build a full environment: ensure parent dir exists, open DB, register providers.
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
        PhonoContext::with_default_providers(resolve_user_agent(ua_flag))?
    } else {
        PhonoContext::new()
    };
    Ok(CliEnv {
        conn,
        ctx,
        fmt,
        db_path,
    })
}
