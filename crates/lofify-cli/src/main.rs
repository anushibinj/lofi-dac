//! Lofify command line tool.

mod process;
mod wav;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "lofify", version, about = "Turn everything you play into lofi")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Render presets offline on generated signals and WAV files (for listening tests).
    Process(process::ProcessArgs),
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Process(args) => process::run(&args),
    }
}
