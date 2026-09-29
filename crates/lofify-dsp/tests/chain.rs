//! Behavioural tests for the complete chain.

mod common;

use common::*;
use lofify_dsp::chain::{Chain, ChainConfig};
use lofify_dsp::limiter::CEILING;
use lofify_dsp::params::*;
use lofify_dsp::shared::SharedParams;
use std::time::Instant;

fn run(chain: &mut Chain, input: &[f32]) -> Vec<f32> {
    let mut out = input.to_vec();
    chain.process(&mut out);
    out
}

#[test]
fn latency_is_ten_milliseconds_by_default() {
    for sr in [44_100.0f32, 48_000.0, 96_000.0] {
        let c = Chain::new(sr);
        let expected = (0.010 * sr).round() as usize;
        assert_eq!(c.latency_samples(), expected, "sr {sr}");
    }
}

#[test]
fn clean_preset_is_transparent_apart_from_latency() {
    let mut chain = Chain::new(SR);
    let input = noise(20_000, 0.6, 1);
    let out = run(&mut chain, &input);
    let l = chain.latency_samples();
    for n in 0..input.len() / 2 - l {
        assert_eq!(out[2 * (n + l)], input[2 * n], "left n={n}");
        assert_eq!(out[2 * (n + l) + 1], input[2 * n + 1], "right n={n}");
    }
}

#[test]
fn bypass_equals_input_exactly() {
    let mut chain = Chain::new(SR);
    chain.set_params(&classic());
    chain.snap();
    chain.set_bypass(true);
    let input = noise(30_000, 1.0, 2); // full scale: dry path must not be limited either
    let out = run(&mut chain, &input);
    let l = chain.latency_samples();
    let fade = (0.020 * SR) as usize + 2;
    for m in fade..input.len() / 2 {
        assert_eq!(out[2 * m], input[2 * (m - l)], "left m={m}");
        assert_eq!(out[2 * m + 1], input[2 * (m - l) + 1], "right m={m}");
    }
}

#[test]
fn bypass_is_a_20ms_linear_crossfade() {
    // Wet path = dry x 2 (output +6.02 dB), so the crossfade is easy to see on DC.
    let mut p = Params::default();
    p.output.gain_db = 20.0 * 2.0f32.log10();
    let mut chain = Chain::new(SR);
    chain.set_params(&p);
    chain.snap();

    let dc = vec![0.3f32; 2 * 4_000];
    let settled = run(&mut chain, &dc);
    assert!((settled[2 * 3_999] - 0.6).abs() < 1.0e-5);

    chain.set_bypass(true);
    let out = run(&mut chain, &dc);
    let fade = (0.020 * SR) as usize;
    let at = |m: usize| out[2 * m];
    assert!(
        (at(fade / 2) - 0.45).abs() < 0.002,
        "midpoint {}",
        at(fade / 2)
    );
    assert!(
        at(fade + 2) == 0.3,
        "must land exactly on dry, got {}",
        at(fade + 2)
    );
    for m in 1..fade {
        assert!(at(m) <= at(m - 1) + 1.0e-6, "not monotonic at {m}");
    }
    // ...and back again
    chain.set_bypass(false);
    let back = run(&mut chain, &dc);
    assert!((back[2 * (fade / 2)] - 0.45).abs() < 0.002);
    assert!((back[2 * (fade + 2)] - 0.6).abs() < 1.0e-5);
}

#[test]
fn wet_dry_impulse_is_aligned_no_comb_filtering() {
    let l = Chain::new(SR).latency_samples();
    let at = 100;
    let mut impulse = vec![0.0f32; 2 * 4_000];
    impulse[2 * at] = 0.5;
    impulse[2 * at + 1] = 0.5;

    // 50/50 mix of a transparent wet path and the dry path: exactly one impulse, at `at + l`.
    let mut p = Params::default();
    p.mix.wet_dry = 0.5;
    let mut chain = Chain::new(SR);
    chain.set_params(&p);
    chain.snap();
    let out = run(&mut chain, &impulse);
    for (i, &v) in out.iter().enumerate() {
        let frame = i / 2;
        if frame == at + l {
            assert!((v - 0.5).abs() < 1.0e-6, "impulse amplitude {v}");
        } else {
            assert_eq!(v, 0.0, "stray energy at frame {frame}");
        }
    }

    // With a level change in the wet path the impulses still coincide: one peak, not two.
    p.output.gain_db = 6.0;
    let mut chain = Chain::new(SR);
    chain.set_params(&p);
    chain.snap();
    let out = run(&mut chain, &impulse);
    let nonzero: Vec<usize> = out
        .chunks_exact(2)
        .enumerate()
        .filter(|(_, f)| f[0] != 0.0)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(nonzero, vec![at + l]);
}

#[test]
fn mixed_output_has_no_comb_notches() {
    // Comb filtering from misaligned paths shows up as deep notches in the transfer function.
    let mut p = classic();
    // keep the wet path linear and time-invariant
    p.tape = TapeParams::default();
    p.saturation = SaturationParams::default();
    p.crush = CrushParams::default();
    p.filters = FilterParams::default();
    p.reverb.mix = 0.0;
    p.vinyl = VinylParams::default();
    p.output.gain_db = -3.0;
    p.mix.wet_dry = 0.5;
    let mut chain = Chain::new(SR);
    chain.set_params(&p);
    chain.snap();
    for hz in [500.0f32, 1_500.0, 3_000.0, 7_000.0, 12_000.0] {
        let input = sine(24_000, hz, 0.4);
        let out = run(&mut chain, &input);
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        let ratio = rms(&out[24_000..]) / rms(&input[24_000..]);
        // wet is -3 dB (0.708), dry 1.0, aligned: 0.5 + 0.354 = 0.854 at every frequency.
        assert!((ratio - 0.854).abs() < 0.01, "{hz} Hz: gain {ratio}");
    }
}

fn hostile_inputs(frames: usize) -> Vec<(&'static str, Vec<f32>)> {
    let square: Vec<f32> = (0..frames * 2)
        .map(|i| if (i / 2 / 48) % 2 == 0 { 1.0 } else { -1.0 })
        .collect();
    let mut poisoned = noise(frames, 0.5, 7);
    for i in (0..poisoned.len()).step_by(997) {
        poisoned[i] = match (i / 997) % 4 {
            0 => f32::NAN,
            1 => f32::INFINITY,
            2 => f32::NEG_INFINITY,
            _ => f32::MAX,
        };
    }
    vec![
        ("silence", vec![0.0; frames * 2]),
        ("dc full scale", vec![1.0; frames * 2]),
        ("full-scale square", square),
        ("full-scale noise", noise(frames, 1.0, 3)),
        ("overdriven noise", noise(frames, 30.0, 4)),
        ("nan/inf", poisoned),
        ("denormal-ish", vec![1.0e-38; frames * 2]),
    ]
}

#[test]
fn never_produces_nan_and_wet_path_respects_the_ceiling() {
    for (label, params) in [
        ("classic", classic()),
        ("extreme", extreme()),
        ("clean", Params::default()),
    ] {
        for (name, input) in hostile_inputs(48_000) {
            let mut chain = Chain::new(SR);
            chain.set_params(&params);
            let out = run(&mut chain, &input);
            for (i, &v) in out.iter().enumerate() {
                assert!(v.is_finite(), "{label}/{name}: non-finite at {i}");
                assert!(
                    v.abs() <= CEILING + 1.0e-6,
                    "{label}/{name}: {v} exceeds -1 dBFS at {i}"
                );
            }
        }
    }
}

#[test]
fn stays_finite_when_parameters_change_violently() {
    let mut chain = Chain::new(SR);
    let input = noise(48_000 * 2, 0.8, 11);
    let mut out = input.clone();
    let (wild, tame) = (extreme(), classic());
    for (k, block) in out.chunks_mut(512).enumerate() {
        chain.set_params(if k % 2 == 0 { &wild } else { &tame });
        chain.set_bypass(k % 7 == 0);
        chain.process(block);
    }
    assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 1.0 + 1.0e-6));
}

#[test]
fn parameter_steps_do_not_zipper() {
    // A hard 20 dB drop in output level must ramp, not jump.
    let mut chain = Chain::new(SR);
    let dc = vec![0.3f32; 2 * 2_000];
    run(&mut chain, &dc);
    let mut p = Params::default();
    p.output.gain_db = -20.0;
    chain.set_params(&p);
    // ~13 time constants of 10 ms are needed to settle to within 1e-3 of the target.
    let out = run(&mut chain, &vec![0.3f32; 2 * 8_000]);
    let max_step = out
        .chunks_exact(2)
        .zip(out.chunks_exact(2).skip(1))
        .map(|(a, b)| (b[0] - a[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(max_step < 0.002, "largest sample-to-sample step {max_step}");
    assert!((out[out.len() - 2] - 0.03).abs() < 0.001);
}

#[test]
fn runs_far_faster_than_real_time() {
    let seconds = 20;
    let mut chain = Chain::new(SR);
    chain.set_params(&classic());
    let mut buf = noise(48_000, 0.3, 5);
    let start = Instant::now();
    for _ in 0..seconds {
        chain.process(&mut buf);
    }
    let elapsed = start.elapsed().as_secs_f64();
    let realtime_factor = elapsed / f64::from(seconds);
    println!(
        "chain used {:.2} % of real time ({:.1}x faster)",
        realtime_factor * 100.0,
        1.0 / realtime_factor
    );
    assert!(
        realtime_factor < 0.25,
        "chain needs {:.1} % of real time",
        realtime_factor * 100.0
    );
}

#[test]
fn sync_from_shared_params_and_bypass() {
    let shared = SharedParams::default();
    let mut chain = Chain::new(SR);
    let mut p = classic();
    p.reverb.mix = 0.5;
    shared.store(&p);
    shared.set_bypass(true);
    chain.sync_from(&shared);
    assert_eq!(chain.params().reverb.mix, 0.5);
    assert!(chain.is_bypassed());
    shared.set_bypass(false);
    chain.sync_from(&shared);
    assert!(!chain.is_bypassed());
}

#[test]
fn frequency_response_reflects_parameters() {
    let mut chain = Chain::new(SR);
    let freqs = [20.0f32, 1_000.0, 10_000.0];
    let flat = chain.frequency_response(&freqs);
    assert!(flat.iter().all(|d| d.abs() < 0.01));
    chain.set_params(&classic());
    let r = chain.frequency_response(&freqs);
    assert!(r[1] - r[0] > 20.0, "HPF should cut 20 Hz: {r:?}");
    assert!(r[1] - r[2] > 20.0, "LPF should cut 10 kHz: {r:?}");
}

#[test]
fn odd_and_empty_buffers_are_handled() {
    let mut chain = Chain::new(SR);
    chain.process(&mut []);
    let mut odd = [0.1f32, 0.2, 0.3];
    chain.process(&mut odd);
    assert_eq!(odd[2], 0.3);
}

#[test]
fn custom_latency_is_honoured() {
    let cfg = ChainConfig {
        base_delay_ms: 3.0,
        seed: 1,
    };
    let chain = Chain::with_config(SR, cfg);
    assert_eq!(chain.latency_samples(), 144);
}

#[test]
fn invalid_sample_rates_fall_back_safely() {
    for sr in [0.0f32, -1.0, f32::NAN, f32::INFINITY] {
        let mut chain = Chain::new(sr);
        let mut buf = vec![0.1f32; 64];
        chain.process(&mut buf);
        assert!(buf.iter().all(|v| v.is_finite()));
    }
}
