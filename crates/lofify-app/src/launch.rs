//! Process start-up: options, logging, single instance, settings, engine and window.

use crate::app::{LofifyApp, Services};
use crate::commands;
use crate::hotkeys::HotkeyRegistration;
use crate::settings::{AppPaths, Settings};
use crate::tray::Tray;
use crate::{icon, logging, single};
use eframe::egui;
use lofify_dsp::shared::SharedParams;
use lofify_engine::engine::{Engine, EngineConfig};
use std::process::ExitCode;
use std::sync::Arc;

/// Command line options.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Start hidden in the tray (used by "launch at login").
    pub minimized: bool,
    pub help: bool,
    pub version: bool,
}

/// Parses the arguments after the program name. Unknown arguments are an error message.
pub fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut o = Options::default();
    for a in args {
        match a.as_str() {
            "--minimized" => o.minimized = true,
            "-h" | "--help" => o.help = true,
            "-V" | "--version" => o.version = true,
            other => return Err(format!("unknown argument {other:?} (try --help)")),
        }
    }
    Ok(o)
}

const USAGE: &str = "Lofify: turn everything you play into lofi\n\nUSAGE:\n    lofify-app [--minimized]\n\nOPTIONS:\n    --minimized    start hidden in the system tray\n    -h, --help     show this help\n    -V, --version  show the version\n";

/// Runs the application and returns the process exit code.
pub fn run(args: &[String]) -> ExitCode {
    let options = match parse_args(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    if options.help {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if options.version {
        println!("lofify-app {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    match start(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn start(options: &Options) -> Result<(), String> {
    let paths = AppPaths::discover();
    let _log_guard = logging::init(paths.as_ref().map(|p| p.log_dir.as_path()));
    tracing::info!("Lofify {} starting", env!("CARGO_PKG_VERSION"));

    let Some(_instance) = single::acquire() else {
        // Not an error: the first instance keeps running.
        eprintln!("Lofify is already running (look for its tray icon).");
        return Ok(());
    };

    let (settings, warning) = match &paths {
        Some(p) => {
            let loaded = Settings::load(&p.settings_file);
            (loaded.settings, loaded.warning)
        }
        None => (
            Settings::default(),
            Some("no config folder found; settings will not be saved".to_string()),
        ),
    };

    let params = Arc::new(SharedParams::new(&settings.params));
    let config = if settings.audio.low_latency {
        EngineConfig::low_latency()
    } else {
        EngineConfig::default()
    };
    let engine = Engine::spawn(lofify_engine::platform::default_backend, config, params);

    let hidden = options.minimized || settings.general.start_minimized;
    let (tx, rx) = commands::channel();
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lofify")
            .with_inner_size([1040.0, 740.0])
            .with_min_inner_size([860.0, 560.0])
            .with_visible(!hidden)
            .with_icon(egui::IconData {
                rgba: icon::rgba(64, true),
                width: 64,
                height: 64,
            }),
        ..eframe::NativeOptions::default()
    };

    let minimized = options.minimized;
    eframe::run_native(
        "Lofify",
        native,
        Box::new(move |cc| {
            // The tray icon and hotkey must be created on the main thread once the event loop runs.
            let services = Services {
                tray: Tray::create(&cc.egui_ctx, tx.clone()),
                hotkeys: HotkeyRegistration::new(&cc.egui_ctx, tx.clone()),
            };
            let mut app = LofifyApp::new(
                engine,
                settings,
                paths,
                services,
                rx,
                Box::new(lofify_engine::platform::list_devices),
                warning,
            );
            if app.starts_hidden(minimized) {
                app.hide();
            } else if hidden {
                // Asked to start hidden but there is no tray to bring the window back: show it.
                cc.egui_ctx
                    .send_viewport_cmd(egui::ViewportCommand::Visible(true));
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| format!("the window could not be created: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn parses_known_options() {
        assert_eq!(parse_args(&[]).unwrap(), Options::default());
        assert!(parse_args(&args(&["--minimized"])).unwrap().minimized);
        assert!(parse_args(&args(&["-h"])).unwrap().help);
        assert!(parse_args(&args(&["--version"])).unwrap().version);
    }

    #[test]
    fn rejects_unknown_options() {
        let e = parse_args(&args(&["--frobnicate"])).unwrap_err();
        assert!(e.contains("--frobnicate") && e.contains("--help"));
    }
}
