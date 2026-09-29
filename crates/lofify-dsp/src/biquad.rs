//! RBJ-cookbook biquad filters (Direct Form II Transposed) with analytic frequency response.
//!
//! Coefficients are designed and stored in `f64`; low cutoff frequencies (a 20 Hz high-pass at
//! 48 kHz) lose too much precision in `f32`.

use crate::util::flush_denormal_f64;
use std::f64::consts::PI;

/// Normalised biquad coefficients (`a0 == 1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coeffs {
    /// Passes the signal through untouched.
    pub const IDENTITY: Coeffs = Coeffs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn normalized(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// Clamps design inputs into a numerically safe range.
    fn prepare(fs: f64, f0: f64, q: f64) -> (f64, f64, f64) {
        let fs = if fs.is_finite() {
            fs.max(1.0)
        } else {
            48_000.0
        };
        let f0 = if f0.is_finite() {
            f0.clamp(1.0, fs * 0.49)
        } else {
            1_000.0_f64.min(fs * 0.49)
        };
        let q = if q.is_finite() { q.max(0.05) } else { 0.707 };
        (f0, q, 2.0 * PI * f0 / fs)
    }

    /// Second-order low-pass.
    pub fn lowpass(fs: f64, f0: f64, q: f64) -> Self {
        let (_, q, w0) = Self::prepare(fs, f0, q);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        Self::normalized(
            (1.0 - cos) / 2.0,
            1.0 - cos,
            (1.0 - cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        )
    }

    /// Second-order high-pass.
    pub fn highpass(fs: f64, f0: f64, q: f64) -> Self {
        let (_, q, w0) = Self::prepare(fs, f0, q);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        Self::normalized(
            (1.0 + cos) / 2.0,
            -(1.0 + cos),
            (1.0 + cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        )
    }

    /// Band-pass with constant 0 dB peak gain.
    pub fn bandpass(fs: f64, f0: f64, q: f64) -> Self {
        let (_, q, w0) = Self::prepare(fs, f0, q);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        Self::normalized(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    /// Low shelf with shelf slope `S = 1`. At `gain_db == 0` this is exactly the identity.
    pub fn low_shelf(fs: f64, f0: f64, gain_db: f64) -> Self {
        let (_, _, w0) = Self::prepare(fs, f0, 0.7);
        let gain_db = if gain_db.is_finite() {
            gain_db.clamp(-60.0, 60.0)
        } else {
            0.0
        };
        let a = 10f64.powf(gain_db / 40.0);
        let (sin, cos) = w0.sin_cos();
        // S = 1  =>  alpha = sin(w0) / 2 * sqrt(2)
        let alpha = sin / 2.0 * std::f64::consts::SQRT_2;
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        Self::normalized(
            a * ((a + 1.0) - (a - 1.0) * cos + two_sqrt_a_alpha),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - two_sqrt_a_alpha),
            (a + 1.0) + (a - 1.0) * cos + two_sqrt_a_alpha,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - two_sqrt_a_alpha,
        )
    }

    /// Squared magnitude `|H(e^{jw})|^2` at `freq` Hz, evaluated analytically.
    pub fn magnitude_squared(&self, fs: f64, freq: f64) -> f64 {
        let w = 2.0 * PI * freq / fs.max(1.0);
        let (s1, c1) = w.sin_cos();
        let (s2, c2) = (2.0 * w).sin_cos();
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        let den = dr * dr + di * di;
        if den <= f64::MIN_POSITIVE {
            return 1.0;
        }
        (nr * nr + ni * ni) / den
    }

    /// Magnitude in decibels at `freq` Hz, floored at -200 dB.
    pub fn magnitude_db(&self, fs: f64, freq: f64) -> f64 {
        10.0 * self.magnitude_squared(fs, freq).max(1.0e-20).log10()
    }
}

/// A single biquad section, Direct Form II Transposed.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    c: Coeffs,
    z1: f64,
    z2: f64,
}

impl Biquad {
    /// Creates a section with the given coefficients and zeroed state.
    pub fn new(c: Coeffs) -> Self {
        Self {
            c,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Replaces the coefficients, keeping the filter state (safe for per-sample sweeps).
    #[inline]
    pub fn set_coeffs(&mut self, c: Coeffs) {
        self.c = c;
    }

    /// Current coefficients.
    pub fn coeffs(&self) -> Coeffs {
        self.c
    }

    /// Clears the delay elements.
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Filters one sample.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = f64::from(x);
        let y = self.c.b0 * x + self.z1;
        self.z1 = flush_denormal_f64(self.c.b1 * x - self.c.a1 * y + self.z2);
        self.z2 = flush_denormal_f64(self.c.b2 * x - self.c.a2 * y);
        y as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 48_000.0;

    /// Measures the steady-state gain in dB by filtering a sine.
    fn measured_db(c: Coeffs, freq: f64) -> f64 {
        let mut bq = Biquad::new(c);
        let n = FS as usize;
        let mut acc_in = 0.0f64;
        let mut acc_out = 0.0f64;
        for i in 0..n {
            let x = (2.0 * PI * freq * i as f64 / FS).sin() as f32;
            let y = bq.process(x);
            if i >= n / 2 {
                acc_in += f64::from(x) * f64::from(x);
                acc_out += f64::from(y) * f64::from(y);
            }
        }
        10.0 * (acc_out / acc_in).log10()
    }

    #[test]
    fn lowpass_is_minus_3db_at_cutoff_for_butterworth_q() {
        let c = Coeffs::lowpass(FS, 4_500.0, std::f64::consts::FRAC_1_SQRT_2);
        assert!((c.magnitude_db(FS, 4_500.0) + 3.0103).abs() < 0.01);
        assert!(c.magnitude_db(FS, 20.0).abs() < 0.01);
    }

    #[test]
    fn highpass_blocks_20hz() {
        let c = Coeffs::highpass(FS, 90.0, std::f64::consts::FRAC_1_SQRT_2);
        assert!(c.magnitude_db(FS, 20.0) < -20.0);
        assert!(c.magnitude_db(FS, 10_000.0).abs() < 0.05);
        assert!(measured_db(c, 20.0) < -20.0);
    }

    #[test]
    fn low_shelf_boosts_lows_and_leaves_highs() {
        let c = Coeffs::low_shelf(FS, 200.0, 2.0);
        assert!((c.magnitude_db(FS, 10.0) - 2.0).abs() < 0.05);
        assert!(c.magnitude_db(FS, 10_000.0).abs() < 0.05);
        // +1 dB at the shelf midpoint for S = 1 is half the gain
        assert!((c.magnitude_db(FS, 200.0) - 1.0).abs() < 0.1);
    }

    #[test]
    fn zero_gain_shelf_is_identity() {
        let c = Coeffs::low_shelf(FS, 200.0, 0.0);
        let mut bq = Biquad::new(c);
        for i in 0..1000 {
            let x = ((i * 37 % 200) as f32 - 100.0) / 100.0;
            assert_eq!(bq.process(x), x);
        }
    }

    #[test]
    fn analytic_response_matches_measurement() {
        let cases = [
            (Coeffs::lowpass(FS, 4_500.0, 0.7), 3_000.0),
            (Coeffs::lowpass(FS, 4_500.0, 0.7), 10_000.0),
            (Coeffs::highpass(FS, 90.0, 0.707), 60.0),
            (Coeffs::low_shelf(FS, 200.0, 6.0), 100.0),
            (Coeffs::bandpass(FS, 2_000.0, 1.0), 2_000.0),
        ];
        for (c, f) in cases {
            let a = c.magnitude_db(FS, f);
            let m = measured_db(c, f);
            assert!((a - m).abs() < 0.1, "f={f}: analytic {a} vs measured {m}");
        }
    }

    #[test]
    fn bandpass_peaks_at_0db() {
        let c = Coeffs::bandpass(FS, 1_000.0, 2.0);
        assert!(c.magnitude_db(FS, 1_000.0).abs() < 0.01);
    }

    #[test]
    fn extreme_designs_stay_finite() {
        for f0 in [0.0, 1.0, 20.0, 24_000.0, 1.0e9, f64::NAN] {
            for q in [0.0, 0.05, 0.7, 20.0] {
                let mut bq = Biquad::new(Coeffs::lowpass(FS, f0, q));
                for i in 0..2000 {
                    let y = bq.process(if i % 50 == 0 { 1.0 } else { 0.0 });
                    assert!(y.is_finite(), "f0={f0} q={q}");
                }
            }
        }
    }
}
