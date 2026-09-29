//! Bitcrusher and sample-rate reducer.
//!
//! Quantisation uses `step = 2^(1 - bits)` (a full-scale range of 2.0 divided into `2^bits`
//! levels) with optional TPDF dither. The sample-rate reducer is a sample-and-hold driven by a
//! phase accumulator, so any target rate works, including non-integer ratios.

use crate::rng::Rng;
use crate::util::Smoothed;

/// At or above this many bits the quantiser is bypassed.
const BITS_OFF: f32 = 24.0;

/// Stereo crusher stage.
#[derive(Debug, Clone)]
pub struct Crusher {
    sample_rate: f32,
    bits: Smoothed,
    target_rate: Smoothed,
    dither: Smoothed,
    phase: f32,
    held_l: f32,
    held_r: f32,
    rng: Rng,
}

impl Crusher {
    /// Creates a transparent crusher.
    pub fn new(sample_rate: f32, seed: u32) -> Self {
        Self {
            sample_rate,
            bits: Smoothed::with_default_time(BITS_OFF, sample_rate),
            target_rate: Smoothed::with_default_time(sample_rate, sample_rate),
            dither: Smoothed::with_default_time(0.0, sample_rate),
            phase: 1.0,
            held_l: 0.0,
            held_r: 0.0,
            rng: Rng::new(seed),
        }
    }

    /// Sets targets.
    pub fn set_params(&mut self, bits: f32, rate_hz: f32, dither: bool) {
        self.bits.set_target(bits);
        self.target_rate.set_target(rate_hz);
        self.dither.set_target(if dither { 1.0 } else { 0.0 });
    }

    /// Jumps to the current targets.
    pub fn snap(&mut self) {
        for s in [&mut self.bits, &mut self.target_rate, &mut self.dither] {
            let t = s.target();
            s.snap(t);
        }
    }

    #[inline]
    fn quantize(&mut self, x: f32, bits: f32, dither_amount: f32) -> f32 {
        if bits >= BITS_OFF {
            return x;
        }
        let step = (1.0f32 - bits).exp2();
        // TPDF dither: the sum of two uniform variables spans +-1 LSB.
        let d = (self.rng.uniform() + self.rng.uniform() - 1.0) * dither_amount;
        ((x / step + d).round()) * step
    }

    /// Processes one stereo frame.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let bits = self.bits.tick();
        let rate = self.target_rate.tick();
        let dither = self.dither.tick();

        // Capping the increment at 1 keeps the phase bounded when the target rate exceeds the
        // device rate, where the stage must be transparent.
        self.phase += (rate / self.sample_rate).clamp(0.0, 1.0);
        if self.phase >= 1.0 {
            self.phase -= 1.0;
            self.held_l = self.quantize(l, bits, dither);
            self.held_r = self.quantize(r, bits, dither);
        }
        (self.held_l, self.held_r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const SR: f32 = 48_000.0;

    fn crusher(bits: f32, rate: f32, dither: bool) -> Crusher {
        let mut c = Crusher::new(SR, 5);
        c.set_params(bits, rate, dither);
        c.snap();
        c
    }

    #[test]
    fn transparent_when_off() {
        let mut c = crusher(24.0, 192_000.0, false);
        for i in 0..1000 {
            let x = (i as f32 * 0.37).sin() * 0.9;
            assert_eq!(c.process(x, -x), (x, -x));
        }
    }

    #[test]
    fn quantiser_produces_only_grid_values() {
        let bits = 4.0;
        let step = (1.0f32 - bits).exp2();
        let mut c = crusher(bits, 192_000.0, false);
        let mut levels = BTreeSet::new();
        for i in 0..5000 {
            let x = (i as f32 * 0.013).sin();
            let (y, _) = c.process(x, x);
            let k = y / step;
            assert!((k - k.round()).abs() < 1.0e-4, "{y} is off the grid");
            assert!((y - x).abs() <= step / 2.0 + 1.0e-6);
            levels.insert((k.round() as i32, 0));
        }
        assert!(levels.len() <= 17, "{} levels", levels.len());
        assert!(levels.len() > 8);
    }

    #[test]
    fn sample_hold_repeats_values_at_target_rate() {
        // 12 kHz at 48 kHz  =>  every value is held for 4 frames.
        let mut c = crusher(24.0, 12_000.0, false);
        let out: Vec<f32> = (0..400).map(|i| c.process(i as f32, 0.0).0).collect();
        let mut changes = 0;
        for w in out.windows(2) {
            if w[0] != w[1] {
                changes += 1;
            }
        }
        assert!((99..=100).contains(&changes), "{changes} changes");
        // Captured values are input frame indices, so consecutive captures are exactly 4 apart.
        let mut captured: Vec<i32> = out.iter().map(|&v| v as i32).collect();
        captured.dedup();
        for w in captured.windows(2).skip(1) {
            assert_eq!(w[1] - w[0], 4);
        }
    }

    #[test]
    fn non_integer_ratio_holds_correct_average_rate() {
        // 20 kHz from 48 kHz: 20000 captures in one second.
        let mut c = crusher(24.0, 20_000.0, false);
        let mut last = f32::NAN;
        let mut updates = 0;
        for i in 0..48_000 {
            let (y, _) = c.process(i as f32, 0.0);
            if y != last {
                updates += 1;
                last = y;
            }
        }
        assert!((19_990..=20_010).contains(&updates), "{updates}");
    }

    #[test]
    fn dither_linearises_sub_lsb_signals() {
        // A DC value of a quarter LSB rounds to 0 without dither, but the average of the dithered
        // output tracks the input.
        let bits = 8.0;
        let step = (1.0f32 - bits).exp2();
        let dc = step * 0.25;
        let mean = |dither: bool| {
            let mut c = crusher(bits, 192_000.0, dither);
            let n = 200_000;
            (0..n).map(|_| f64::from(c.process(dc, dc).0)).sum::<f64>() / f64::from(n)
        };
        assert_eq!(mean(false), 0.0);
        let m = mean(true);
        assert!(
            (m - f64::from(dc)).abs() < f64::from(step) * 0.02,
            "mean {m} vs {dc}"
        );
    }
}
