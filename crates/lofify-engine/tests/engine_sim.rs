//! End-to-end tests of the engine control thread against the simulated backend: real threads,
//! real ring buffers, real DSP, real reconnect logic.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use lofify_dsp::params::Params;
use lofify_dsp::shared::SharedParams;
use lofify_engine::engine::{Engine, EngineConfig, EngineState};
use lofify_engine::sim::{SimSignal, SimWorld};
use lofify_engine::types::{CaptureSelection, DeviceConfig, OutputSelection};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn fast_config() -> EngineConfig {
    EngineConfig {
        poll_interval: Duration::from_millis(40),
        // A little extra prefill keeps the simulation robust on loaded CI machines.
        prefill_ms: 40.0,
        startup_fade_ms: 10.0,
        ..EngineConfig::default()
    }
}

fn engine_for(world: &SimWorld) -> Engine {
    let backend = world.backend();
    Engine::spawn(
        move || Ok(Box::new(backend)),
        fast_config(),
        Arc::new(SharedParams::new(&Params::default())),
    )
}

fn speakers() -> DeviceConfig {
    DeviceConfig {
        capture: CaptureSelection::Auto,
        output: OutputSelection::Named("Speakers".into()),
    }
}

fn wait_for(what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

fn running(e: &Engine) -> bool {
    e.status().state == EngineState::Running
}

/// Frequency from rising zero crossings of the left channel.
fn frequency(stereo: &[f32], rate: f32) -> f32 {
    let l: Vec<f32> = stereo.chunks_exact(2).map(|f| f[0]).collect();
    let c: Vec<usize> = (1..l.len())
        .filter(|&i| l[i - 1] < 0.0 && l[i] >= 0.0)
        .collect();
    assert!(c.len() > 10, "no tone found ({} crossings)", c.len());
    (c.len() - 1) as f32 * rate / (c[c.len() - 1] - c[0]) as f32
}

fn peak(v: &[f32]) -> f32 {
    v.iter().fold(0.0, |m, s| m.max(s.abs()))
}

#[test]
fn passes_a_tone_from_the_cable_to_the_speakers() {
    let world = SimWorld::windows_like();
    let engine = engine_for(&world);
    engine.set_devices(speakers());
    engine.start();
    wait_for("running", Duration::from_secs(3), || running(&engine));
    std::thread::sleep(Duration::from_millis(1500));

    let status = engine.status();
    assert_eq!(status.state, EngineState::Running);
    assert!(
        status
            .capture
            .as_ref()
            .unwrap()
            .device_name
            .starts_with("CABLE Output")
    );
    assert!(
        status
            .output
            .as_ref()
            .unwrap()
            .device_name
            .starts_with("Speakers")
    );
    assert!(status.metrics.audio_detected, "{:?}", status.metrics);
    assert_eq!(status.metrics.resample_errors, 0);

    let recorded = world.recorded(&world.device_id("Speakers").unwrap());
    let tail = &recorded[recorded.len() / 3..];
    let f = frequency(tail, 48_000.0);
    assert!((f - 440.0).abs() < 2.0, "{f} Hz");
    assert!((peak(tail) - 0.5).abs() < 0.05, "peak {}", peak(tail));
    engine.shutdown();
}

#[test]
fn reports_a_plausible_measured_latency() {
    let world = SimWorld::windows_like();
    let engine = engine_for(&world);
    engine.set_devices(speakers());
    engine.start();
    wait_for("running", Duration::from_secs(3), || running(&engine));
    std::thread::sleep(Duration::from_millis(1000));
    let m = engine.metrics();
    // 40 ms prefill + 10 ms tape delay + 20 ms simulated device buffers
    assert!(
        m.latency_ms > 40.0 && m.latency_ms < 120.0,
        "{} ms",
        m.latency_ms
    );
    assert!((m.capture_latency_ms - 10.0).abs() < 0.1 && (m.output_latency_ms - 10.0).abs() < 0.1);
    assert!(m.ring_fill_ms > 20.0 && m.ring_fill_ms < 80.0, "{m:?}");
    engine.shutdown();
}

#[test]
fn refuses_to_output_to_the_virtual_cable() {
    // Windows is playing through "CABLE Input" (the default), so "default output" would feed
    // Lofify's own output back into its capture source.
    let world = SimWorld::windows_like();
    let engine = engine_for(&world);
    engine.set_devices(DeviceConfig {
        capture: CaptureSelection::Auto,
        output: OutputSelection::Default,
    });
    engine.start();
    wait_for("error state", Duration::from_secs(3), || {
        matches!(engine.status().state, EngineState::Error(_))
    });
    let EngineState::Error(msg) = engine.status().state else {
        unreachable!()
    };
    assert!(
        msg.contains("feedback") && msg.contains("CABLE Input"),
        "{msg}"
    );
    assert_eq!(
        world.captures_opened(),
        0,
        "nothing may be opened before the check passes"
    );
    assert_eq!(world.outputs_opened(), 0);

    // Naming the cable explicitly is refused as well.
    engine.set_devices(DeviceConfig {
        capture: CaptureSelection::Auto,
        output: OutputSelection::Named("CABLE Input".into()),
    });
    wait_for("still an error", Duration::from_secs(2), || {
        matches!(engine.status().state, EngineState::Error(_))
    });
    assert_eq!(world.outputs_opened(), 0);

    // Choosing a real device fixes it.
    engine.set_devices(speakers());
    wait_for("running", Duration::from_secs(3), || running(&engine));
    engine.shutdown();
}

#[test]
fn refuses_loopback_of_the_output_device_itself() {
    let world = SimWorld::windows_like();
    let engine = engine_for(&world);
    engine.set_devices(DeviceConfig {
        capture: CaptureSelection::Loopback("Speakers".into()),
        output: OutputSelection::Named("Speakers".into()),
    });
    engine.start();
    wait_for("error", Duration::from_secs(3), || {
        matches!(engine.status().state, EngineState::Error(_))
    });
    assert_eq!(world.captures_opened() + world.outputs_opened(), 0);
    engine.shutdown();
}

#[test]
fn loopback_capture_of_another_device_works() {
    let world = SimWorld::windows_like();
    let engine = engine_for(&world);
    engine.set_devices(DeviceConfig {
        capture: CaptureSelection::Loopback("CABLE Input".into()),
        output: OutputSelection::Named("Headphones".into()),
    });
    engine.start();
    wait_for("running", Duration::from_secs(3), || running(&engine));
    std::thread::sleep(Duration::from_millis(800));
    let rec = world.recorded(&world.device_id("Headphones").unwrap());
    assert!(peak(&rec) > 0.3);
    engine.shutdown();
}

#[test]
fn waits_for_a_missing_cable_and_starts_when_it_appears() {
    let world = SimWorld::new();
    world.add_input("Microphone", 48_000, 2);
    let out = world.add_output("Speakers", 48_000, 2, true);
    let engine = engine_for(&world);
    engine.set_devices(speakers());
    engine.start();
    wait_for(
        "waiting",
        Duration::from_secs(2),
        || matches!(engine.status().state, EngineState::Waiting(ref m) if m.contains("virtual audio cable")),
    );
    assert_eq!(world.captures_opened(), 0);

    world.add_input("CABLE Output (VB-Audio Virtual Cable)", 48_000, 2);
    wait_for("running", Duration::from_secs(3), || running(&engine));
    std::thread::sleep(Duration::from_millis(600));
    assert!(peak(&world.recorded(&out)) > 0.3);
    engine.shutdown();
}

#[test]
fn recovers_when_the_output_device_is_unplugged_and_replugged() {
    let world = SimWorld::windows_like();
    let engine = engine_for(&world);
    engine.set_devices(speakers());
    engine.start();
    wait_for("running", Duration::from_secs(3), || running(&engine));
    let old = world.device_id("Speakers").unwrap();

    world.remove_device(&old);
    wait_for("waiting after unplug", Duration::from_secs(3), || {
        matches!(engine.status().state, EngineState::Waiting(_))
    });
    assert!(engine.status().restarts >= 1);
    assert!(engine.status().output.is_none());

    let new = world.add_output("Speakers (Realtek Audio)", 48_000, 2, true);
    wait_for("running again", Duration::from_secs(3), || running(&engine));
    std::thread::sleep(Duration::from_millis(700));
    assert!(
        peak(&world.recorded(&new)) > 0.3,
        "audio must flow on the new device"
    );
    engine.shutdown();
}

#[test]
fn follows_changes_of_the_default_output() {
    let world = SimWorld::windows_like();
    // Windows plays through the cable; the real output is whatever the user's default *was*.
    // Model that by making the headphones the default output and selecting "default".
    let hp = world.device_id("Headphones").unwrap();
    let spk = world.device_id("Speakers").unwrap();
    world.set_default_output(&hp);
    let engine = engine_for(&world);
    engine.set_devices(DeviceConfig {
        capture: CaptureSelection::Auto,
        output: OutputSelection::Default,
    });
    engine.start();
    wait_for("running", Duration::from_secs(3), || running(&engine));
    std::thread::sleep(Duration::from_millis(500));
    assert!(peak(&world.recorded(&hp)) > 0.3);
    assert!(world.recorded(&spk).is_empty());

    world.set_default_output(&spk);
    wait_for("rebuilt on the new default", Duration::from_secs(3), || {
        peak(&world.recorded(&spk)) > 0.3
    });
    assert!(engine.status().restarts >= 1);
    engine.shutdown();
}

#[test]
fn a_stream_error_triggers_an_automatic_rebuild() {
    let world = SimWorld::windows_like();
    let engine = engine_for(&world);
    engine.set_devices(speakers());
    engine.start();
    wait_for("running", Duration::from_secs(3), || running(&engine));
    world.inject_capture_error();
    wait_for("rebuild", Duration::from_secs(4), || {
        engine.status().restarts >= 1 && running(&engine)
    });
    world.clear_recordings();
    std::thread::sleep(Duration::from_millis(700));
    assert!(peak(&world.recorded(&world.device_id("Speakers").unwrap())) > 0.3);

    world.inject_output_error();
    wait_for("second rebuild", Duration::from_secs(4), || {
        engine.status().restarts >= 2 && running(&engine)
    });
    engine.shutdown();
}

#[test]
fn silent_capture_is_not_an_error_and_audio_resumes() {
    // WASAPI loopback stops delivering packets when nothing plays.
    let world = SimWorld::windows_like();
    let engine = engine_for(&world);
    engine.set_devices(speakers());
    engine.start();
    wait_for("running", Duration::from_secs(3), || running(&engine));
    std::thread::sleep(Duration::from_millis(500));

    world.set_capture_active(false);
    std::thread::sleep(Duration::from_millis(1500));
    let idle = engine.status();
    assert_eq!(
        idle.state,
        EngineState::Running,
        "starvation is normal silence"
    );
    assert_eq!(idle.metrics.underruns, 0, "{:?}", idle.metrics);
    assert!(idle.metrics.waiting_for_audio);
    assert!(!idle.metrics.audio_detected);
    assert_eq!(idle.restarts, 0);

    world.clear_recordings();
    world.set_capture_active(true);
    std::thread::sleep(Duration::from_millis(1200));
    let rec = world.recorded(&world.device_id("Speakers").unwrap());
    assert!(peak(&rec) > 0.3, "audio must come back after the gap");
    assert!(engine.status().metrics.audio_detected);
    // The idle gap itself was never counted (asserted above); tolerate one genuine scheduling
    // glitch from a heavily loaded machine.
    assert!(engine.status().metrics.underruns <= 1);
    engine.shutdown();
}

#[test]
fn different_device_sample_rates_are_resampled() {
    let world = SimWorld::new();
    world.add_input("CABLE Output (VB-Audio Virtual Cable)", 44_100, 2);
    let out = world.add_output("Speakers", 48_000, 2, false); // cannot convert by itself
    let engine = engine_for(&world);
    engine.set_devices(speakers());
    engine.start();
    wait_for("running", Duration::from_secs(3), || running(&engine));
    std::thread::sleep(Duration::from_millis(1500));
    let s = engine.status();
    assert_eq!(s.capture.unwrap().sample_rate, 44_100);
    assert_eq!(s.output.unwrap().sample_rate, 48_000);
    let rec = world.recorded(&out);
    let f = frequency(&rec[rec.len() / 3..], 48_000.0);
    assert!((f - 440.0).abs() < 2.0, "{f} Hz");
    engine.shutdown();
}

#[test]
fn surround_capture_is_downmixed_and_mono_output_works() {
    let world = SimWorld::new();
    world.add_input("CABLE Output (VB-Audio Virtual Cable)", 48_000, 6);
    let out = world.add_output("Mono Speaker", 48_000, 1, true);
    let engine = engine_for(&world);
    engine.set_devices(DeviceConfig {
        capture: CaptureSelection::Auto,
        output: OutputSelection::Named("Mono".into()),
    });
    engine.start();
    wait_for("running", Duration::from_secs(3), || running(&engine));
    std::thread::sleep(Duration::from_millis(800));
    // the sim writes the tone into all six channels; FL+FR (1.0 each) plus centre/rear folds
    // give a loud but bounded, finite signal
    let rec = world.recorded(&out);
    assert!(peak(&rec) > 0.3 && rec.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
    engine.shutdown();
}

#[test]
fn stop_and_start_and_device_switch_while_running() {
    let world = SimWorld::windows_like();
    let engine = engine_for(&world);
    engine.set_devices(speakers());
    engine.start();
    wait_for("running", Duration::from_secs(3), || running(&engine));

    engine.stop();
    wait_for("stopped", Duration::from_secs(2), || {
        engine.status().state == EngineState::Stopped
    });
    assert!(engine.status().capture.is_none());

    engine.start();
    wait_for("running again", Duration::from_secs(3), || running(&engine));

    let hp = world.device_id("Headphones").unwrap();
    engine.set_devices(DeviceConfig {
        capture: CaptureSelection::Auto,
        output: OutputSelection::Named("Headphones".into()),
    });
    wait_for("audio on the headphones", Duration::from_secs(4), || {
        peak(&world.recorded(&hp)) > 0.3
    });
    engine.shutdown();
}

#[test]
fn live_parameter_changes_are_heard_through_the_engine() {
    let world = SimWorld::windows_like();
    let engine = engine_for(&world);
    engine.set_devices(speakers());
    engine.start();
    wait_for("running", Duration::from_secs(3), || running(&engine));
    std::thread::sleep(Duration::from_millis(600));
    let spk = world.device_id("Speakers").unwrap();
    let before = peak(&world.recorded(&spk)[world.recorded(&spk).len() / 2..]);
    assert!((before - 0.5).abs() < 0.05);

    let mut p = Params::default();
    p.output.gain_db = -12.0;
    engine.params().store(&p);
    std::thread::sleep(Duration::from_millis(800));
    let rec = world.recorded(&spk);
    let after = peak(&rec[rec.len() - 4_800..]);
    assert!((after - 0.5 * 0.251).abs() < 0.02, "{after}");
    engine.shutdown();
}

#[test]
fn stays_stable_over_a_longer_run_with_a_drifting_capture_clock() {
    // ~16 simulated seconds at 2x speed with a capture clock that is 300 ppm fast. Clock drift
    // itself is tested deterministically in tests/renderer.rs; this is the threaded smoke test:
    // no restarts, no runaway latency, no dropped audio, and (allowing for scheduling noise on
    // a busy machine, amplified by the time scale) at most a couple of glitches.
    let world = SimWorld::windows_like();
    world.set_time_scale(2.0);
    world.set_capture_ppm(300.0);
    world.set_signal(SimSignal::Sine {
        hz: 300.0,
        amp: 0.4,
    });
    let engine = engine_for(&world);
    engine.set_devices(speakers());
    engine.start();
    wait_for("running", Duration::from_secs(3), || running(&engine));
    std::thread::sleep(Duration::from_secs(8));
    let s = engine.status();
    let m = s.metrics;
    assert!(
        m.output_frames > 48_000 * 14,
        "ran {} frames",
        m.output_frames
    );
    assert!(m.underruns <= 4, "{m:?}");
    // A stalled thread makes the renderer trim the ring (bounded latency by design), so allow a
    // couple of those on a machine that is deliberately oversubscribed in stress runs.
    assert!(m.overruns <= 2, "{m:?}");
    assert_eq!(s.restarts, 0);
    assert_eq!(s.state, EngineState::Running);
    assert!(m.ring_fill_ms > 10.0 && m.ring_fill_ms < 100.0, "{m:?}");
    engine.shutdown();
}

#[test]
fn unsupported_backend_reports_an_error_instead_of_crashing() {
    let engine = Engine::spawn(
        || {
            Err(lofify_engine::error::EngineError::Unsupported(
                "nothing here".into(),
            ))
        },
        fast_config(),
        Arc::new(SharedParams::default()),
    );
    engine.start();
    wait_for(
        "error",
        Duration::from_secs(2),
        || matches!(engine.status().state, EngineState::Error(ref m) if m.contains("nothing here")),
    );
    engine.stop();
    engine.shutdown();
}

#[test]
fn dropping_the_engine_shuts_down_cleanly() {
    let world = SimWorld::windows_like();
    let start = Instant::now();
    {
        let engine = engine_for(&world);
        engine.set_devices(speakers());
        engine.start();
        wait_for("running", Duration::from_secs(3), || running(&engine));
    }
    assert!(start.elapsed() < Duration::from_secs(6));
}
