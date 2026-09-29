//! Soft saturation: `y = tanh(drive * x) / drive`, followed by makeup gain.
//!
//! Dividing by `drive` keeps small signals at unity gain and makes `drive -> 0` the identity, so
//! the drive knob can sweep from clean to crunchy without a level jump.

use crate::util::{Smoothed, db_to_lin};

/// Below this drive the stage is a pure pass-through.
const DRIVE_OFF: f32 = 1.0e-3;

/// Stereo saturation stage.
#[derive(Debug, Clone)]
pub struct Saturation {
    drive: Smoothed,
    makeup: Smoothed,
}

impl Saturation {
    /// Creates a stage that starts clean.
    pub fn new(sample_rate: f32) -> Self {
        Self {
            drive: Smoothed::with_default_time(0.0, sample_rate),
            makeup: Smoothed::with_default_time(1.0, sample_rate),
        }
    }

    /// Sets targets. `makeup_db` is converted to linear once here, not per sample.
    pub fn set_params(&mut self, drive: f32, makeup_db: f32) {
        self.drive.set_target(drive);
        self.makeup.set_target(db_to_lin(makeup_db));
    }

    /// Jumps to the current targets.
    pub fn snap(&mut self) {
        let (d, m) = (self.drive.target(), self.makeup.target());
        self.drive.snap(d);
        self.makeup.snap(m);
    }

    /// Processes one stereo frame.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let d = self.drive.tick();
        let g = self.makeup.tick();
        if d < DRIVE_OFF {
            (l * g, r * g)
        } else {
            let inv = 1.0 / d;
            ((d * l).tanh() * inv * g, (d * r).tanh() * inv * g)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn stage(drive: f32, makeup_db: f32) -> Saturation {
        let mut s = Saturation::new(48_000.0);
        s.set_params(drive, makeup_db);
        s.snap();
        s
    }

    #[test]
    fn zero_drive_is_identity() {
        let mut s = stage(0.0, 0.0);
        for x in [-1.0f32, -0.3, 0.0, 0.7, 1.0] {
            assert_eq!(s.process(x, x), (x, x));
        }
    }

    #[test]
    fn small_signals_keep_unity_gain() {
        let mut s = stage(3.0, 0.0);
        let (y, _) = s.process(0.001, 0.0);
        assert!((y / 0.001 - 1.0).abs() < 1.0e-3);
    }

    #[test]
    fn output_is_bounded_by_inverse_drive() {
        let mut s = stage(4.0, 0.0);
        let (y, _) = s.process(10.0, -10.0);
        assert!(y <= 0.25 + 1.0e-6);
    }

    #[test]
    fn makeup_gain_is_applied() {
        let mut s = stage(0.0, 6.0);
        let (y, _) = s.process(0.5, 0.5);
        assert!((y - 0.5 * db_to_lin(6.0)).abs() < 1.0e-6);
    }

    #[test]
    fn saturation_generates_odd_harmonics() {
        // A hot 1 kHz sine through tanh must gain a 3rd harmonic; a clean one must not.
        let sr = 48_000.0;
        let harmonic = |drive: f32| {
            let mut s = stage(drive, 0.0);
            let n = 4800; // exactly 100 periods of 1 kHz
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for i in 0..n {
                let x = 0.8 * (TAU * 1000.0 * i as f32 / sr).sin();
                let (y, _) = s.process(x, x);
                let w = f64::from(TAU) * 3000.0 * i as f64 / f64::from(sr);
                re += f64::from(y) * w.cos();
                im += f64::from(y) * w.sin();
            }
            ((re * re + im * im).sqrt() * 2.0 / n as f64) as f32
        };
        assert!(harmonic(0.0) < 1.0e-4);
        assert!(harmonic(4.0) > 0.01);
    }
}
