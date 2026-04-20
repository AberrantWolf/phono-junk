//! Default DB path + User-Agent resolution, shared by CLI and GUI.
//!
//! Single source of truth so both frontends land on the same library
//! file by default — users who only ever run one of them still see the
//! other's catalog without thinking about paths.

use std::path::PathBuf;

/// Default catalog-database path. Returns `<data_dir>/phono-junk/library.db`
/// where `data_dir` is the XDG / platform data directory (see the `dirs`
/// crate). Honours the `PHONO_JUNK_DB` environment variable as an
/// override. Returns `None` only when the platform has no resolvable
/// data directory *and* no env override — exceedingly rare.
pub fn default_db_path() -> Option<PathBuf> {
    if let Ok(env) = std::env::var("PHONO_JUNK_DB") {
        return Some(PathBuf::from(env));
    }
    let base = dirs::data_dir()?;
    Some(base.join("phono-junk").join("library.db"))
}

/// Default User-Agent for network providers. Honours the
/// `PHONO_JUNK_USER_AGENT` environment variable as an override. MB
/// requires a descriptive UA with contact info; the baked-in default
/// follows that convention.
pub fn default_user_agent() -> String {
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
