use serde::Serialize;

use crate::env::OutputFormat;
use crate::error::CliError;

/// Emit `value` as JSON or as the caller's human-formatted string.
/// One call site per subcommand; zero subcommand-specific serde plumbing.
pub fn emit<T: Serialize>(
    fmt: OutputFormat,
    value: &T,
    human: impl FnOnce(&T) -> String,
) -> Result<(), CliError> {
    match fmt {
        OutputFormat::Human => {
            println!("{}", human(value));
        }
        OutputFormat::Json => {
            let s = serde_json::to_string_pretty(value)?;
            println!("{s}");
        }
    }
    Ok(())
}
