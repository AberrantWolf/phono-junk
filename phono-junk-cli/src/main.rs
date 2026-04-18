use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "phono-junk",
    about = "Audio CD rip identification and library management"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan a directory tree for CD rips and identify them.
    Scan { root: PathBuf },
    /// Identify a single disc from its cue or chd.
    Identify { path: PathBuf },
    /// Verify a disc against AccurateRip.
    Verify { path: PathBuf },
    /// Export selected discs as FLAC to a target library.
    Export {
        #[arg(long)]
        disc_ids: Vec<i64>,
        #[arg(long)]
        out: PathBuf,
    },
    /// List cataloged discs.
    List,
}

fn main() {
    env_logger::init();
    let cli = Cli::parse();
    match cli.command {
        Command::Scan { root } => {
            println!("scan {root:?} — not yet implemented");
        }
        Command::Identify { path } => {
            println!("identify {path:?} — not yet implemented");
        }
        Command::Verify { path } => {
            println!("verify {path:?} — not yet implemented");
        }
        Command::Export { disc_ids, out } => {
            println!("export {disc_ids:?} -> {out:?} — not yet implemented");
        }
        Command::List => {
            println!("list — not yet implemented");
        }
    }
}
