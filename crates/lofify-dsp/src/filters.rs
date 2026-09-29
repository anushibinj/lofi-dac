//! High-pass, low-pass ("the muffle") and low-shelf ("warmth") filters.
//!
//! * HPF: one RBJ section, Butterworth Q (12 dB/oct).
//! * LPF: two cascaded RBJ sections (24 dB/oct). The user's Q applies to the first section and
//!   the second is Butterworth. A single 2nd-order section at 4.5 kHz only removes about 14 dB
//!   at 10 kHz; the cascade removes about 28 dB, which is what the "muffle" needs.
//! * Low shelf: one RBJ section.
//!
//! Enabling or disabling a filter crossfades (10 ms) between the filtered and unfiltered signal
//! while the filter keeps running, so toggling never clicks.

use crate::biquad::{Biquad, Coeffs};
use crate::params::FilterParams;
use crate::util::Smoothed;

/// Butterworth Q for the fixed sections.
const BUTTERWORTH_Q: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// Designed coefficients for the whole bank.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterCoeffs {
    pub hpf: Coeffs,
    pub lpf_a: Coeffs,
    pub lpf_b: Coeffs,
    pub shelf: Coeffs,
}

/// Designs every section from raw values (used both at runtime and for the plot).
pub fn design(
    sample_rate: f32,
    hpf_hz: f32,
    lpf_hz: f32,
    lpf_q: f32,
    shelf_hz: f32,
    shelf_db: f32,
) -> FilterCoeffs {
    let fs = f64::from(sample_rate);
    FilterCoeffs {
        hpf: Coeffs::highpass(fs, f64::from(hpf_hz), BUTTERWORTH_Q),
        lpf_a: Coeffs::lowpass(fs, f64::from(lpf_hz), f64::from(lpf_q)),
        lpf_b: Coeffs::lowpass(fs, f64::from(lpf_hz), BUTTERWORTH_Q),
        shelf: Coeffs::low_shelf(fs, f64::from(shelf_hz), f64::from(shelf_db)),
    }
}

/// Combined magnitude response in dB of the filter chain described by `p`, evaluated analytically
/// from the biquad coefficients. Disabled filters contribute 0 dB.
pub fn frequency_response(p: &FilterParams, sample_rate: f32, freqs: &[f32]) -> Vec<f32> {
    let c = design(
        sample_rate,
        p.hpf_hz,
        p.lpf_hz,
        p.lpf_q,
        p.shelf_hz,
        p.shelf_db,
    );
    let fs = f64::from(sample_rate);
    freqs
        .iter()
        .map(|&f| {
            let f = f64::from(f);
            let mut db = c.shelf.magnitude_db(fs, f);
            if p.hpf_enabled {
                db += c.hpf.magnitude_db(fs, f);
            }
            if p.lpf_enabled {
                db += c.lpf_a.magnitude_db(fs, f) + c.lpf_b.magnitude_db(fs, f);
            }
            db as f32
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct Channel {
    hpf: Biquad,
    lpf_a: Biquad,
    lpf_b: Biquad,
    shelf: Biquad,
}

impl Channel {
    fn new(c: &FilterCoeffs) -> Self {
        Self {
            hpf: Biquad::new(c.hpf),
            lpf_a: Biquad::new(c.lpf_a),
            lpf_b: Biquad::new(c.lpf_b),
            shelf: Biquad::new(c.shelf),
        }
    }

    fn set(&mut self, c: &FilterCoeffs) {
        self.hpf.set_coeffs(c.hpf);
        self.lpf_a.set_coeffs(c.lpf_a);
        self.lpf_b.set_coeffs(c.lpf_b);
        self.shelf.set_coeffs(c.shelf);
    }

    #[inline]
    fn process(&mut self, x: f32, hpf_amt: f32, lpf_amt: f32) -> f32 {
        let hp = self.hpf.process(x);
        let x = x + (hp - x) * hpf_amt;
        let lp = self.lpf_b.process(self.lpf_a.process(x));
        let x = x + (lp - x) * lpf_amt;
        self.shelf.process(x)
    }
}

/// Stereo filter bank with per-sample smoothed parameters.
#[derive(Debug, Clone)]
pub struct FilterBank {
    sample_rate: f32,
    hpf_amt: Smoothed,
    lpf_amt: Smoothed,
    hpf_hz: Smoothed,
    lpf_hz: Smoothed,
    lpf_q: Smoothed,
    shelf_hz: Smoothed,
    shelf_db: Smoothed,
    last_key: [f32; 5],
    left: Channel,
    right: Channel,
}

impl FilterBank {
    /// Creates a transparent bank (both filters off, shelf at 0 dB).
    pub fn new(sample_rate: f32) -> Self {
        let p = FilterParams::default();
        let c = design(
            sample_rate,
            p.hpf_hz,
            p.lpf_hz,
            p.lpf_q,
            p.shelf_hz,
            p.shelf_db,
        );
        let s = |v| Smoothed::with_default_time(v, sample_rate);
        Self {
            sample_rate,
            hpf_amt: s(0.0),
            lpf_amt: s(0.0),
            hpf_hz: s(p.hpf_hz),
            lpf_hz: s(p.lpf_hz),
            lpf_q: s(p.lpf_q),
            shelf_hz: s(p.shelf_hz),
            shelf_db: s(p.shelf_db),
            last_key: [p.hpf_hz, p.lpf_hz, p.lpf_q, p.shelf_hz, p.shelf_db],
            left: Channel::new(&c),
            right: Channel::new(&c),
        }
    }

    /// Sets targets.
    pub fn set_params(&mut self, p: &FilterParams) {
        self.hpf_amt
            .set_target(if p.hpf_enabled { 1.0 } else { 0.0 });
        self.lpf_amt
            .set_target(if p.lpf_enabled { 1.0 } else { 0.0 });
        self.hpf_hz.set_target(p.hpf_hz);
        self.lpf_hz.set_target(p.lpf_hz);
        self.lpf_q.set_target(p.lpf_q);
        self.shelf_hz.set_target(p.shelf_hz);
        self.shelf_db.set_target(p.shelf_db);
    }

    /// Jumps to the current targets (offline rendering and tests).
    pub fn snap(&mut self) {
        for s in [
            &mut self.hpf_amt,
            &mut self.lpf_amt,
            &mut self.hpf_hz,
            &mut self.lpf_hz,
            &mut self.lpf_q,
            &mut self.shelf_hz,
            &mut self.shelf_db,
        ] {
            let t = s.target();
            s.snap(t);
        }
        self.redesign();
    }

    fn redesign(&mut self) {
        let key = [
            self.hpf_hz.value(),
            self.lpf_hz.value(),
            self.lpf_q.value(),
            self.shelf_hz.value(),
            self.shelf_db.value(),
        ];
        self.last_key = key;
        let c = design(self.sample_rate, key[0], key[1], key[2], key[3], key[4]);
        self.left.set(&c);
        self.right.set(&c);
    }

    /// Processes one stereo frame.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let hpf_amt = self.hpf_amt.tick();
        let lpf_amt = self.lpf_amt.tick();
        let key = [
            self.hpf_hz.tick(),
            self.lpf_hz.tick(),
            self.lpf_q.tick(),
            self.shelf_hz.tick(),
            self.shelf_db.tick(),
        ];
        // Redesigning costs several trig calls, so only do it while a parameter is moving.
        if key != self.last_key {
            self.last_key = key;
            let c = design(self.sample_rate, key[0], key[1], key[2], key[3], key[4]);
            self.left.set(&c);
            self.right.set(&c);
        }
        (
            self.left.process(l, hpf_amt, lpf_amt),
            self.right.process(r, hpf_amt, lpf_amt),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;

    fn classic() -> FilterParams {
        FilterParams {
            hpf_enabled: true,
            hpf_hz: 90.0,
            lpf_enabled: true,
            lpf_hz: 4_500.0,
            lpf_q: 0.7,
            shelf_hz: 200.0,
            shelf_db: 2.0,
        }
    }

    fn bank(p: &FilterParams) -> FilterBank {
        let mut b = FilterBank::new(SR);
        b.set_params(p);
        b.snap();
        b
    }

    /// Steady-state gain in dB for a sine at `freq`.
    fn measured_db(b: &mut FilterBank, freq: f32) -> f32 {
        let n = SR as usize * 2;
        let (mut i_acc, mut o_acc) = (0.0f64, 0.0f64);
        for i in 0..n {
            let x = (TAU * freq * i as f32 / SR).sin() * 0.5;
            let (y, _) = b.process(x, x);
            if i >= n / 2 {
                i_acc += f64::from(x * x);
                o_acc += f64::from(y * y);
            }
        }
        (10.0 * (o_acc / i_acc).log10()) as f32
    }

    #[test]
    fn lpf_attenuates_10khz_by_more_than_20db() {
        let p = classic();
        let mut b = bank(&p);
        let pass = measured_db(&mut b, 1_000.0);
        let mut b = bank(&p);
        let stop = measured_db(&mut b, 10_000.0);
        assert!(pass - stop > 20.0, "passband {pass} dB, 10 kHz {stop} dB");
        let analytic = frequency_response(&p, SR, &[1_000.0, 10_000.0]);
        assert!(analytic[0] - analytic[1] > 20.0);
    }

    #[test]
    fn hpf_blocks_20hz() {
        let p = classic();
        let mut b = bank(&p);
        let pass = measured_db(&mut b, 1_000.0);
        let mut b = bank(&p);
        let stop = measured_db(&mut b, 20.0);
        // The low shelf adds up to +2 dB below 200 Hz, so compare against the passband.
        assert!(pass - stop > 20.0, "passband {pass} dB, 20 Hz {stop} dB");
    }

    #[test]
    fn analytic_response_matches_runtime_bank() {
        let p = classic();
        for f in [30.0f32, 90.0, 200.0, 1_000.0, 4_500.0, 8_000.0] {
            let mut b = bank(&p);
            let m = measured_db(&mut b, f);
            let a = frequency_response(&p, SR, &[f])[0];
            assert!((m - a).abs() < 0.2, "f={f}: measured {m}, analytic {a}");
        }
    }

    #[test]
    fn disabled_bank_is_bit_exact_identity() {
        let mut p = classic();
        p.hpf_enabled = false;
        p.lpf_enabled = false;
        p.shelf_db = 0.0;
        let mut b = bank(&p);
        for i in 0..4000 {
            let x = (i as f32 * 0.31).sin() * 0.8;
            assert_eq!(b.process(x, -x), (x, -x));
        }
    }

    #[test]
    fn disabled_filters_contribute_zero_db() {
        let mut p = classic();
        p.hpf_enabled = false;
        p.lpf_enabled = false;
        p.shelf_db = 0.0;
        for db in frequency_response(&p, SR, &[20.0, 1_000.0, 10_000.0]) {
            assert!(db.abs() < 1.0e-3);
        }
    }

    #[test]
    fn toggling_a_filter_does_not_click() {
        // Run a steady sine with the LPF off, switch it on and check the output stays smooth.
        let mut p = classic();
        p.lpf_enabled = false;
        let mut b = bank(&p);
        let f = 500.0;
        let mut prev = 0.0f32;
        let mut max_jump = 0.0f32;
        for i in 0..SR as usize {
            if i == SR as usize / 2 {
                p.lpf_enabled = true;
                b.set_params(&p);
            }
            let x = (TAU * f * i as f32 / SR).sin() * 0.5;
            let (y, _) = b.process(x, x);
            if i > 4800 {
                max_jump = max_jump.max((y - prev).abs());
            }
            prev = y;
        }
        // A 500 Hz sine at 0.5 amplitude moves at most 0.5 * 2 pi 500 / 48000 = 0.033 per sample.
        assert!(max_jump < 0.06, "max sample-to-sample jump {max_jump}");
    }

    #[test]
    fn sweeping_cutoff_stays_finite() {
        let mut p = classic();
        let mut b = bank(&p);
        for i in 0..48_000 {
            if i % 4_800 == 0 {
                p.lpf_hz = if (i / 4_800) % 2 == 0 {
                    600.0
                } else {
                    18_000.0
                };
                p.hpf_hz = if (i / 4_800) % 2 == 0 { 800.0 } else { 25.0 };
                b.set_params(&p);
            }
            let x = if i % 97 == 0 { 1.0 } else { -0.1 };
            let (y, _) = b.process(x, x);
            assert!(y.is_finite() && y.abs() < 10.0);
        }
    }
}
