//! Deterministic test signals for offline rendering and tests. All return interleaved stereo.

use crate::rng::Rng;
use crate::util::{db_to_lin, lin_to_db};
use std::f32::consts::TAU;

/// RMS level in dBFS of an interleaved buffer.
pub fn rms_dbfs(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -180.0;
    }
    let ms = samples
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum::<f64>()
        / samples.len() as f64;
    lin_to_db(ms.sqrt() as f32)
}

/// Peak absolute value.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |m, s| m.max(s.abs()))
}

fn frames(sample_rate: f32, seconds: f32) -> usize {
    (sample_rate * seconds).max(0.0) as usize
}

/// Exponential sine sweep from `f0` to `f1` Hz with 50 ms fades.
pub fn sine_sweep(sample_rate: f32, seconds: f32, f0: f32, f1: f32, amp: f32) -> Vec<f32> {
    let n = frames(sample_rate, seconds);
    let t_total = seconds.max(1.0e-3);
    let k = (f1 / f0).ln();
    let fade = (0.05 * sample_rate) as usize;
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f32 / sample_rate;
        let phase = TAU * f0 * t_total / k * ((t / t_total * k).exp() - 1.0);
        let edge = i.min(n - 1 - i);
        let env = if edge < fade {
            edge as f32 / fade as f32
        } else {
            1.0
        };
        let v = amp * env * phase.sin();
        out.push(v);
        out.push(v);
    }
    out
}

/// Pink noise (-3 dB/octave, independent channels) scaled to `rms_db` dBFS.
pub fn pink_noise(sample_rate: f32, seconds: f32, rms_db: f32, seed: u32) -> Vec<f32> {
    let n = frames(sample_rate, seconds);
    let mut out = vec![0.0f32; n * 2];
    for ch in 0..2 {
        let mut rng = Rng::new(seed.wrapping_add(ch as u32 * 7919));
        // Paul Kellet's economy pink filter
        let (mut b0, mut b1, mut b2) = (0.0f32, 0.0f32, 0.0f32);
        for i in 0..n {
            let white = rng.bipolar();
            b0 = 0.99765 * b0 + white * 0.099_046;
            b1 = 0.963 * b1 + white * 0.296_516_4;
            b2 = 0.57 * b2 + white * 1.052_691_3;
            out[i * 2 + ch] = b0 + b1 + b2 + white * 0.1848;
        }
    }
    let gain = db_to_lin(rms_db - rms_dbfs(&out));
    out.iter_mut().for_each(|s| *s *= gain);
    out
}

/// A soft plucked-chord loop (Dm9, G7, Cmaj9, Am9) with bass and a light hat, peaking at `amp`.
/// Music-like material is much better than noise for judging lofi effects by ear.
pub fn chords(sample_rate: f32, seconds: f32, amp: f32) -> Vec<f32> {
    const BPM: f32 = 84.0;
    // (bass, [chord tones]) as MIDI note numbers
    const PROGRESSION: [(u8, [u8; 4]); 4] = [
        (38, [53, 57, 60, 64]),
        (43, [53, 59, 62, 64]),
        (36, [52, 55, 59, 62]),
        (45, [48, 52, 55, 59]),
    ];
    let n = frames(sample_rate, seconds);
    let mut buf = vec![0.0f32; n * 2];
    let eighth = 60.0 / BPM / 2.0;
    let mut rng = Rng::new(31337);

    let midi = |m: u8| 440.0 * ((f32::from(m) - 69.0) / 12.0).exp2();
    let note = |buf: &mut [f32], start_s: f32, freq: f32, level: f32, tau: f32, pan: f32| {
        let start = (start_s * sample_rate) as usize;
        let len = (tau * 6.0 * sample_rate) as usize;
        let (gl, gr) = ((1.0 - pan).sqrt(), (1.0 + pan).sqrt());
        for j in 0..len {
            let i = start + j;
            if i >= n {
                break;
            }
            let t = j as f32 / sample_rate;
            let env = (1.0 - (-t / 0.008).exp()) * (-t / tau).exp();
            let ph = TAU * freq * t;
            let v = level * env * (ph.sin() + 0.4 * (2.0 * ph).sin() + 0.15 * (3.0 * ph).sin());
            buf[i * 2] += v * gl * 0.7071;
            buf[i * 2 + 1] += v * gr * 0.7071;
        }
    };

    let total_eighths = (seconds / eighth).ceil() as usize + 1;
    for step in 0..total_eighths {
        let t = step as f32 * eighth;
        let (bass, tones) = PROGRESSION[(step / 16) % PROGRESSION.len()];
        let pos = step % 16;
        if pos % 8 == 0 {
            note(&mut buf, t, midi(bass), 0.5, 0.7, 0.0);
        }
        // Arpeggio: every eighth note steps through the chord tones, alternating sides.
        let tone = tones[[0usize, 1, 2, 3, 2, 1, 3, 2][pos % 8]];
        let pan = if pos % 2 == 0 { -0.35 } else { 0.35 };
        note(&mut buf, t, midi(tone), 0.22, 0.45, pan);
        // Off-beat hat: a burst of noise
        if pos % 2 == 1 {
            let start = (t * sample_rate) as usize;
            let len = (0.04 * sample_rate) as usize;
            let mut prev = 0.0f32;
            for j in 0..len {
                let i = start + j;
                if i >= n {
                    break;
                }
                let white = rng.bipolar();
                let hp = white - prev; // crude high-pass
                prev = white;
                let v = hp * 0.05 * (-(j as f32) / (0.008 * sample_rate)).exp();
                buf[i * 2] += v;
                buf[i * 2 + 1] += v;
            }
        }
    }
    let p = peak(&buf);
    if p > 0.0 {
        let g = amp / p;
        buf.iter_mut().for_each(|s| *s *= g);
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    #[test]
    fn lengths_are_stereo_frames() {
        assert_eq!(sine_sweep(SR, 1.0, 20.0, 20_000.0, 0.5).len(), 96_000);
        assert_eq!(pink_noise(SR, 0.5, -18.0, 1).len(), 48_000);
        assert_eq!(chords(SR, 2.0, 0.5).len(), 192_000);
    }

    #[test]
    fn sweep_has_requested_peak_and_fades() {
        let s = sine_sweep(SR, 2.0, 20.0, 20_000.0, 0.5);
        let p = peak(&s);
        assert!(p <= 0.5 + 1.0e-6 && p > 0.49, "peak {p}");
        assert_eq!(s[0], 0.0);
        assert!(s[s.len() - 2].abs() < 1.0e-3);
    }

    #[test]
    fn sweep_frequency_rises() {
        let s = sine_sweep(SR, 4.0, 100.0, 10_000.0, 0.5);
        let crossings = |from: usize, to: usize| {
            s[from * 2..to * 2]
                .chunks_exact(2)
                .zip(s[from * 2 + 2..to * 2].chunks_exact(2))
                .filter(|(a, b)| a[0] <= 0.0 && b[0] > 0.0)
                .count()
        };
        let early = crossings(0, 48_000); // first second
        let late = crossings(144_000, 192_000); // last second
        assert!(late > early * 5, "early {early} late {late}");
    }

    #[test]
    fn pink_noise_hits_requested_rms() {
        let p = pink_noise(SR, 5.0, -18.0, 3);
        assert!((rms_dbfs(&p) + 18.0).abs() < 0.01);
    }

    #[test]
    fn pink_noise_has_equal_energy_per_octave() {
        use crate::biquad::{Biquad, Coeffs};
        let p = pink_noise(SR, 10.0, -20.0, 5);
        let band = |lo: f64, hi: f64| {
            let fs = f64::from(SR);
            let mut hp = [
                Biquad::new(Coeffs::highpass(fs, lo, 0.707)),
                Biquad::new(Coeffs::highpass(fs, lo, 0.707)),
            ];
            let mut lp = [
                Biquad::new(Coeffs::lowpass(fs, hi, 0.707)),
                Biquad::new(Coeffs::lowpass(fs, hi, 0.707)),
            ];
            let mut acc = 0.0f64;
            for x in p.chunks_exact(2).map(|f| f[0]) {
                let y = hp[0].process(x);
                let y = hp[1].process(y);
                let y = lp[0].process(y);
                let y = lp[1].process(y);
                acc += f64::from(y) * f64::from(y);
            }
            10.0 * acc.log10()
        };
        let octaves = [
            band(125.0, 250.0),
            band(250.0, 500.0),
            band(500.0, 1_000.0),
            band(1_000.0, 2_000.0),
        ];
        let (min, max) = octaves
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), &x| (a.min(x), b.max(x)));
        assert!(max - min < 3.0, "octave energies {octaves:?}");
    }

    #[test]
    fn chords_are_musical_and_deterministic() {
        let a = chords(SR, 6.0, 0.5);
        assert!((peak(&a) - 0.5).abs() < 1.0e-4);
        assert!(rms_dbfs(&a) > -30.0);
        assert_eq!(a, chords(SR, 6.0, 0.5));
        // stereo: channels differ because notes are panned
        assert!(a.chunks_exact(2).any(|f| (f[0] - f[1]).abs() > 0.01));
    }

    #[test]
    fn zero_length_is_empty() {
        assert!(sine_sweep(SR, 0.0, 20.0, 2_000.0, 0.5).is_empty());
        assert!(pink_noise(SR, 0.0, -20.0, 1).is_empty());
        assert!(chords(SR, 0.0, 0.5).is_empty());
    }
}
