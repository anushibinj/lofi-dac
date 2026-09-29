//! Small numeric helpers shared by every DSP stage.

use std::f32::consts::PI;

/// Values with a magnitude below this are flushed to zero inside feedback paths so the FPU never
/// has to handle denormals.
const DENORMAL_THRESHOLD: f32 = 1.0e-20;

/// Converts decibels to a linear amplitude factor.
#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Converts a linear amplitude to decibels, floored at -180 dB.
#[inline]
pub fn lin_to_db(x: f32) -> f32 {
    20.0 * x.abs().max(1.0e-9).log10()
}

/// Replaces NaN and infinities with silence.
#[inline]
pub fn sanitize(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// Flushes tiny values (potential denormals) to exactly zero.
#[inline]
pub fn flush_denormal(x: f32) -> f32 {
    if x.abs() < DENORMAL_THRESHOLD { 0.0 } else { x }
}

/// Same as [`flush_denormal`] for `f64` filter state.
#[inline]
pub fn flush_denormal_f64(x: f64) -> f64 {
    if x.abs() < 1.0e-30 { 0.0 } else { x }
}

/// Clamps `x` into `[lo, hi]`, mapping NaN to `fallback`.
#[inline]
pub fn clamp_or(x: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if x.is_finite() {
        x.clamp(lo, hi)
    } else {
        fallback
    }
}

/// Wraps a phase in `[0, 1)`.
#[inline]
pub fn wrap_phase(p: f32) -> f32 {
    p - p.floor()
}

/// Angular frequency helper: `2 * pi * f`.
#[inline]
pub fn tau_hz(f: f32) -> f32 {
    2.0 * PI * f
}

/// Default parameter smoothing time constant.
pub const SMOOTH_MS: f32 = 10.0;

/// One-pole parameter smoother, ticked once per sample.
///
/// The value snaps to the target once it is within a tiny relative distance, so a settled
/// smoother returns *exactly* its target and downstream code can detect "no change" with `==`.
#[derive(Debug, Clone, Copy)]
pub struct Smoothed {
    current: f32,
    target: f32,
    coeff: f32,
}

impl Smoothed {
    /// Creates a smoother that starts settled at `value`.
    pub fn new(value: f32, sample_rate: f32, time_ms: f32) -> Self {
        let samples = (time_ms * 0.001 * sample_rate).max(1.0);
        Self {
            current: value,
            target: value,
            coeff: 1.0 - (-1.0 / samples).exp(),
        }
    }

    /// Creates a smoother with the default 10 ms time constant.
    pub fn with_default_time(value: f32, sample_rate: f32) -> Self {
        Self::new(value, sample_rate, SMOOTH_MS)
    }

    /// Sets a new target. Non-finite targets are ignored.
    #[inline]
    pub fn set_target(&mut self, target: f32) {
        if target.is_finite() {
            self.target = target;
        }
    }

    /// Jumps straight to `value` with no smoothing.
    pub fn snap(&mut self, value: f32) {
        if value.is_finite() {
            self.current = value;
            self.target = value;
        }
    }

    /// The target the smoother is heading towards.
    #[inline]
    pub fn target(&self) -> f32 {
        self.target
    }

    /// The current smoothed value without advancing.
    #[inline]
    pub fn value(&self) -> f32 {
        self.current
    }

    /// True when the current value equals the target.
    #[inline]
    pub fn is_settled(&self) -> bool {
        self.current == self.target
    }

    /// Advances one sample and returns the new value.
    #[inline]
    pub fn next(&mut self) -> f32 {
        let d = self.target - self.current;
        let next = self.current + d * self.coeff;
        // `next == current` means the increment fell below one ulp and the one-pole has stalled
        // just short of the target, so finish the move instead of hovering there forever.
        if next == self.current || d.abs() <= 1.0e-6 * (1.0 + self.target.abs()) {
            self.current = self.target;
        } else {
            self.current = next;
        }
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_roundtrip() {
        for db in [-60.0f32, -6.0, 0.0, 3.0, 12.0] {
            assert!((lin_to_db(db_to_lin(db)) - db).abs() < 1.0e-3);
        }
    }

    #[test]
    fn sanitize_replaces_non_finite() {
        assert_eq!(sanitize(f32::NAN), 0.0);
        assert_eq!(sanitize(f32::INFINITY), 0.0);
        assert_eq!(sanitize(f32::NEG_INFINITY), 0.0);
        assert_eq!(sanitize(0.25), 0.25);
    }

    #[test]
    fn denormals_are_flushed() {
        assert_eq!(flush_denormal(1.0e-30), 0.0);
        assert_eq!(flush_denormal(0.5), 0.5);
    }

    #[test]
    fn smoother_reaches_target_exactly() {
        let mut s = Smoothed::with_default_time(0.0, 48_000.0);
        s.set_target(1.0);
        let mut steps = 0;
        while !s.is_settled() && steps < 48_000 {
            s.next();
            steps += 1;
        }
        assert!(s.is_settled());
        assert_eq!(s.value(), 1.0);
        // ~10 ms time constant: 10 time constants is well under 200 ms at 48 kHz.
        assert!(steps < 9_600, "took {steps} samples");
    }

    #[test]
    fn smoother_time_constant_is_about_10ms() {
        let sr = 48_000.0;
        let mut s = Smoothed::with_default_time(0.0, sr);
        s.set_target(1.0);
        let n = (0.010 * sr) as usize;
        let mut v = 0.0;
        for _ in 0..n {
            v = s.next();
        }
        // One time constant reaches 1 - 1/e = 63.2 %.
        assert!((v - 0.632).abs() < 0.01, "v = {v}");
    }

    #[test]
    fn smoother_ignores_nan() {
        let mut s = Smoothed::with_default_time(0.5, 48_000.0);
        s.set_target(f32::NAN);
        assert_eq!(s.next(), 0.5);
    }
}
