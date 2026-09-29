//! Memoryless soft limiter.
//!
//! Signals below the knee pass untouched. Above it, `tanh` bends the signal smoothly towards the
//! ceiling with a continuous first derivative, so the output never exceeds -1 dBFS and there is
//! no lookahead latency.

/// -1 dBFS.
pub const CEILING: f32 = 0.891_250_9;
/// Start of the soft region.
pub const KNEE: f32 = 0.7;

/// Soft-limits one sample. Non-finite input yields 0.
#[inline]
pub fn soft_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        return if x.is_finite() { x } else { 0.0 };
    }
    if !a.is_finite() {
        return 0.0;
    }
    let range = CEILING - KNEE;
    let y = KNEE + range * ((a - KNEE) / range).tanh();
    y.min(CEILING).copysign(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_below_the_knee() {
        for i in -700..=700 {
            let x = i as f32 / 1000.0;
            assert_eq!(soft_limit(x), x);
        }
    }

    #[test]
    fn never_exceeds_minus_one_dbfs() {
        for i in 0..100_000 {
            let x = i as f32 * 0.001;
            assert!(soft_limit(x) <= CEILING, "x = {x}");
            assert!(soft_limit(-x) >= -CEILING, "x = -{x}");
        }
        assert!(soft_limit(f32::MAX) <= CEILING);
        assert!(soft_limit(1.0e30) <= CEILING);
    }

    #[test]
    fn is_monotonic_and_continuous() {
        let mut prev = soft_limit(-5.0);
        let mut x = -5.0f32;
        while x < 5.0 {
            x += 0.0005;
            let y = soft_limit(x);
            assert!(y >= prev - 1.0e-6, "not monotonic at {x}");
            assert!((y - prev).abs() < 0.001, "jump at {x}: {prev} -> {y}");
            prev = y;
        }
    }

    #[test]
    fn full_scale_maps_below_ceiling() {
        let y = soft_limit(1.0);
        assert!(y < CEILING && y > 0.8, "{y}");
    }

    #[test]
    fn symmetric() {
        for x in [0.5f32, 0.75, 1.0, 3.0] {
            assert_eq!(soft_limit(-x), -soft_limit(x));
        }
    }

    #[test]
    fn non_finite_becomes_silence() {
        assert_eq!(soft_limit(f32::NAN), 0.0);
        assert_eq!(soft_limit(f32::INFINITY), 0.0);
        assert_eq!(soft_limit(f32::NEG_INFINITY), 0.0);
    }
}
