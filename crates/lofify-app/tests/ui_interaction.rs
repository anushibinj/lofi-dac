//! Click-through tests: the real widgets are found through their accessibility labels and
//! clicked, exactly as a user would, against the simulated audio backend.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use lofify_app::app::{DeviceSource, LofifyApp, Services};
use lofify_app::commands;
use lofify_app::settings::{AppPaths, Settings};
use lofify_app::wizard::Step;
use lofify_dsp::shared::SharedParams;
use lofify_engine::backend::AudioBackend;
use lofify_engine::engine::{Engine, EngineConfig, EngineState};
use lofify_engine::sim::SimWorld;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Rig {
    harness: Harness<'static, LofifyApp>,
    world: SimWorld,
    _dir: tempfile::TempDir,
}

fn configured() -> Settings {
    let mut s = Settings::default();
    s.general.wizard_done = true;
    s.general.auto_start = false;
    s.audio.output_device = "Speakers (Realtek Audio)".into();
    s
}

fn rig(world: SimWorld, settings: Settings) -> Rig {
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
    let (_tx, rx) = commands::channel();
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
    let harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 2600.0))
        .build_ui_state(
            |ui, app: &mut LofifyApp| {
                let ctx = ui.ctx().clone();
                app.tick(&ctx);
                app.draw(ui);
            },
            app,
        );
    Rig {
        harness,
        world,
        _dir: dir,
    }
}

impl Rig {
    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.harness.step();
        }
    }

    fn app(&self) -> &LofifyApp {
        self.harness.state()
    }

    fn wait_for(&mut self, what: &str, cond: impl Fn(&LofifyApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            self.harness.step();
            if cond(self.harness.state()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {what}");
    }
}

#[test]
fn clicking_power_starts_processing_and_clicking_again_stops_it() {
    let mut r = rig(SimWorld::windows_like(), configured());
    r.steps(2);
    r.harness.get_by_label("Power  OFF").click();
    r.steps(2);
    assert!(r.app().power);
    r.wait_for("engine running", |a| {
        a.engine().status().state == EngineState::Running
    });
    r.harness.get_by_label("Power  ON").click();
    r.steps(2);
    assert!(!r.app().power);
    r.wait_for("engine stopped", |a| {
        a.engine().status().state == EngineState::Stopped
    });
    assert!(r.harness.query_by_label("Power  OFF").is_some());
    assert!(r.world.outputs_opened() >= 1);
}

#[test]
fn clicking_bypass_toggles_it_and_updates_the_engine() {
    let mut r = rig(SimWorld::windows_like(), configured());
    r.steps(2);
    r.harness.get_by_label("Bypass").click();
    r.steps(2);
    assert!(r.app().bypass);
    assert!(r.app().engine().params().bypass());
    r.harness.get_by_label("Bypass  ON").click();
    r.steps(2);
    assert!(!r.app().bypass && !r.app().engine().params().bypass());
}

#[test]
fn save_as_opens_a_name_field_and_saves_a_preset() {
    let mut r = rig(SimWorld::windows_like(), configured());
    r.steps(2);
    r.harness.get_by_label("Save As...").click();
    r.steps(2);
    let field = r.harness.get_by_role(egui::accesskit::Role::TextInput);
    field.focus();
    field.type_text("Rainy Day");
    r.steps(2);
    r.harness.get_by_label("Save").click();
    r.steps(3);
    assert!(
        r.app().preset("Rainy Day").is_some(),
        "the preset must exist after saving"
    );
    assert_eq!(
        r.app().settings.general.preset,
        "Rainy Day",
        "and become the selected preset"
    );
}

#[test]
fn saving_a_reserved_name_shows_an_error_instead() {
    let mut r = rig(SimWorld::windows_like(), configured());
    r.steps(2);
    r.harness.get_by_label("Save As...").click();
    r.steps(2);
    let field = r.harness.get_by_role(egui::accesskit::Role::TextInput);
    field.focus();
    field.type_text("Cassette");
    r.steps(2);
    r.harness.get_by_label("Save").click();
    r.steps(3);
    assert!(
        r.harness
            .query_by_label_contains("built-in preset")
            .is_some()
    );
    assert!(
        r.app()
            .preset("Cassette")
            .is_some_and(|p| p.description != "Saved from the Lofify app.")
    );
}

#[test]
fn the_preset_menu_switches_presets() {
    let mut r = rig(SimWorld::windows_like(), configured());
    r.steps(2);
    // The first combo box in the layout is the preset menu (then capture and output).
    r.harness
        .get_all_by_role(egui::accesskit::Role::ComboBox)
        .next()
        .unwrap()
        .click();
    r.steps(3);
    r.harness.get_by_label("Cassette").click();
    r.steps(3);
    let cassette = lofify_dsp::presets::builtin_preset("Cassette")
        .unwrap()
        .params;
    assert_eq!(r.app().params, cassette);
    assert_eq!(r.app().settings.general.preset, "Cassette");
}

#[test]
fn moving_a_slider_marks_the_preset_modified_and_reaches_the_engine() {
    let mut r = rig(SimWorld::windows_like(), configured());
    r.steps(2);
    assert!(!r.app().preset_modified());
    r.harness
        .get_by_role_and_label(egui::accesskit::Role::Slider, "Room size")
        .focus();
    r.harness.key_press(egui::Key::ArrowRight);
    r.steps(3);
    assert!(
        r.app().preset_modified(),
        "a slider change must mark the preset as modified"
    );
    assert_eq!(
        r.app().engine().params().load().reverb.room,
        r.app().params.reverb.room
    );
}

#[test]
fn the_wizard_walks_through_all_four_steps_and_finishes_with_a_working_setup() {
    let mut r = rig(SimWorld::windows_like(), configured());
    r.harness.state_mut().restart_wizard();
    r.steps(2);
    assert_eq!(r.app().wizard_step(), Some(Step::Cable));
    assert!(r.harness.query_by_label_contains("Step 1 of 4").is_some());
    assert!(
        r.harness
            .query_by_label_contains("Found the virtual cable")
            .is_some()
    );

    r.harness.get_by_label("Next").click();
    r.steps(2);
    assert_eq!(r.app().wizard_step(), Some(Step::DefaultPlayback));
    assert!(
        r.harness
            .query_by_label_contains("Windows is playing sound through the cable")
            .is_some()
    );

    r.harness.get_by_label("Next").click();
    r.steps(2);
    assert_eq!(r.app().wizard_step(), Some(Step::Output));
    // The picker never offers the cable; choose the headphones the way the combo box would.
    r.harness.state_mut().wizard.as_mut().unwrap().output = "Headphones (USB DAC)".into();
    r.steps(2);

    r.harness.get_by_label("Next").click();
    r.steps(2);
    assert_eq!(r.app().wizard_step(), Some(Step::Confirm));
    r.wait_for("audio detected", |a| {
        a.engine().status().metrics.audio_detected
    });
    r.steps(2);
    assert!(r.harness.query_by_label_contains("It works").is_some());

    r.harness.get_by_label("Finish").click();
    r.steps(2);
    assert_eq!(r.app().wizard_step(), None);
    assert!(r.app().settings.general.wizard_done);
    assert_eq!(r.app().settings.audio.output_device, "Headphones (USB DAC)");
    assert!(r.app().power);
    let hp = r.world.device_id("Headphones").unwrap();
    assert!(
        !r.world.recorded(&hp).is_empty(),
        "audio must have reached the chosen headphones"
    );
}

#[test]
fn the_wizard_explains_a_missing_cable_and_blocks_next() {
    let world = SimWorld::new();
    world.add_output("Speakers", 48_000, 2, true);
    let mut r = rig(world, configured());
    r.harness.state_mut().restart_wizard();
    r.steps(2);
    assert!(
        r.harness
            .query_by_label_contains("No virtual audio cable found")
            .is_some()
    );
    assert!(
        r.harness
            .query_by_label_contains("Download VB-CABLE")
            .is_some()
    );
    assert!(
        r.harness
            .get_by_label("Next")
            .accesskit_node()
            .is_disabled(),
        "Next must stay disabled without a cable"
    );
    // ...until the cable appears and the list refreshes.
    r.world
        .add_input("CABLE Output (VB-Audio Virtual Cable)", 48_000, 2);
    r.harness.get_by_label("Re-check").click();
    r.steps(2);
    assert!(
        !r.harness
            .get_by_label("Next")
            .accesskit_node()
            .is_disabled()
    );
}

#[test]
fn skipping_the_wizard_marks_it_done() {
    let mut r = rig(SimWorld::windows_like(), configured());
    r.harness.state_mut().restart_wizard();
    r.steps(2);
    r.harness.get_by_label("Skip setup").click();
    r.steps(2);
    assert_eq!(r.app().wizard_step(), None);
    assert!(r.app().settings.general.wizard_done);
    assert!(
        r.harness.query_by_label("Power  OFF").is_some(),
        "the main window must come back"
    );
}
