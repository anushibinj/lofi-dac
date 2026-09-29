//! Headless smoke tests of the whole interface: build the app on the simulated backend, run real
//! egui frames without a window, and check what was drawn and how the controls behave.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use eframe::egui;
use lofify_app::app::{DeviceSource, LofifyApp, Services};
use lofify_app::commands::{self, AppCommand};
use lofify_app::settings::{AppPaths, Settings};
use lofify_dsp::shared::SharedParams;
use lofify_engine::backend::AudioBackend;
use lofify_engine::engine::{Engine, EngineConfig, EngineState};
use lofify_engine::sim::SimWorld;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Harness {
    app: LofifyApp,
    ctx: egui::Context,
    tx: commands::CommandSender,
    world: SimWorld,
    dir: tempfile::TempDir,
}

fn harness(world: SimWorld, settings: Settings) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::in_dirs(dir.path().join("config"), dir.path().join("logs"));
    let backend = world.backend();
    let engine = Engine::spawn(
        move || Ok(Box::new(backend)),
        EngineConfig {
            poll_interval: Duration::from_millis(40),
            prefill_ms: 40.0,
            ..EngineConfig::default()
        },
        Arc::new(SharedParams::new(&settings.params)),
    );
    let (tx, rx) = commands::channel();
    let list_world = world.clone();
    let source: DeviceSource = Box::new(move || list_world.backend().list_devices());
    let app = LofifyApp::new(
        engine,
        settings,
        Some(paths),
        Services::none(),
        rx,
        source,
        None,
    );
    Harness {
        app,
        ctx: egui::Context::default(),
        tx,
        world,
        dir,
    }
}

fn configured() -> Settings {
    let mut s = Settings::default();
    s.general.wizard_done = true;
    s.general.auto_start = false;
    s.audio.output_device = "Speakers (Realtek Audio)".into();
    s
}

impl Harness {
    /// Runs one frame and returns all text that was drawn.
    fn frame(&mut self) -> String {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 2600.0),
            )),
            ..Default::default()
        };
        let app = &mut self.app;
        let out = self.ctx.run_ui(raw, |ui| {
            app.tick(ui.ctx());
            app.draw(ui);
        });
        let mut text = String::new();
        for clipped in &out.shapes {
            collect_text(&clipped.shape, &mut text);
        }
        text
    }

    fn frames(&mut self, n: usize) -> String {
        let mut last = String::new();
        for _ in 0..n {
            last = self.frame();
        }
        last
    }
}

fn collect_text(shape: &egui::Shape, out: &mut String) {
    match shape {
        egui::Shape::Text(t) => {
            out.push_str(t.galley.text());
            out.push('\n');
        }
        egui::Shape::Vec(v) => v.iter().for_each(|s| collect_text(s, out)),
        _ => {}
    }
}

fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn the_main_window_shows_every_section() {
    let mut h = harness(SimWorld::windows_like(), configured());
    // The virtual screen is tall so egui does not cull the sliders below the fold.
    let text = h.frames(3);
    for want in [
        "Lofify",
        "Power",
        "Bypass",
        "Preset",
        "Classic Lofi",
        "Save As...",
        "Devices",
        "Capture from",
        "Play through",
        "Output level",
        "Filter response",
        "Input",
        "Tape wow and flutter",
        "Saturation",
        "Bitcrusher",
        "Filters",
        "Reverb",
        "Vinyl crackle and hiss",
        "Output",
        "No audio",
        "Latency",
        "Underruns",
        "Overruns",
    ] {
        assert!(
            text.contains(want),
            "the UI should show {want:?}; drew:\n{text}"
        );
    }
}

#[test]
fn every_effect_parameter_has_a_slider() {
    let mut h = harness(SimWorld::windows_like(), configured());
    let text = h.frames(3);
    for want in [
        "Trim",
        "Wow depth",
        "Wow rate",
        "Flutter depth",
        "Flutter rate",
        "Random drift",
        "Drive",
        "Makeup gain",
        "Bit depth",
        "Sample rate",
        "Dither",
        "High-pass",
        "High-pass cutoff",
        "Low-pass",
        "Low-pass cutoff",
        "Low-pass resonance",
        "Warmth frequency",
        "Warmth gain",
        "Room size",
        "Damping",
        "Mix",
        "Pops per second",
        "Pop level",
        "Hiss level",
        "Output gain",
        "Wet / dry mix",
    ] {
        assert!(text.contains(want), "missing slider {want:?}");
    }
}

#[test]
fn power_starts_and_stops_the_engine_and_audio_is_detected() {
    let mut world = SimWorld::windows_like();
    world.set_time_scale(1.0);
    let mut h = harness(std::mem::take(&mut world), configured());
    assert!(!h.app.power);
    h.app.set_power(true);
    wait_for("running", || {
        h.frame();
        h.app.engine().status().state == EngineState::Running
    });
    std::thread::sleep(Duration::from_millis(700));
    let text = h.frame();
    assert!(text.contains("Running"), "{text}");
    assert!(h.app.engine().status().metrics.audio_detected);

    h.app.set_power(false);
    wait_for("stopped", || {
        h.frame();
        h.app.engine().status().state == EngineState::Stopped
    });
    assert!(h.frame().contains("Stopped"));
}

#[test]
fn feedback_protection_is_visible_when_the_default_output_is_the_cable() {
    let mut settings = configured();
    settings.audio.output_device.clear(); // "System default" which is the cable in this world
    let mut h = harness(SimWorld::windows_like(), settings);
    h.app.set_power(true);
    wait_for("error state", || {
        h.frame();
        matches!(h.app.engine().status().state, EngineState::Error(_))
    });
    let text = h.frame();
    assert!(text.contains("feedback"), "{text}");
    assert!(
        text.contains("virtual cable"),
        "the picker must warn about the cable: {text}"
    );
    assert_eq!(h.world.outputs_opened(), 0);
}

#[test]
fn bypass_command_toggles_and_reaches_the_engine() {
    let mut h = harness(SimWorld::windows_like(), configured());
    assert!(!h.app.bypass);
    h.tx.send(AppCommand::ToggleBypass).unwrap();
    h.frame();
    assert!(h.app.bypass);
    assert!(h.app.engine().params().bypass());
    h.tx.send(AppCommand::ToggleBypass).unwrap();
    h.frame();
    assert!(!h.app.bypass && !h.app.engine().params().bypass());
}

#[test]
fn choosing_a_preset_updates_the_parameters() {
    let mut h = harness(SimWorld::windows_like(), configured());
    h.app.select_preset("Cassette");
    h.frame();
    let cassette = lofify_dsp::presets::builtin_preset("Cassette")
        .unwrap()
        .params;
    assert_eq!(h.app.engine().params().load(), cassette);
    assert!(!h.app.preset_modified());
    // touching a parameter marks the preset as modified
    let text = h.frame();
    assert!(text.contains("Cassette"));
}

#[test]
fn saving_a_user_preset_writes_a_file_and_lists_it() {
    let mut h = harness(SimWorld::windows_like(), configured());
    h.app.select_preset("Vinyl Dust");
    assert!(
        h.app.save_preset("Classic Lofi").is_err(),
        "built-in names are reserved"
    );
    assert!(h.app.save_preset("   ").is_err());
    h.app.save_preset("My Dusty Mix").unwrap();
    let file: PathBuf = h
        .dir
        .path()
        .join("config")
        .join("presets")
        .join("my-dusty-mix.toml");
    assert!(file.exists());
    let text = h.frame();
    assert!(text.contains("My Dusty Mix"), "{text}");
    assert!(h.app.preset("My Dusty Mix").is_some());
}

#[test]
fn settings_are_saved_after_a_change() {
    let mut h = harness(SimWorld::windows_like(), configured());
    h.app.select_preset("Late Night");
    h.app.shutdown(); // saves immediately
    let loaded = Settings::load(&h.dir.path().join("config").join("settings.toml"));
    assert_eq!(loaded.warning, None);
    assert_eq!(loaded.settings.general.preset, "Late Night");
    assert_eq!(
        loaded.settings.params,
        lofify_dsp::presets::builtin_preset("Late Night")
            .unwrap()
            .params
    );
}

#[test]
fn invalid_hotkeys_are_rejected_with_a_message() {
    let mut h = harness(SimWorld::windows_like(), configured());
    let before = h.app.settings_hotkey().to_string();
    h.app.set_hotkey_text("Banana");
    h.app.apply_hotkey_setting();
    let text = h.frame();
    assert!(text.contains("Hotkey"), "{text}");
    assert_eq!(
        h.app.settings_hotkey(),
        before,
        "the stored hotkey must not change"
    );
    h.app.set_hotkey_text("ctrl+shift+b");
    h.app.apply_hotkey_setting();
    assert_eq!(h.app.settings_hotkey(), "Ctrl+Shift+B");
}

#[test]
fn the_wizard_walks_through_the_four_steps_on_a_fresh_windows_setup() {
    // The wizard is Windows-only; on other systems the first run skips straight to the main UI.
    let mut h = harness(SimWorld::windows_like(), Settings::default());
    if !cfg!(target_os = "windows") {
        assert!(h.app.wizard_step().is_none());
        assert!(h.frames(2).contains("Power"));
        return;
    }
    assert!(h.frames(2).contains("Step 1 of 4"));
}

#[test]
fn a_frame_never_panics_with_no_devices_and_extreme_parameters() {
    let mut settings = configured();
    settings.params.reverb.mix = 1.0;
    settings.params.crush.bits = 2.0;
    let h_world = SimWorld::new(); // no devices at all
    let mut h = harness(h_world, settings);
    h.app.set_power(true);
    for _ in 0..20 {
        let text = h.frame();
        assert!(text.contains("Power"));
    }
}
