//! Vinyl crackle and hiss.
//!
//! * **Pops**: a Poisson process (`pops_per_sec`) with exponentially distributed amplitude and
//!   random sign. Each pop is a unit impulse pushed through a band-pass, which rings and decays
//!   like a real click. `pop_db` is the *mean peak amplitude* of a pop.
//! * **Ticks**: a denser, quieter, brighter Poisson process (six times the pop rate, 15 dB down)
//!   that gives the "dusty" crackle texture.
//! * **Hiss**: uniform white noise band-limited to roughly 400 Hz - 9 kHz. `hiss_db` is its RMS
//!   level, calibrated analytically from the shaping filters.
//!
//! Randomness comes from a xorshift PRNG, so renders are reproducible for a given seed.

use crate::biquad::{Biquad, Coeffs};
use crate::rng::Rng;
use crate::util::{Smoothed, db_to_lin};
use std::f64::consts::FRAC_1_SQRT_2;

const POP_CENTER_HZ: f64 = 1_600.0;
const POP_Q: f64 = 0.7;
const TICK_CENTER_HZ: f64 = 3_500.0;
const TICK_Q: f64 = 1.0;
const TICK_RATE_MULTIPLIER: f32 = 6.0;
const TICK_LEVEL: f32 = 0.18;
/// Cap on the exponential amplitude distribution so a rare huge pop cannot blast the listener.
const AMPLITUDE_CAP: f32 = 5.0;
const HISS_HPF_HZ: f64 = 400.0;
const HISS_LPF_HZ: f64 = 9_000.0;
/// At or below this level (dB) the hiss is fully off.
const HISS_OFF_DB: f32 = -89.9;

/// Peak absolute value of the impulse response of `c`.
fn impulse_peak(c: Coeffs) -> f32 {
    let mut bq = Biquad::new(c);
    let mut peak = 0.0f32;
    for i in 0..1024 {
        peak = peak.max(bq.process(if i == 0 { 1.0 } else { 0.0 }).abs());
    }
    peak.max(1.0e-6)
}

/// Mean of `|H|^2` over 0..Nyquist: the power gain of the filters for white noise.
fn noise_power_gain(fs: f64, filters: &[Coeffs]) -> f64 {
    let n = 2048;
    let sum: f64 = (0..n)
        .map(|i| {
            let f = (f64::from(i) + 0.5) / f64::from(n) * fs / 2.0;
            filters
                .iter()
                .map(|c| c.magnitude_squared(fs, f))
                .product::<f64>()
        })
        .sum();
    sum / f64::from(n)
}

/// Stereo crackle and hiss generator.
#[derive(Debug, Clone)]
pub struct Vinyl {
    sample_rate: f32,
    rng: Rng,
    pops_per_sec: Smoothed,
    pop_amp: Smoothed,
    hiss_amp: Smoothed,
    pop_shape: [Biquad; 2],
    tick_shape: [Biquad; 2],
    hiss_hp: [Biquad; 2],
    hiss_lp: [Biquad; 2],
    pop_norm: f32,
    tick_norm: f32,
    hiss_norm: f32,
    pops_fired: u64,
}

impl Vinyl {
    /// Creates a silent generator.
    pub fn new(sample_rate: f32, seed: u32) -> Self {
        let fs = f64::from(sample_rate);
        let pop = Coeffs::bandpass(fs, POP_CENTER_HZ, POP_Q);
        let tick = Coeffs::bandpass(fs, TICK_CENTER_HZ, TICK_Q);
        let hp = Coeffs::highpass(fs, HISS_HPF_HZ, FRAC_1_SQRT_2);
        let lp = Coeffs::lowpass(fs, HISS_LPF_HZ, FRAC_1_SQRT_2);
        let power = noise_power_gain(fs, &[hp, lp]).max(1.0e-9);
        Self {
            sample_rate,
            rng: Rng::new(seed),
            pops_per_sec: Smoothed::with_default_time(0.0, sample_rate),
            pop_amp: Smoothed::with_default_time(0.0, sample_rate),
            hiss_amp: Smoothed::with_default_time(0.0, sample_rate),
            pop_shape: [Biquad::new(pop); 2],
            tick_shape: [Biquad::new(tick); 2],
            hiss_hp: [Biquad::new(hp); 2],
            hiss_lp: [Biquad::new(lp); 2],
            pop_norm: 1.0 / impulse_peak(pop),
            tick_norm: 1.0 / impulse_peak(tick),
            // uniform [-1, 1) has variance 1/3
            hiss_norm: (3.0 / power).sqrt() as f32,
            pops_fired: 0,
        }
    }

    /// Sets targets. `pop_db` is the mean pop peak in dBFS, `hiss_db` the hiss RMS in dBFS.
    pub fn set_params(&mut self, pops_per_sec: f32, pop_db: f32, hiss_db: f32) {
        self.pops_per_sec.set_target(pops_per_sec.max(0.0));
        self.pop_amp.set_target(db_to_lin(pop_db));
        self.hiss_amp.set_target(if hiss_db <= HISS_OFF_DB {
            0.0
        } else {
            db_to_lin(hiss_db)
        });
    }

    /// Jumps to the current targets.
    pub fn snap(&mut self) {
        for s in [
            &mut self.pops_per_sec,
            &mut self.pop_amp,
            &mut self.hiss_amp,
        ] {
            let t = s.target();
            s.snap(t);
        }
    }

    /// Number of pops triggered so far (diagnostics and tests).
    pub fn pops_fired(&self) -> u64 {
        self.pops_fired
    }

    /// Generates one stereo frame of noise to be added to the signal.
    #[inline]
    pub fn process(&mut self) -> (f32, f32) {
        let rate = self.pops_per_sec.tick();
        let pop_amp = self.pop_amp.tick();
        let hiss_amp = self.hiss_amp.tick();

        let mut pop_in = 0.0;
        let mut tick_in = 0.0;
        if rate > 0.0 {
            let p = rate / self.sample_rate;
            if self.rng.uniform() < p {
                pop_in =
                    self.rng.sign() * self.rng.exponential(AMPLITUDE_CAP) * pop_amp * self.pop_norm;
                self.pops_fired += 1;
            }
            if self.rng.uniform() < p * TICK_RATE_MULTIPLIER {
                tick_in = self.rng.sign()
                    * self.rng.exponential(AMPLITUDE_CAP)
                    * pop_amp
                    * TICK_LEVEL
                    * self.tick_norm;
            }
        }
        // The right channel hears each click slightly quieter, like an off-centre stylus.
        let right_gain = 0.7 + 0.3 * self.rng.uniform();

        let hiss = hiss_amp * self.hiss_norm;
        let (mut out_l, mut out_r) = (0.0, 0.0);
        for (ch, out) in [&mut out_l, &mut out_r].into_iter().enumerate() {
            let g = if ch == 0 { 1.0 } else { right_gain };
            let clicks =
                self.pop_shape[ch].process(pop_in * g) + self.tick_shape[ch].process(tick_in * g);
            let noise = if hiss > 0.0 {
                self.hiss_lp[ch].process(self.hiss_hp[ch].process(self.rng.bipolar() * hiss))
            } else {
                0.0
            };
            *out = clicks + noise;
        }
        (out_l, out_r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    fn vinyl(pops: f32, pop_db: f32, hiss_db: f32) -> Vinyl {
        let mut v = Vinyl::new(SR, 2024);
        v.set_params(pops, pop_db, hiss_db);
        v.snap();
        v
    }

    fn rms_db(v: &[f32]) -> f32 {
        let ms = v.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>() / v.len() as f64;
        (10.0 * ms.log10()) as f32
    }

    #[test]
    fn silent_when_everything_is_off() {
        let mut v = vinyl(0.0, -34.0, -90.0);
        for _ in 0..10_000 {
            assert_eq!(v.process(), (0.0, 0.0));
        }
    }

    #[test]
    fn hiss_rms_matches_requested_level() {
        for target in [-50.0f32, -40.0] {
            let mut v = vinyl(0.0, -34.0, target);
            let out: Vec<f32> = (0..SR as usize * 10).map(|_| v.process().0).collect();
            let measured = rms_db(&out[SR as usize..]);
            assert!(
                (measured - target).abs() < 1.0,
                "target {target} measured {measured}"
            );
        }
    }

    #[test]
    fn hiss_is_band_limited() {
        // Power below 100 Hz must be far below power in the mid band.
        let mut v = vinyl(0.0, -34.0, -40.0);
        let out: Vec<f32> = (0..SR as usize * 4).map(|_| v.process().0).collect();
        let goertzel = |f: f32| {
            let w = 2.0 * std::f32::consts::PI * f / SR;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, x) in out.iter().enumerate() {
                re += f64::from(*x) * f64::from((w * i as f32).cos());
                im += f64::from(*x) * f64::from((w * i as f32).sin());
            }
            (re * re + im * im).sqrt()
        };
        let low: f64 = [30.0, 50.0, 70.0].iter().map(|&f| goertzel(f)).sum();
        let mid: f64 = [1_000.0, 2_000.0, 3_000.0]
            .iter()
            .map(|&f| goertzel(f))
            .sum();
        assert!(low < mid * 0.2, "low {low} mid {mid}");
    }

    #[test]
    fn pop_rate_is_poisson_with_requested_mean() {
        let mut v = vinyl(6.0, -34.0, -90.0);
        let seconds = 100;
        for _ in 0..SR as usize * seconds {
            v.process();
        }
        let n = v.pops_fired() as f64;
        // mean 600, sigma ~24.5
        assert!((n - 600.0).abs() < 4.0 * 24.5, "{n} pops in {seconds} s");
    }

    #[test]
    fn pop_peaks_follow_requested_level() {
        let level = db_to_lin(-34.0);
        let mut v = vinyl(6.0, -34.0, -90.0);
        let out: Vec<f32> = (0..SR as usize * 60).map(|_| v.process().0).collect();
        let peak = out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        // Peaks are exponentially distributed with mean `level`, capped at 5x.
        assert!(
            peak > 2.5 * level && peak < 5.6 * level,
            "peak {} vs level {level}",
            peak
        );
        assert!(peak < 0.2, "-34 dBFS pops must stay well below full scale");
    }

    #[test]
    fn both_signs_occur() {
        let mut v = vinyl(20.0, -30.0, -90.0);
        let out: Vec<f32> = (0..SR as usize * 5).map(|_| v.process().0).collect();
        assert!(out.iter().any(|&x| x > 0.005));
        assert!(out.iter().any(|&x| x < -0.005));
    }

    #[test]
    fn crackle_has_no_dc_offset() {
        let mut v = vinyl(20.0, -30.0, -90.0);
        let n = SR as usize * 20;
        let mean: f64 = (0..n).map(|_| f64::from(v.process().0)).sum::<f64>() / n as f64;
        assert!(mean.abs() < 1.0e-4, "dc {mean}");
    }

    #[test]
    fn channels_differ() {
        let mut v = vinyl(20.0, -30.0, -50.0);
        let mut differ = 0;
        for _ in 0..10_000 {
            let (l, r) = v.process();
            if (l - r).abs() > 1.0e-6 {
                differ += 1;
            }
        }
        assert!(differ > 9_000);
    }

    #[test]
    fn same_seed_is_reproducible() {
        let mut a = vinyl(6.0, -34.0, -50.0);
        let mut b = vinyl(6.0, -34.0, -50.0);
        for _ in 0..5_000 {
            assert_eq!(a.process(), b.process());
        }
    }
}
