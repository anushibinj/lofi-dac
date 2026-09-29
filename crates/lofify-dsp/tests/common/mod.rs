//! Helpers shared by the DSP integration tests.
#![allow(dead_code)]

use lofify_dsp::params::*;
use lofify_dsp::rng::Rng;

pub const SR: f32 = 48_000.0;

/// The starting point for "Classic Lofi" from the brief, spelled out explicitly.
pub fn classic() -> Params {
    Params {
        tape: TapeParams {
            wow_cents: 8.0,
            wow_hz: 0.5,
            flutter_cents: 2.0,
            flutter_hz: 6.5,
            drift_cents: 1.5,
        },
        saturation: SaturationParams {
            drive: 1.5,
            makeup_db: 1.5,
        },
        crush: CrushParams {
            bits: 12.0,
            sample_rate_hz: 20_000.0,
            dither: true,
        },
        filters: FilterParams {
            hpf_enabled: true,
            hpf_hz: 90.0,
            lpf_enabled: true,
            lpf_hz: 4_500.0,
            lpf_q: 0.7,
            shelf_hz: 200.0,
            shelf_db: 2.0,
        },
        reverb: ReverbParams {
            room: 0.5,
            damping: 0.5,
            mix: 0.12,
        },
        vinyl: VinylParams {
            pops_per_sec: 6.0,
            pop_db: -34.0,
            hiss_db: -50.0,
        },
        ..Params::default()
    }
}

/// Every parameter pushed to the end of its range that is most likely to break something.
pub fn extreme() -> Params {
    Params {
        input: InputParams { trim_db: 12.0 },
        tape: TapeParams {
            wow_cents: 20.0,
            wow_hz: 0.3,
            flutter_cents: 10.0,
            flutter_hz: 2.0,
            drift_cents: 6.0,
        },
        saturation: SaturationParams {
            drive: 8.0,
            makeup_db: 12.0,
        },
        crush: CrushParams {
            bits: 2.0,
            sample_rate_hz: 1_000.0,
            dither: true,
        },
        filters: FilterParams {
            hpf_enabled: true,
            hpf_hz: 1_000.0,
            lpf_enabled: true,
            lpf_hz: 500.0,
            lpf_q: 4.0,
            shelf_hz: 800.0,
            shelf_db: 12.0,
        },
        reverb: ReverbParams {
            room: 1.0,
            damping: 0.0,
            mix: 1.0,
        },
        vinyl: VinylParams {
            pops_per_sec: 40.0,
            pop_db: -10.0,
            hiss_db: -30.0,
        },
        output: OutputParams { gain_db: 12.0 },
        mix: MixParams { wet_dry: 1.0 },
    }
}

/// Interleaved stereo white noise in `[-amp, amp)`.
pub fn noise(frames: usize, amp: f32, seed: u32) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    (0..frames * 2).map(|_| rng.bipolar() * amp).collect()
}

/// Interleaved stereo sine (same in both channels).
pub fn sine(frames: usize, hz: f32, amp: f32) -> Vec<f32> {
    (0..frames)
        .flat_map(|i| {
            let v = amp * (std::f32::consts::TAU * hz * i as f32 / SR).sin();
            [v, v]
        })
        .collect()
}
