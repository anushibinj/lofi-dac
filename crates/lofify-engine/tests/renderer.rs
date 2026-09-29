//! Deterministic tests of the output renderer: no threads, no sleeping, a simulated clock.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use lofify_dsp::chain::ChainConfig;
use lofify_dsp::params::Params;
use lofify_dsp::shared::SharedParams;
use lofify_engine::metrics::Shared;
use lofify_engine::output::{OutputRenderer, RendererConfig};
use rtrb::{Producer, RingBuffer};
use std::f32::consts::TAU;
use std::sync::Arc;

const DSP_LATENCY: usize = 480; // 10 ms at 48 kHz

struct Rig {
    producer: Producer<f32>,
    renderer: OutputRenderer,
    shared: Arc<Shared>,
    params: Arc<SharedParams>,
}

/// Same rates, no resampler, no fades: exact and easy to reason about.
fn exact_cfg() -> RendererConfig {
    RendererConfig {
        drift_compensation: false,
        startup_fade_ms: 0.0,
        resume_fade_ms: 0.0,
        chain: ChainConfig {
            base_delay_ms: 10.0,
            seed: 1,
        },
        ..RendererConfig::default()
    }
}

fn rig(cfg: RendererConfig) -> Rig {
    let (producer, consumer) = RingBuffer::<f32>::new(2 * 48_000);
    let shared = Arc::new(Shared::default());
    let params = Arc::new(SharedParams::new(&Params::default()));
    let renderer = OutputRenderer::new(consumer, params.clone(), shared.clone(), cfg).unwrap();
    Rig {
        producer,
        renderer,
        shared,
        params,
    }
}

impl Rig {
    fn push(&mut self, stereo: &[f32]) {
        let (pushed, rest) = self.producer.push_partial_slice(stereo);
        assert_eq!(pushed.len(), stereo.len());
        assert!(rest.is_empty());
    }

    fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut out = vec![9.0f32; frames * 2];
        self.renderer.render(&mut out, 2);
        out
    }
}

fn sine(rate: f32, hz: f32, start: usize, frames: usize, amp: f32) -> Vec<f32> {
    (start..start + frames)
        .flat_map(|i| {
            let v = amp * (TAU * hz * i as f32 / rate).sin();
            [v, v]
        })
        .collect()
}

fn dc(frames: usize, v: f32) -> Vec<f32> {
    vec![v; frames * 2]
}

/// Runs `ticks` callbacks of 480 frames, pushing `in_per_tick` (fractional) input frames before
/// each one, and returns everything rendered.
fn run_ticks(rig: &mut Rig, ticks: usize, in_per_tick: f64, in_rate: f32, hz: f32) -> Vec<f32> {
    let mut out = Vec::with_capacity(ticks * 960);
    let mut owed = 0.0f64;
    let mut written = 0usize;
    for _ in 0..ticks {
        owed += in_per_tick;
        let n = owed.floor() as usize;
        owed -= n as f64;
        rig.push(&sine(in_rate, hz, written, n, 0.5));
        written += n;
        out.extend(rig.render(480));
    }
    out
}

#[test]
fn outputs_exact_silence_until_the_prefill_is_reached() {
    let mut r = rig(exact_cfg());
    r.push(&dc(480, 0.4)); // 10 ms of the 20 ms prefill
    let out = r.render(480);
    assert!(out.iter().all(|&v| v == 0.0));
    assert!(r.shared.snapshot().waiting_for_audio);
    assert_eq!(
        r.shared.underruns(),
        0,
        "initial prefill is not an underrun"
    );
}

#[test]
fn audio_starts_once_prefilled_and_is_the_input_delayed_by_the_dsp_latency() {
    let mut r = rig(exact_cfg());
    let input = sine(48_000.0, 440.0, 0, 48_000, 0.5);
    let mut out = Vec::new();
    for tick in 0..100 {
        r.push(&input[tick * 960..(tick + 1) * 960]);
        out.extend(r.render(480));
    }
    // Prefill of 960 frames means the first callback outputs silence, then audio flows.
    assert!(!r.shared.snapshot().waiting_for_audio);
    // Find the offset between input and output by exact matching.
    let out_l: Vec<f32> = out.chunks_exact(2).map(|f| f[0]).collect();
    let in_l: Vec<f32> = input.chunks_exact(2).map(|f| f[0]).collect();
    let start = out_l
        .iter()
        .position(|&v| v != 0.0)
        .expect("audio must appear");
    let offset = start - 1; // sine[0] == 0, so the first non-zero sample is input index 1
    for n in 0..20_000 {
        assert_eq!(out_l[offset + n], in_l[n], "sample {n}");
    }
    // The delay is the DSP latency plus the prefill wait (one 480-frame callback of silence).
    assert_eq!(offset, DSP_LATENCY + 480);
}

#[test]
fn underrun_is_counted_once_and_playback_recovers() {
    let mut r = rig(exact_cfg());
    r.push(&dc(2_400, 0.4));
    for _ in 0..5 {
        r.render(480); // drains the ring exactly
    }
    let starved = r.render(480);
    assert_eq!(
        r.shared.underruns(),
        0,
        "not counted until capture comes back"
    );
    assert!(starved.iter().all(|&v| v.is_finite()));
    assert!(r.shared.snapshot().waiting_for_audio);
    // Capture comes back within 100 ms.
    r.render(480);
    r.push(&dc(2_000, 0.4));
    r.render(480);
    assert_eq!(r.shared.underruns(), 1);
    let out = r.render(480 * 2);
    assert!(out.iter().skip(2 * 480).all(|&v| (v - 0.4).abs() < 1.0e-6));
}

#[test]
fn a_long_capture_gap_is_idle_silence_not_an_underrun() {
    let mut r = rig(exact_cfg());
    r.push(&dc(2_400, 0.4));
    for _ in 0..5 {
        r.render(480);
    }
    // 2 seconds of nothing (WASAPI loopback stops delivering packets in silence).
    for i in 0..200 {
        let out = r.render(480);
        // the first callbacks still flush the 10 ms DSP delay line
        if i >= 3 {
            assert!(
                out.iter().all(|&v| v == 0.0),
                "the chain must output silence when idle"
            );
        }
    }
    r.push(&dc(2_000, 0.4));
    r.render(480);
    r.render(480);
    assert_eq!(r.shared.underruns(), 0);
    assert_eq!(r.shared.overruns(), 0);
    assert!(!r.shared.snapshot().waiting_for_audio);
}

#[test]
fn underrun_fades_out_instead_of_clicking() {
    let mut r = rig(exact_cfg());
    r.push(&dc(1_400, 0.5));
    r.render(480); // waiting -> running, delay line fills
    r.render(480);
    // Only 440 frames remain; the next 480-frame callback cannot be satisfied.
    let out = r.render(480);
    let (l, _): (Vec<f32>, Vec<f32>) = out.chunks_exact(2).map(|f| (f[0], f[1])).unzip();
    let max_step = l
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        max_step < 0.01,
        "abrupt step of {max_step} during the underrun"
    );
}

#[test]
fn overrun_drops_the_oldest_frames_and_bounds_latency() {
    let mut r = rig(exact_cfg());
    // 20 000 frames of a rising ramp arrive before the first callback (max fill is 120 ms).
    let ramp: Vec<f32> = (0..20_000)
        .flat_map(|i| [i as f32 / 40_000.0, i as f32 / 40_000.0])
        .collect();
    r.push(&ramp);
    let first = r.render(480);
    let m = r.shared.snapshot();
    assert_eq!(m.overruns, 1);
    assert_eq!(
        m.dropped_frames,
        20_000 - 960,
        "trimmed down to the 20 ms prefill"
    );
    assert!(
        m.ring_fill_ms <= 20.5,
        "latency must be bounded, ring holds {} ms",
        m.ring_fill_ms
    );
    assert!(
        first.iter().all(|&v| v == 0.0),
        "still inside the DSP delay"
    );
    // The audio that survives is the NEWEST: it starts at input frame 19 040. The first 480
    // frames it produced are still inside the 480-frame DSP delay, so the ramp appears in the
    // next callback, starting exactly at input frame 19 040.
    let second = r.render(480);
    assert!(
        (second[0] - 19_040.0 / 40_000.0).abs() < 1.0e-6,
        "expected the newest audio, got {}",
        second[0]
    );
    assert_eq!(r.shared.overruns(), 1, "no further overruns");
}

#[test]
fn non_finite_input_never_reaches_the_output() {
    let mut r = rig(exact_cfg());
    let mut poison = dc(3_000, 0.3);
    for i in (0..poison.len()).step_by(37) {
        poison[i] = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY][i % 3];
    }
    r.push(&poison);
    for _ in 0..6 {
        assert!(
            r.render(480)
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 1.0)
        );
    }
}

#[test]
fn startup_fade_ramps_the_output_over_50ms() {
    let mut r = rig(RendererConfig {
        startup_fade_ms: 50.0,
        resume_fade_ms: 5.0,
        ..exact_cfg()
    });
    let mut out = Vec::new();
    for _ in 0..30 {
        r.push(&dc(480, 0.4));
        out.extend(r.render(480));
    }
    let l: Vec<f32> = out.chunks_exact(2).map(|f| f[0]).collect();
    // Audio reaches the output after prefill + DSP delay; the fade started at frame 0 and is
    // 2400 frames long, so by the time DC appears (frame 960+) it is already 40 % up.
    let first = l.iter().position(|&v| v != 0.0).unwrap();
    assert!(
        l[first] < 0.4 * 0.6,
        "first audible sample must be attenuated: {}",
        l[first]
    );
    let max_step = l
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(max_step < 0.002, "the fade must be smooth, step {max_step}");
    assert!(
        (l[l.len() - 1] - 0.4).abs() < 1.0e-6,
        "must end at full level"
    );
}

#[test]
fn resuming_after_a_gap_fades_in_the_input() {
    let mut r = rig(RendererConfig {
        resume_fade_ms: 5.0,
        ..exact_cfg()
    });
    let mut out = Vec::new();
    r.push(&dc(2_000, 0.5));
    for _ in 0..4 {
        out.extend(r.render(480));
    }
    // a gap, then the input returns at full level
    for _ in 0..3 {
        out.extend(r.render(480));
    }
    r.push(&dc(2_000, 0.5));
    for _ in 0..6 {
        out.extend(r.render(480));
    }
    let l: Vec<f32> = out.chunks_exact(2).map(|f| f[0]).collect();
    let max_step = l
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    // Fades are 0.5 over at least ~80 frames (about 0.006 per frame); a click would be ~0.5.
    assert!(max_step < 0.01, "step of {max_step}");
}

#[test]
fn channel_mapping_on_the_output_side() {
    let mut r = rig(exact_cfg());
    r.push(&dc(3_000, 0.5));
    for _ in 0..3 {
        r.render(480);
    }
    // 5.1 device with i16 samples: front pair carries the signal, the rest is silent.
    let mut buf = vec![7i16; 480 * 6];
    r.renderer.render(&mut buf, 6);
    for frame in buf.chunks_exact(6) {
        assert!((f32::from(frame[0]) / 32_768.0 - 0.5).abs() < 1.0e-3);
        assert_eq!(frame[0], frame[1]);
        assert_eq!(&frame[2..], &[0, 0, 0, 0]);
    }
    // mono device
    let mut mono = vec![7.0f32; 480];
    r.renderer.render(&mut mono, 1);
    assert!(mono.iter().all(|&v| (v - 0.5).abs() < 1.0e-6));
    // u16 (offset binary) silence is the midpoint, including the odd leftover sample
    let mut odd = vec![1u16; 5];
    r.renderer.render(&mut odd, 2);
    assert_eq!(odd[4], 32_768);
}

#[test]
fn live_parameter_changes_reach_the_audio() {
    let mut r = rig(exact_cfg());
    let mut last = Vec::new();
    for tick in 0..40 {
        if tick == 5 {
            let mut p = Params::default();
            p.output.gain_db = -6.0206;
            r.params.store(&p);
        }
        r.push(&dc(480, 0.4));
        last = r.render(480);
        if tick == 4 {
            assert!(
                (last[last.len() - 2] - 0.4).abs() < 1.0e-6,
                "unchanged before the update"
            );
        }
    }
    // the 10 ms parameter smoothing needs about 13 time constants (130 ms) to settle
    assert!(
        (last[last.len() - 2] - 0.2).abs() < 1.0e-3,
        "{}",
        last[last.len() - 2]
    );
}

#[test]
fn bypass_is_applied_from_the_shared_flag() {
    let mut r = rig(exact_cfg());
    let mut p = Params::default();
    p.output.gain_db = 6.0;
    r.params.store(&p);
    r.params.set_bypass(true);
    r.push(&dc(4_000, 0.25));
    let mut out = Vec::new();
    for _ in 0..7 {
        out.extend(r.render(480));
    }
    // bypassed: the wet +6 dB must not be heard
    assert!((out[out.len() - 2] - 0.25).abs() < 1.0e-6);
}

#[test]
fn meters_and_audio_detection() {
    let mut r = rig(exact_cfg());
    let mut out_peak = 0.0f32;
    for _ in 0..12 {
        r.push(&sine(48_000.0, 1_000.0, 0, 480, 0.5));
        r.render(480);
        out_peak = out_peak.max(r.shared.snapshot().output_peak);
    }
    let m = r.shared.snapshot();
    assert!(m.audio_detected);
    assert!(m.input_peak > 0.4);
    assert!(out_peak > 0.4, "{out_peak}");
    // silence for over half a second
    for _ in 0..60 {
        r.render(480);
    }
    assert!(!r.shared.snapshot().audio_detected);
}

#[test]
fn latency_is_reported_and_plausible() {
    let mut r = rig(exact_cfg());
    r.renderer
        .set_device_latency(std::time::Duration::from_millis(10));
    r.shared.set_capture_latency_us(5_000);
    for _ in 0..40 {
        r.push(&dc(480, 0.1));
        r.render(480);
    }
    let m = r.shared.snapshot();
    // ring (~10-20 ms) + DSP 10 ms + devices 15 ms
    assert!(
        m.latency_ms > 25.0 && m.latency_ms < 60.0,
        "{} ms",
        m.latency_ms
    );
    assert!((m.output_latency_ms - 10.0).abs() < 0.01);
    assert!((m.capture_latency_ms - 5.0).abs() < 0.01);
}

#[test]
fn different_sample_rates_keep_the_pitch_and_do_not_glitch() {
    let mut r = rig(RendererConfig {
        capture_rate: 44_100,
        output_rate: 48_000,
        ..exact_cfg()
    });
    let out = run_ticks(&mut r, 400, 441.0, 44_100.0, 440.0);
    let l: Vec<f32> = out.chunks_exact(2).map(|f| f[0]).collect();
    let tail = &l[l.len() / 2..];
    let crossings: Vec<usize> = (1..tail.len())
        .filter(|&i| tail[i - 1] < 0.0 && tail[i] >= 0.0)
        .collect();
    let f = (crossings.len() - 1) as f32 * 48_000.0
        / (crossings[crossings.len() - 1] - crossings[0]) as f32;
    assert!((f - 440.0).abs() < 1.0, "{f} Hz");
    assert_eq!(r.shared.underruns(), 0);
    assert_eq!(r.shared.overruns(), 0);
}

/// After the controller has converged (last third of the run) the ring must sit at the 20 ms
/// prefill target, not merely somewhere safe.
fn assert_settled_at_target(fills: &[f32]) {
    let settled = &fills[fills.len() * 2 / 3..];
    let mean = settled.iter().sum::<f32>() / settled.len() as f32;
    let (lo, hi) = settled
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
    assert!(
        (mean - 20.0).abs() < 2.5,
        "mean ring fill {mean} ms is not at the 20 ms target"
    );
    assert!(
        lo > 12.0 && hi < 30.0,
        "ring fill wandered between {lo} and {hi} ms"
    );
}

/// Simulates `seconds` of a capture clock that differs from the output clock by `ppm`.
fn drift_run(ppm: f64, compensate: bool, seconds: usize) -> (Rig, Vec<f32>) {
    let mut r = rig(RendererConfig {
        drift_compensation: compensate,
        ..exact_cfg()
    });
    let ticks = seconds * 100; // 10 ms per tick
    let in_per_tick = 480.0 * (1.0 + ppm * 1.0e-6);
    let mut fills = Vec::new();
    let mut owed = 0.0;
    let mut written = 0usize;
    for _ in 0..ticks {
        owed += in_per_tick;
        let n = owed.floor() as usize;
        owed -= n as f64;
        r.push(&sine(48_000.0, 300.0, written, n, 0.3));
        written += n;
        r.render(480);
        fills.push(r.shared.snapshot().ring_fill_ms);
    }
    (r, fills)
}

#[test]
fn drift_compensation_absorbs_a_fast_capture_clock() {
    let (r, fills) = drift_run(400.0, true, 120);
    let m = r.shared.snapshot();
    assert_eq!((m.underruns, m.overruns), (0, 0), "{m:?}");
    assert_settled_at_target(&fills);
}

#[test]
fn drift_compensation_absorbs_a_slow_capture_clock() {
    let (r, fills) = drift_run(-400.0, true, 120);
    let m = r.shared.snapshot();
    assert_eq!((m.underruns, m.overruns), (0, 0), "{m:?}");
    assert_settled_at_target(&fills);
}

#[test]
fn without_compensation_a_slow_capture_clock_eventually_underruns() {
    // Negative control: proves the test above is actually exercising the drift controller.
    let (r, _) = drift_run(-800.0, false, 90);
    assert!(r.shared.underruns() >= 1, "{:?}", r.shared.snapshot());
}

/// Underruns when capture stalls for `stall_ms` starting at `phase` ms into a callback period,
/// then delivers everything it missed in one burst (what a delayed capture thread does).
fn underruns_with_stall(cfg: RendererConfig, stall_ms: usize, phase: usize) -> u64 {
    let mut r = rig(cfg);
    let stall_start = 300 + phase;
    let mut backlog = 0usize;
    let mut written = 0usize;
    for ms in 0..600 {
        backlog += 48; // capture produces 48 frames per millisecond
        if !(stall_start..stall_start + stall_ms).contains(&ms) {
            r.push(&sine(48_000.0, 300.0, written, backlog, 0.3));
            written += backlog;
            backlog = 0;
        }
        if ms % 10 == 9 {
            r.render(480);
        }
    }
    r.shared.underruns()
}

/// The longest capture stall (in whole ms) that never causes an underrun at any phase.
fn max_tolerated_stall_ms(cfg: RendererConfig) -> usize {
    for stall in 0..=20 {
        if (0..10).any(|phase| underruns_with_stall(cfg, stall, phase) > 0) {
            return stall.saturating_sub(1);
        }
    }
    20
}

#[test]
fn tolerates_capture_stalls_of_almost_one_callback_period() {
    // With a 20 ms prefill and 10 ms callbacks the ring holds one callback period of slack. The
    // drift-compensating resampler must not eat into it: its small chunks cost only ~1.3 ms
    // (with 256-frame chunks it cost 5.3 ms and halved the margin, which showed up as
    // underruns in the 10-minute soak test).
    let plain = max_tolerated_stall_ms(exact_cfg());
    let nudged = max_tolerated_stall_ms(RendererConfig {
        drift_compensation: true,
        ..exact_cfg()
    });
    assert!(plain >= 9, "without the resampler: {plain} ms");
    assert!(
        nudged >= 8,
        "with the drift-compensating resampler: {nudged} ms"
    );
}
