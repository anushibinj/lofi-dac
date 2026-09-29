//! `lofify passthrough` and `lofify live`: run the real engine and print diagnostics.
//!
//! `passthrough` runs the Clean preset (no effects) to verify capture, ring buffer and output on
//! their own. `live` runs a preset and accepts commands on stdin: `b` toggles bypass,
//! `preset <name>` switches preset, `q` quits.

use anyhow::{Context, Result, bail};
use clap::Args;
use lofify_dsp::params::Params;
use lofify_dsp::presets::{Preset, builtin_preset, builtin_presets};
use lofify_dsp::shared::SharedParams;
use lofify_engine::engine::{Engine, EngineConfig, EngineState, Status};
use lofify_engine::sim::SimWorld;
use lofify_engine::types::{CaptureSelection, DeviceConfig, OutputSelection};
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

#[derive(Args, Debug, Clone)]
pub struct EngineArgs {
    /// Capture source: "auto" (detect the virtual cable), a recording device name, "loopback"
    /// (default playback device) or "loopback:<playback device name>".
    #[arg(long, default_value = "auto")]
    pub capture: String,

    /// Output device name, or "default". Must be your real speakers, never the cable.
    #[arg(long, default_value = "default")]
    pub output: String,

    /// Stop after this many seconds (wall clock). 0 runs until Ctrl+C.
    #[arg(long, default_value_t = 0)]
    pub seconds: u64,

    /// Use a simulated Windows machine instead of real hardware (works on any OS).
    #[arg(long)]
    pub simulate: bool,

    /// Simulation speed multiplier (e.g. 10 runs ten minutes of audio in one minute).
    #[arg(long, default_value_t = 1.0)]
    pub sim_speed: f64,

    /// Simulated capture clock error in parts per million.
    #[arg(long, default_value_t = 0.0)]
    pub sim_ppm: f64,

    /// Lower latency: 10 ms prefill and a 3 ms tape delay (needs stable devices).
    #[arg(long)]
    pub low_latency: bool,

    /// Override the prefill in milliseconds.
    #[arg(long)]
    pub prefill_ms: Option<f32>,

    /// Disable clock drift compensation.
    #[arg(long)]
    pub no_drift_compensation: bool,

    /// Print a status line this often, in seconds.
    #[arg(long, default_value_t = 1)]
    pub report_every: u64,
}

#[derive(Args, Debug)]
pub struct LiveArgs {
    #[command(flatten)]
    pub engine: EngineArgs,

    /// Preset name (see `lofify process --help` for the list).
    #[arg(short, long, default_value = "Classic Lofi")]
    pub preset: String,

    /// Load a preset from a TOML file instead.
    #[arg(long)]
    pub preset_file: Option<std::path::PathBuf>,

    /// Start bypassed.
    #[arg(long)]
    pub bypass: bool,
}

pub fn parse_capture(s: &str) -> CaptureSelection {
    let t = s.trim();
    if t.eq_ignore_ascii_case("auto") {
        CaptureSelection::Auto
    } else if t.eq_ignore_ascii_case("loopback") {
        CaptureSelection::Loopback(String::new())
    } else if let Some(name) = t.strip_prefix("loopback:") {
        CaptureSelection::Loopback(name.trim().to_string())
    } else {
        CaptureSelection::Input(t.to_string())
    }
}

pub fn parse_output(s: &str) -> OutputSelection {
    let t = s.trim();
    if t.eq_ignore_ascii_case("default") || t.is_empty() {
        OutputSelection::Default
    } else {
        OutputSelection::Named(t.to_string())
    }
}

fn engine_config(args: &EngineArgs) -> EngineConfig {
    let mut cfg = if args.low_latency {
        EngineConfig::low_latency()
    } else {
        EngineConfig::default()
    };
    if let Some(p) = args.prefill_ms {
        cfg.prefill_ms = p;
    }
    cfg.drift_compensation = !args.no_drift_compensation;
    cfg
}

fn db(peak: f32) -> f32 {
    20.0 * peak.max(1.0e-6).log10()
}

fn bar(peak: f32) -> String {
    let filled = (((db(peak) + 60.0) / 60.0).clamp(0.0, 1.0) * 10.0).round() as usize;
    format!("{}{}", "#".repeat(filled), "-".repeat(10 - filled))
}

/// One diagnostics line.
pub fn status_line(elapsed: Duration, speed: f64, s: &Status) -> String {
    let m = &s.metrics;
    let t = elapsed.as_secs_f64() * speed;
    let state = match &s.state {
        EngineState::Stopped => "stopped".to_string(),
        EngineState::Starting => "starting".to_string(),
        EngineState::Running if m.waiting_for_audio => "idle".to_string(),
        EngineState::Running => "running".to_string(),
        EngineState::Waiting(why) => format!("waiting: {why}"),
        EngineState::Error(why) => format!("ERROR: {why}"),
    };
    format!(
        "[{:02}:{:02}] {state}  lat {:5.1} ms  ring {:5.1} ms  in [{}] out [{}]  underruns {}  overruns {}  restarts {}  audio {}",
        (t / 60.0) as u64,
        (t % 60.0) as u64,
        m.latency_ms,
        m.ring_fill_ms,
        bar(m.input_peak),
        bar(m.output_peak),
        m.underruns,
        m.overruns,
        s.restarts,
        if m.audio_detected { "yes" } else { "no" },
    )
}

fn resolve_preset(args: &LiveArgs) -> Result<Preset> {
    if let Some(file) = &args.preset_file {
        let text = std::fs::read_to_string(file)
            .with_context(|| format!("cannot read {}", file.display()))?;
        return Ok(Preset::from_toml(&text)?);
    }
    builtin_preset(&args.preset).with_context(|| {
        let names: Vec<String> = builtin_presets().into_iter().map(|p| p.name).collect();
        format!(
            "unknown preset {:?}; available: {}",
            args.preset,
            names.join(", ")
        )
    })
}

/// `lofify passthrough`
pub fn passthrough(args: &EngineArgs) -> Result<()> {
    run(args, &Params::default(), false, "Clean (passthrough)")
}

/// `lofify live`
pub fn live(args: &LiveArgs) -> Result<()> {
    let preset = resolve_preset(args)?;
    run(&args.engine, &preset.params, args.bypass, &preset.name)
}

fn run(args: &EngineArgs, params: &Params, bypass: bool, preset_name: &str) -> Result<()> {
    let shared_params = Arc::new(SharedParams::new(params));
    shared_params.set_bypass(bypass);

    let engine = if args.simulate {
        let world = SimWorld::windows_like();
        world.set_time_scale(args.sim_speed);
        world.set_capture_ppm(args.sim_ppm);
        println!(
            "simulation: windows-like machine, speed x{}, capture clock {:+} ppm",
            args.sim_speed, args.sim_ppm
        );
        Engine::spawn(
            move || Ok(Box::new(world.backend())),
            engine_config(args),
            shared_params.clone(),
        )
    } else {
        if !lofify_engine::platform::is_supported() {
            bail!("no audio backend for this operating system; try --simulate");
        }
        Engine::spawn(
            lofify_engine::platform::default_backend,
            engine_config(args),
            shared_params.clone(),
        )
    };
    let speed = if args.simulate { args.sim_speed } else { 1.0 };

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst))
            .context("cannot install Ctrl+C handler")?;
    }
    let commands = spawn_stdin_reader();

    engine.set_devices(DeviceConfig {
        capture: parse_capture(&args.capture),
        output: parse_output(&args.output),
    });
    engine.start();
    println!(
        "preset: {preset_name}{}",
        if bypass { " (bypassed)" } else { "" }
    );
    println!(
        "running; press Ctrl+C to stop (live: type `b` + Enter to toggle bypass, `preset <name>` to switch)\n"
    );

    let started = Instant::now();
    let mut latency_sum = 0.0f64;
    let mut latency_n = 0u32;
    let mut last_report = Instant::now() - Duration::from_secs(3600);
    let mut hinted = false;
    while !stop.load(Ordering::SeqCst) {
        if args.seconds > 0 && started.elapsed() >= Duration::from_secs(args.seconds) {
            break;
        }
        while let Ok(cmd) = commands.try_recv() {
            if handle_command(&cmd, &shared_params) {
                stop.store(true, Ordering::SeqCst);
            }
        }
        if last_report.elapsed() >= Duration::from_secs(args.report_every.max(1)) {
            last_report = Instant::now();
            let status = engine.status();
            println!("{}", status_line(started.elapsed(), speed, &status));
            if status.state == EngineState::Running && status.metrics.latency_ms > 0.0 {
                latency_sum += f64::from(status.metrics.latency_ms);
                latency_n += 1;
            }
            if let EngineState::Error(why) = &status.state
                && !hinted
                && why.contains("feedback")
            {
                println!(
                    "  hint: choose your real speakers/headphones, e.g. --output \"Speakers\""
                );
                hinted = true;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let status = engine.status();
    engine.shutdown();
    let m = status.metrics;
    let secs = started.elapsed().as_secs_f64() * speed;
    println!("\nsummary");
    println!("  audio time      {secs:.1} s");
    println!("  underruns       {}", m.underruns);
    println!(
        "  overruns        {} (dropped {} frames)",
        m.overruns, m.dropped_frames
    );
    println!("  restarts        {}", status.restarts);
    if latency_n > 0 {
        println!(
            "  mean latency    {:.1} ms",
            latency_sum / f64::from(latency_n)
        );
    }
    if let Some(c) = &status.capture {
        println!(
            "  capture         {} ({} Hz, {} ch)",
            c.device_name, c.sample_rate, c.channels
        );
    }
    if let Some(o) = &status.output {
        println!(
            "  output          {} ({} Hz, {} ch)",
            o.device_name, o.sample_rate, o.channels
        );
    }
    Ok(())
}

/// Reads stdin lines on a helper thread so the main loop never blocks.
fn spawn_stdin_reader() -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin()
            .lock()
            .lines()
            .map_while(std::result::Result::ok)
        {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// Handles one stdin command; returns true to quit.
fn handle_command(cmd: &str, params: &SharedParams) -> bool {
    let cmd = cmd.trim();
    match cmd {
        "q" | "quit" | "exit" => return true,
        "b" | "bypass" => {
            let now = !params.bypass();
            params.set_bypass(now);
            println!("bypass {}", if now { "ON" } else { "off" });
        }
        _ => {
            if let Some(name) = cmd.strip_prefix("preset ") {
                match builtin_preset(name) {
                    Some(p) => {
                        params.store(&p.params);
                        println!("preset: {}", p.name);
                    }
                    None => println!("unknown preset {name:?}"),
                }
            } else if !cmd.is_empty() {
                println!("commands: b (toggle bypass), preset <name>, q");
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_argument_parsing() {
        assert_eq!(parse_capture("auto"), CaptureSelection::Auto);
        assert_eq!(parse_capture("AUTO"), CaptureSelection::Auto);
        assert_eq!(
            parse_capture("loopback"),
            CaptureSelection::Loopback(String::new())
        );
        assert_eq!(
            parse_capture("loopback: Speakers"),
            CaptureSelection::Loopback("Speakers".into())
        );
        assert_eq!(
            parse_capture("CABLE Output"),
            CaptureSelection::Input("CABLE Output".into())
        );
    }

    #[test]
    fn output_argument_parsing() {
        assert_eq!(parse_output("default"), OutputSelection::Default);
        assert_eq!(parse_output(""), OutputSelection::Default);
        assert_eq!(
            parse_output(" Speakers "),
            OutputSelection::Named("Speakers".into())
        );
    }

    #[test]
    fn meter_bars() {
        assert_eq!(bar(0.0), "----------");
        assert_eq!(bar(1.0), "##########");
        assert_eq!(bar(0.1).matches('#').count(), 7); // -20 dB
    }

    #[test]
    fn commands_toggle_bypass_and_switch_presets() {
        let p = SharedParams::default();
        assert!(!handle_command("b", &p));
        assert!(p.bypass());
        assert!(!handle_command("bypass", &p));
        assert!(!p.bypass());
        assert!(!handle_command("preset Cassette", &p));
        assert_eq!(p.load(), builtin_preset("Cassette").unwrap().params);
        assert!(!handle_command("preset nope", &p));
        assert!(handle_command("q", &p));
    }

    #[test]
    fn status_line_shows_the_important_numbers() {
        let s = Status {
            state: EngineState::Running,
            capture: None,
            output: None,
            restarts: 2,
            metrics: lofify_engine::metrics::Metrics {
                underruns: 3,
                overruns: 1,
                latency_ms: 42.5,
                ring_fill_ms: 20.0,
                audio_detected: true,
                ..Default::default()
            },
            backend: "test".into(),
        };
        let line = status_line(Duration::from_secs(75), 1.0, &s);
        for want in [
            "[01:15]",
            "running",
            "42.5 ms",
            "underruns 3",
            "overruns 1",
            "restarts 2",
            "audio yes",
        ] {
            assert!(line.contains(want), "{line:?} should contain {want:?}");
        }
    }

    #[test]
    fn status_line_explains_errors() {
        let s = Status {
            state: EngineState::Error("refusing to start to avoid a feedback loop".into()),
            capture: None,
            output: None,
            restarts: 0,
            metrics: Default::default(),
            backend: String::new(),
        };
        assert!(status_line(Duration::ZERO, 1.0, &s).contains("ERROR: refusing"));
    }
}
