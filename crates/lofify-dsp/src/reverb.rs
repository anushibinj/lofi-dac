//! Freeverb-style stereo reverb: 8 parallel damped feedback combs and 4 series allpasses per
//! channel, with the right channel's delay lengths offset for stereo spread.

use crate::util::{Smoothed, flush_denormal};

/// Original Freeverb delay lengths, tuned for 44.1 kHz.
const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_TUNING: [usize; 4] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;
const REFERENCE_RATE: f32 = 44_100.0;
/// Input attenuation that keeps the comb bank from clipping.
const INPUT_GAIN: f32 = 0.015;
const ALLPASS_FEEDBACK: f32 = 0.5;
/// Makes a fully wet signal about as loud as the dry input for broadband material.
pub(crate) const WET_SCALE: f32 = 1.5;

#[derive(Debug, Clone)]
struct Comb {
    buf: Vec<f32>,
    idx: usize,
    store: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            idx: 0,
            store: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let out = self.buf[self.idx];
        self.store = flush_denormal(out * (1.0 - damp) + self.store * damp);
        self.buf[self.idx] = flush_denormal(input + self.store * feedback);
        self.idx += 1;
        if self.idx == self.buf.len() {
            self.idx = 0;
        }
        out
    }
}

#[derive(Debug, Clone)]
struct Allpass {
    buf: Vec<f32>,
    idx: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            idx: 0,
        }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let buffered = self.buf[self.idx];
        let out = buffered - input;
        self.buf[self.idx] = flush_denormal(input + buffered * ALLPASS_FEEDBACK);
        self.idx += 1;
        if self.idx == self.buf.len() {
            self.idx = 0;
        }
        out
    }
}

/// Stereo reverb stage.
#[derive(Debug, Clone)]
pub struct Reverb {
    combs_l: Vec<Comb>,
    combs_r: Vec<Comb>,
    allpass_l: Vec<Allpass>,
    allpass_r: Vec<Allpass>,
    room: Smoothed,
    damping: Smoothed,
    mix: Smoothed,
}

impl Reverb {
    /// Creates a reverb with all buffers allocated once. Delay lengths scale with `sample_rate`.
    pub fn new(sample_rate: f32) -> Self {
        let scale = sample_rate / REFERENCE_RATE;
        let len = |n: usize| ((n as f32) * scale).round() as usize;
        let spread = len(STEREO_SPREAD);
        Self {
            combs_l: COMB_TUNING.iter().map(|&n| Comb::new(len(n))).collect(),
            combs_r: COMB_TUNING
                .iter()
                .map(|&n| Comb::new(len(n) + spread))
                .collect(),
            allpass_l: ALLPASS_TUNING
                .iter()
                .map(|&n| Allpass::new(len(n)))
                .collect(),
            allpass_r: ALLPASS_TUNING
                .iter()
                .map(|&n| Allpass::new(len(n) + spread))
                .collect(),
            room: Smoothed::with_default_time(0.5, sample_rate),
            damping: Smoothed::with_default_time(0.5, sample_rate),
            mix: Smoothed::with_default_time(0.0, sample_rate),
        }
    }

    /// Sets targets: `room` 0..1 (decay time), `damping` 0..1 (high-frequency absorption),
    /// `mix` 0..1 (0 = dry, 1 = fully wet).
    pub fn set_params(&mut self, room: f32, damping: f32, mix: f32) {
        self.room.set_target(room);
        self.damping.set_target(damping);
        self.mix.set_target(mix);
    }

    /// Jumps to the current targets.
    pub fn snap(&mut self) {
        for s in [&mut self.room, &mut self.damping, &mut self.mix] {
            let t = s.target();
            s.snap(t);
        }
    }

    /// Processes one stereo frame.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let feedback = 0.7 + 0.26 * self.room.tick();
        let damp = 0.4 * self.damping.tick();
        let mix = self.mix.tick();

        let input = (l + r) * INPUT_GAIN;
        let mut wet_l = 0.0;
        let mut wet_r = 0.0;
        for c in &mut self.combs_l {
            wet_l += c.process(input, feedback, damp);
        }
        for c in &mut self.combs_r {
            wet_r += c.process(input, feedback, damp);
        }
        for a in &mut self.allpass_l {
            wet_l = a.process(wet_l);
        }
        for a in &mut self.allpass_r {
            wet_r = a.process(wet_r);
        }
        wet_l *= WET_SCALE;
        wet_r *= WET_SCALE;
        (l + (wet_l - l) * mix, r + (wet_r - r) * mix)
    }

    /// Whether the mix is (and has settled at) zero, i.e. the stage is an exact pass-through.
    pub fn is_off(&self) -> bool {
        self.mix.is_settled() && self.mix.target() == 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    const SR: f32 = 48_000.0;

    fn reverb(room: f32, damping: f32, mix: f32) -> Reverb {
        let mut r = Reverb::new(SR);
        r.set_params(room, damping, mix);
        r.snap();
        r
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>() / v.len() as f64).sqrt()
            as f32
    }

    fn impulse_response(rv: &mut Reverb, seconds: f32) -> Vec<f32> {
        (0..(SR * seconds) as usize)
            .map(|i| rv.process(if i == 0 { 1.0 } else { 0.0 }, 0.0).0)
            .collect()
    }

    #[test]
    fn zero_mix_is_bit_exact_passthrough() {
        let mut rv = reverb(0.9, 0.2, 0.0);
        assert!(rv.is_off());
        for i in 0..5000 {
            let x = (i as f32 * 0.21).sin();
            assert_eq!(rv.process(x, -x), (x, -x));
        }
    }

    #[test]
    fn impulse_produces_a_decaying_tail() {
        let mut rv = reverb(0.7, 0.5, 1.0);
        let ir = impulse_response(&mut rv, 3.0);
        let win = |from: f32, to: f32| rms(&ir[(from * SR) as usize..(to * SR) as usize]);
        let early = win(0.05, 0.15);
        let late = win(1.5, 1.6);
        assert!(early > 1.0e-4, "no tail: {early}");
        assert!(late < early * 0.5, "tail does not decay: {early} -> {late}");
    }

    #[test]
    fn bigger_room_decays_slower() {
        let tail = |room: f32| {
            let mut rv = reverb(room, 0.3, 1.0);
            let ir = impulse_response(&mut rv, 2.0);
            rms(&ir[(1.0 * SR) as usize..(1.2 * SR) as usize])
        };
        assert!(tail(0.9) > tail(0.3) * 3.0);
    }

    #[test]
    fn damping_removes_high_frequencies_from_the_tail() {
        // Energy of the first difference relative to total energy grows with brightness.
        let brightness = |damping: f32| {
            let mut rv = reverb(0.7, damping, 1.0);
            let ir = impulse_response(&mut rv, 1.0);
            let tail = &ir[4_800..];
            let diff: f64 = tail
                .windows(2)
                .map(|w| f64::from(w[1] - w[0]).powi(2))
                .sum();
            let total: f64 = tail.iter().map(|x| f64::from(*x).powi(2)).sum();
            diff / total
        };
        assert!(brightness(0.0) > brightness(1.0) * 1.5);
    }

    #[test]
    fn wet_level_is_comparable_to_input_level() {
        let mut rng = Rng::new(99);
        let mut rv = reverb(0.5, 0.5, 1.0);
        let input: Vec<f32> = (0..SR as usize * 6).map(|_| rng.bipolar() * 0.3).collect();
        let out: Vec<f32> = input.iter().map(|&x| rv.process(x, x).0).collect();
        let half = input.len() / 2;
        let db = 20.0 * (rms(&out[half..]) / rms(&input[half..])).log10();
        assert!(db.abs() < 3.0, "wet level {db} dB relative to input");
    }

    #[test]
    fn channels_are_decorrelated() {
        let mut rv = reverb(0.6, 0.5, 1.0);
        let mut differ = 0;
        for i in 0..20_000 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = rv.process(x, x);
            if (l - r).abs() > 1.0e-6 {
                differ += 1;
            }
        }
        assert!(differ > 1_000);
    }

    #[test]
    fn stays_bounded_and_finite_at_maximum_settings() {
        let mut rng = Rng::new(5);
        let mut rv = reverb(1.0, 0.0, 1.0);
        for _ in 0..SR as usize * 10 {
            let (l, r) = rv.process(rng.bipolar(), rng.bipolar());
            assert!(l.is_finite() && r.is_finite());
            assert!(l.abs() < 50.0 && r.abs() < 50.0);
        }
    }

    #[test]
    fn tail_decays_to_exact_silence_without_denormals() {
        let mut rv = reverb(0.0, 0.5, 1.0);
        let ir = impulse_response(&mut rv, 8.0);
        assert!(ir[ir.len() - 1000..].iter().all(|&x| x == 0.0));
    }
}
