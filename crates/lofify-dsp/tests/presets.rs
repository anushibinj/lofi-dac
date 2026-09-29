//! Every built-in preset must be safe, roughly level-matched and audibly different from Clean.

mod common;

use common::SR;
use lofify_dsp::chain::Chain;
use lofify_dsp::limiter::CEILING;
use lofify_dsp::params::Params;
use lofify_dsp::presets::builtin_presets;
use lofify_dsp::signals::{chords, peak, pink_noise, rms_dbfs, sine_sweep};

fn render(params: &Params, input: &[f32]) -> Vec<f32> {
    let mut chain = Chain::new(SR);
    chain.set_params(params);
    chain.snap();
    let mut out = input.to_vec();
    chain.process(&mut out);
    out
}

fn signals() -> Vec<(&'static str, Vec<f32>)> {
    vec![
        ("chords", chords(SR, 20.0, 0.5)),
        ("pink noise", pink_noise(SR, 10.0, -18.0, 1)),
        ("sine sweep", sine_sweep(SR, 10.0, 20.0, 20_000.0, 0.5)),
    ]
}

#[test]
fn presets_never_exceed_minus_one_dbfs() {
    for p in builtin_presets() {
        for (name, input) in signals() {
            let out = render(&p.params, &input);
            assert!(out.iter().all(|v| v.is_finite()), "{} / {name}", p.name);
            assert!(
                peak(&out) <= CEILING + 1.0e-6,
                "{} / {name}: peak {}",
                p.name,
                peak(&out)
            );
        }
    }
}

#[test]
fn presets_are_roughly_level_matched_on_music() {
    // Switching presets should not blast or vanish: within 3 dB of the input on musical material.
    let input = chords(SR, 20.0, 0.5);
    let skip = input.len() / 4;
    for p in builtin_presets() {
        let out = render(&p.params, &input);
        let delta = rms_dbfs(&out[skip..]) - rms_dbfs(&input[skip..]);
        assert!(
            delta.abs() < 3.0,
            "{} changes level by {delta:+.1} dB",
            p.name
        );
    }
}

fn difference_db(a: &[f32], b: &[f32]) -> f32 {
    let d: Vec<f32> = a.iter().zip(b).map(|(x, y)| x - y).collect();
    rms_dbfs(&d)
}

#[test]
fn every_effect_preset_is_audibly_different_from_clean() {
    let input = chords(SR, 10.0, 0.5);
    let clean = render(&Params::default(), &input);
    for p in builtin_presets().into_iter().filter(|p| p.name != "Clean") {
        let out = render(&p.params, &input);
        let diff = difference_db(&out, &clean);
        // The difference must be within about 25 dB of the signal itself, i.e. very audible.
        assert!(
            diff > rms_dbfs(&input) - 25.0,
            "{} differs from clean by only {diff:.1} dBFS",
            p.name
        );
    }
}

#[test]
fn presets_differ_from_each_other() {
    let input = chords(SR, 10.0, 0.5);
    let presets = builtin_presets();
    let outs: Vec<Vec<f32>> = presets.iter().map(|p| render(&p.params, &input)).collect();
    for i in 0..presets.len() {
        for j in (i + 1)..presets.len() {
            let diff = difference_db(&outs[i], &outs[j]);
            assert!(
                diff > rms_dbfs(&input) - 30.0,
                "{} and {} are nearly identical ({diff:.1} dBFS)",
                presets[i].name,
                presets[j].name
            );
        }
    }
}

#[test]
fn classic_lofi_muffles_high_frequencies_on_music() {
    // The spectrum above 6 kHz should be far quieter than in the source.
    use lofify_dsp::biquad::{Biquad, Coeffs};
    let input = pink_noise(SR, 8.0, -18.0, 9);
    let classic = builtin_presets().remove(0);
    let out = render(&classic.params, &input);
    let band_db = |x: &[f32]| {
        let mut hp = [
            Biquad::new(Coeffs::highpass(f64::from(SR), 6_000.0, 0.707)),
            Biquad::new(Coeffs::highpass(f64::from(SR), 6_000.0, 0.707)),
        ];
        let filtered: Vec<f32> = x
            .chunks_exact(2)
            .map(|f| {
                let y = hp[0].process(f[0]);
                hp[1].process(y)
            })
            .collect();
        rms_dbfs(&filtered[SR as usize..])
    };
    assert!(band_db(&input) - band_db(&out) > 15.0);
}
