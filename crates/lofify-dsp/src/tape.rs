//! Tape wow and flutter: a modulated fractional delay line.
//!
//! The delay is `base + wow LFO + flutter LFO + slow random drift`. Because pitch is the time
//! derivative of the delay, a peak pitch deviation of `c` cents at LFO frequency `f` needs a
//! delay amplitude of `(2^(c/1200) - 1) / (2 pi f)` seconds. Both channels share one modulator,
//! like a real tape transport. With all depths at zero the stage is an exact integer delay.

use crate::rng::Rng;
use crate::util::{Smoothed, wrap_phase};
use std::f32::consts::TAU;

/// Smallest delay in samples the cubic interpolator can read without touching future samples.
const MIN_DELAY: f32 = 2.0;
/// Time constant of the random drift walk, in seconds.
const DRIFT_TAU_S: f32 = 0.5;

/// Peak delay amplitude in seconds for a pitch deviation of `cents` at `hz`.
pub fn delay_amplitude_seconds(cents: f32, hz: f32) -> f32 {
    ((cents / 1200.0).exp2() - 1.0) / (TAU * hz.max(0.01))
}

/// Slow random drift in `[-1, 1]`: a one-pole follower chasing a random target.
#[derive(Debug, Clone)]
struct Drift {
    rng: Rng,
    value: f32,
    target: f32,
    coeff: f32,
    countdown: u32,
    period: u32,
}

impl Drift {
    fn new(sample_rate: f32, seed: u32) -> Self {
        let period = (DRIFT_TAU_S * sample_rate) as u32;
        Self {
            rng: Rng::new(seed),
            value: 0.0,
            target: 0.0,
            coeff: 1.0 - (-1.0 / (DRIFT_TAU_S * sample_rate)).exp(),
            countdown: 0,
            period: period.max(1),
        }
    }

    #[inline]
    fn tick(&mut self) -> f32 {
        if self.countdown == 0 {
            self.target = self.rng.bipolar();
            // 0.6x .. 1.4x of the nominal period
            let jitter = 0.6 + 0.8 * self.rng.uniform();
            self.countdown = ((self.period as f32) * jitter) as u32 + 1;
        }
        self.countdown -= 1;
        self.value += (self.target - self.value) * self.coeff;
        self.value
    }
}

/// The tape transport stage.
#[derive(Debug, Clone)]
pub struct Tape {
    sample_rate: f32,
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    mask: usize,
    /// Index of the sample written most recently.
    pos: usize,
    base_delay: usize,
    wow_phase: f32,
    flutter_phase: f32,
    drift: Drift,
    wow_cents: Smoothed,
    wow_hz: Smoothed,
    flutter_cents: Smoothed,
    flutter_hz: Smoothed,
    drift_cents: Smoothed,
    /// Most recent instantaneous delay in samples (for tests and metering).
    last_delay: f32,
}

impl Tape {
    /// Creates the stage with `base_delay_samples` of constant latency.
    pub fn new(sample_rate: f32, base_delay_samples: usize, seed: u32) -> Self {
        let base_delay = base_delay_samples.max(MIN_DELAY as usize + 1);
        let len = (2 * base_delay + 8).next_power_of_two();
        let sr = sample_rate;
        Self {
            sample_rate,
            buf_l: vec![0.0; len],
            buf_r: vec![0.0; len],
            mask: len - 1,
            pos: 0,
            base_delay,
            wow_phase: 0.0,
            flutter_phase: 0.25,
            drift: Drift::new(sr, seed),
            wow_cents: Smoothed::with_default_time(0.0, sr),
            wow_hz: Smoothed::with_default_time(0.5, sr),
            flutter_cents: Smoothed::with_default_time(0.0, sr),
            flutter_hz: Smoothed::with_default_time(6.5, sr),
            drift_cents: Smoothed::with_default_time(0.0, sr),
            last_delay: base_delay as f32,
        }
    }

    /// Constant latency of this stage in samples.
    pub fn latency_samples(&self) -> usize {
        self.base_delay
    }

    /// Sets modulation targets (cents and Hz).
    pub fn set_params(
        &mut self,
        wow_cents: f32,
        wow_hz: f32,
        flutter_cents: f32,
        flutter_hz: f32,
        drift_cents: f32,
    ) {
        self.wow_cents.set_target(wow_cents);
        self.wow_hz.set_target(wow_hz);
        self.flutter_cents.set_target(flutter_cents);
        self.flutter_hz.set_target(flutter_hz);
        self.drift_cents.set_target(drift_cents);
    }

    /// Jumps all smoothers to their targets (offline rendering and tests).
    pub fn snap(&mut self) {
        for s in [
            &mut self.wow_cents,
            &mut self.wow_hz,
            &mut self.flutter_cents,
            &mut self.flutter_hz,
            &mut self.drift_cents,
        ] {
            let t = s.target();
            s.snap(t);
        }
    }

    /// Most recent instantaneous delay in samples.
    pub fn last_delay(&self) -> f32 {
        self.last_delay
    }

    /// Processes one stereo frame.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let wow_c = self.wow_cents.tick();
        let wow_f = self.wow_hz.tick();
        let fl_c = self.flutter_cents.tick();
        let fl_f = self.flutter_hz.tick();
        let dr_c = self.drift_cents.tick();

        let sr = self.sample_rate;
        let mut a_wow = delay_amplitude_seconds(wow_c, wow_f) * sr;
        let mut a_fl = delay_amplitude_seconds(fl_c, fl_f) * sr;
        // Drift: slope of a unit random walk is at most 2 / tau, so this bounds its pitch to `dr_c`.
        let mut a_dr = ((dr_c / 1200.0).exp2() - 1.0) * DRIFT_TAU_S * 0.5 * sr;

        // Never let the read head run into the future or off the buffer.
        let headroom = self.base_delay as f32 - MIN_DELAY;
        let total = a_wow + a_fl + a_dr;
        if total > headroom {
            let k = headroom / total;
            a_wow *= k;
            a_fl *= k;
            a_dr *= k;
        }

        let wow = (self.wow_phase * TAU).sin();
        let fl = (self.flutter_phase * TAU).sin();
        let dr = self.drift.tick();
        self.wow_phase = wrap_phase(self.wow_phase + wow_f / sr);
        self.flutter_phase = wrap_phase(self.flutter_phase + fl_f / sr);

        let delay = self.base_delay as f32 + a_wow * wow + a_fl * fl + a_dr * dr;
        let delay = delay.clamp(MIN_DELAY, (self.mask - 4) as f32);
        self.last_delay = delay;

        // Write the new sample, then read `delay` samples behind it.
        self.pos = self.pos.wrapping_add(1);
        self.buf_l[self.pos & self.mask] = l;
        self.buf_r[self.pos & self.mask] = r;

        let dc = delay.ceil();
        let frac = dc - delay; // [0, 1)
        let i0 = self.pos.wrapping_sub(dc as usize);
        let out_l = cubic(&self.buf_l, self.mask, i0, frac);
        let out_r = cubic(&self.buf_r, self.mask, i0, frac);
        (out_l, out_r)
    }
}

/// Catmull-Rom interpolation between `buf[i0]` and `buf[i0 + 1]`. Returns `buf[i0]` exactly at
/// `frac == 0`.
#[inline]
fn cubic(buf: &[f32], mask: usize, i0: usize, frac: f32) -> f32 {
    let x0 = buf[i0.wrapping_sub(1) & mask];
    let x1 = buf[i0 & mask];
    let x2 = buf[i0.wrapping_add(1) & mask];
    let x3 = buf[i0.wrapping_add(2) & mask];
    x1 + 0.5
        * frac
        * (x2 - x0
            + frac * (2.0 * x0 - 5.0 * x1 + 4.0 * x2 - x3 + frac * (3.0 * (x1 - x2) + x3 - x0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;
    const BASE: usize = 480; // 10 ms

    fn tape(w: (f32, f32), f: (f32, f32), d: f32) -> Tape {
        let mut t = Tape::new(SR, BASE, 1234);
        t.set_params(w.0, w.1, f.0, f.1, d);
        t.snap();
        t
    }

    /// Runs `samples` frames of silence and records the instantaneous delay after each one.
    fn delay_trace(t: &mut Tape, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|_| {
                t.process(0.0, 0.0);
                t.last_delay()
            })
            .collect()
    }

    /// Peak pitch deviation in cents from a delay trace. The slope is taken over 32 samples so
    /// the f32 resolution of the delay value does not show up as fake pitch noise.
    fn peak_cents(delays: &[f32]) -> f32 {
        const K: usize = 32;
        (BASE * 2..delays.len() - K)
            .map(|n| {
                let ratio = 1.0 - (delays[n + K] - delays[n]) / K as f32;
                (1200.0 * ratio.log2()).abs()
            })
            .fold(0.0, f32::max)
    }

    #[test]
    fn no_modulation_is_an_exact_delay() {
        let mut t = tape((0.0, 0.5), (0.0, 6.5), 0.0);
        let input: Vec<f32> = (0..4000)
            .map(|i| ((i * 7919) % 2001) as f32 / 1000.0 - 1.0)
            .collect();
        let out: Vec<f32> = input.iter().map(|&x| t.process(x, x).0).collect();
        for n in BASE..input.len() {
            assert_eq!(out[n], input[n - BASE], "n={n}");
        }
    }

    #[test]
    fn wow_pitch_matches_configured_cents() {
        let mut t = tape((8.0, 0.5), (0.0, 6.5), 0.0);
        let delays = delay_trace(&mut t, 48_000 * 4);
        let cents = peak_cents(&delays);
        assert!((cents - 8.0).abs() < 0.2, "measured {cents} cents");
    }

    #[test]
    fn flutter_pitch_matches_configured_cents() {
        let mut t = tape((0.0, 0.5), (2.0, 6.5), 0.0);
        let delays = delay_trace(&mut t, 48_000 * 2);
        let cents = peak_cents(&delays);
        assert!((cents - 2.0).abs() < 0.1, "measured {cents} cents");
    }

    #[test]
    fn combined_modulation_stays_within_sum_of_cents() {
        let mut t = tape((8.0, 0.5), (2.0, 6.5), 1.5);
        let delays = delay_trace(&mut t, 48_000 * 10);
        let cents = peak_cents(&delays);
        assert!(cents <= 8.0 + 2.0 + 1.5 + 0.3, "measured {cents} cents");
        assert!(cents > 5.0, "modulation should be audible, got {cents}");
    }

    #[test]
    fn drift_alone_stays_within_cents() {
        let mut t = tape((0.0, 0.5), (0.0, 6.5), 3.0);
        let delays = delay_trace(&mut t, 48_000 * 20);
        let cents = peak_cents(&delays);
        assert!(cents <= 3.0 + 0.1, "measured {cents} cents");
        assert!(cents > 0.2, "drift should move the pitch, got {cents}");
    }

    #[test]
    fn interpolated_audio_matches_ideal_fractional_delay() {
        let mut t = tape((8.0, 0.5), (2.0, 6.5), 1.0);
        let f = 1_000.0f32;
        let mut worst = 0.0f32;
        for n in 0..48_000usize {
            let x = (TAU * f * n as f32 / SR).sin();
            let (y, _) = t.process(x, x);
            let ideal = (TAU * f * (n as f32 - t.last_delay()) / SR).sin();
            if n > BASE * 2 {
                worst = worst.max((y - ideal).abs());
            }
        }
        assert!(worst < 2.0e-3, "worst interpolation error {worst}");
    }

    #[test]
    fn delay_amplitude_formula() {
        // 8 cents at 0.5 Hz  =>  (2^(8/1200) - 1) / (2 pi 0.5) ~= 1.475 ms
        let a = delay_amplitude_seconds(8.0, 0.5);
        assert!((a - 0.001_475).abs() < 0.000_01, "{a}");
    }

    #[test]
    fn extreme_depth_is_clamped_and_never_reads_the_future() {
        let mut t = tape((20.0, 0.3), (10.0, 2.0), 6.0);
        let delays = delay_trace(&mut t, 48_000 * 5);
        let min = delays.iter().copied().fold(f32::MAX, f32::min);
        let max = delays.iter().copied().fold(f32::MIN, f32::max);
        assert!(min >= MIN_DELAY - 0.01, "min delay {min}");
        assert!(max <= 2.0 * BASE as f32, "max delay {max}");
    }

    #[test]
    fn latency_is_base_delay() {
        assert_eq!(Tape::new(SR, BASE, 1).latency_samples(), BASE);
    }
}
