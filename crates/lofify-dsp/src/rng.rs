//! Tiny deterministic xorshift PRNG (allocation free, audio-thread safe).

/// xorshift32 generator.
#[derive(Debug, Clone)]
pub struct Rng {
    state: u32,
}

impl Rng {
    /// Creates a generator. A zero seed is replaced by a fixed non-zero value.
    pub fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { 0x9E37_79B9 } else { seed },
        }
    }

    /// Next raw 32-bit value.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    /// Uniform in `[0, 1)`.
    #[inline]
    pub fn uniform(&mut self) -> f32 {
        // 24 random bits map exactly onto the f32 mantissa.
        (self.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0)
    }

    /// Uniform in `[-1, 1)`.
    #[inline]
    pub fn bipolar(&mut self) -> f32 {
        self.uniform() * 2.0 - 1.0
    }

    /// Exponentially distributed with mean 1, capped at `cap` to keep pops sane.
    #[inline]
    pub fn exponential(&mut self, cap: f32) -> f32 {
        (-(1.0 - self.uniform()).ln()).min(cap)
    }

    /// Random sign, `+1.0` or `-1.0`.
    #[inline]
    pub fn sign(&mut self) -> f32 {
        if self.next_u32() & 0x8000_0000 == 0 {
            1.0
        } else {
            -1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_is_in_range_and_centered() {
        let mut r = Rng::new(1);
        let n = 100_000;
        let mut sum = 0.0f64;
        for _ in 0..n {
            let u = r.uniform();
            assert!((0.0..1.0).contains(&u));
            sum += f64::from(u);
        }
        assert!((sum / f64::from(n) - 0.5).abs() < 0.01);
    }

    #[test]
    fn zero_seed_does_not_lock_up() {
        let mut r = Rng::new(0);
        assert_ne!(r.next_u32(), 0);
    }

    #[test]
    fn exponential_has_mean_near_one() {
        let mut r = Rng::new(7);
        let n = 200_000;
        let sum: f64 = (0..n).map(|_| f64::from(r.exponential(50.0))).sum();
        assert!((sum / f64::from(n) - 1.0).abs() < 0.02);
    }

    #[test]
    fn deterministic_for_same_seed() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }
}
