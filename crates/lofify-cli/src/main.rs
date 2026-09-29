//! Lofify command line tool.

mod devices;
mod live;
mod process;
mod wav;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::process::ExitCode;

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
    /// List audio devices and detect a virtual cable.
    Devices(devices::DevicesArgs),
    /// Diagnostics: run capture -> ring buffer -> output with no effects and print metrics.
    Passthrough(live::EngineArgs),
    /// Run the live effects chain with a preset.
    Live(live::LiveArgs),
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("lofify=warn".parse()?),
        )
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().command {
        Command::Process(args) => process::run(&args),
        Command::Devices(args) => devices::run(&args),
        Command::Passthrough(args) => live::passthrough(&args),
        Command::Live(args) => live::live(&args),
    }
}
