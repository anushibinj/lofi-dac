//! The complete lofi chain.
//!
//! Signal flow (per stereo frame):
//!
//! ```text
//! input --+--> [dry delay = latency] --------------------------------+
//!         |                                                          v
//!         +--> trim -> tape -> saturation -> crusher -> filters -> reverb
//!                       -> + crackle/hiss -> output gain -> soft limiter --> [wet/dry mix] -> out
//! ```
//!
//! The wet path has a fixed latency (the tape base delay). The dry path is delayed by exactly the
//! same number of samples, so mixing the two never comb-filters. Bypass is a 20 ms linear
//! crossfade to that delay-compensated dry signal and is bit-exact once complete.

use crate::crusher::Crusher;
use crate::filters::{self, FilterBank};
use crate::limiter::soft_limit;
use crate::params::Params;
use crate::reverb::Reverb;
use crate::saturation::Saturation;
use crate::shared::SharedParams;
use crate::tape::Tape;
use crate::util::{Smoothed, db_to_lin, sanitize};
use crate::vinyl::Vinyl;

/// Default constant latency of the wet path (the tape base delay).
pub const DEFAULT_BASE_DELAY_MS: f32 = 10.0;
/// Length of the bypass crossfade.
pub const BYPASS_FADE_MS: f32 = 20.0;

/// Construction-time options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChainConfig {
    /// Base delay of the tape stage in milliseconds. This is the chain's latency.
    pub base_delay_ms: f32,
    /// Seed for every random source (crackle, hiss, drift, dither).
    pub seed: u32,
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            base_delay_ms: DEFAULT_BASE_DELAY_MS,
            seed: 0x04C0_F1F1,
        }
    }
}

/// The lofi effects chain. Allocates once in [`Chain::new`]; [`Chain::process`] never allocates.
#[derive(Debug, Clone)]
pub struct Chain {
    sample_rate: f32,
    params: Params,
    latency: usize,

    trim: Smoothed,
    out_gain: Smoothed,
    wet_dry: Smoothed,
    tape: Tape,
    saturation: Saturation,
    crusher: Crusher,
    filters: FilterBank,
    reverb: Reverb,
    vinyl: Vinyl,

    dry_l: Vec<f32>,
    dry_r: Vec<f32>,
    dry_mask: usize,
    dry_pos: usize,

    /// 0.0 = fully processed, 1.0 = fully bypassed.
    bypass_pos: f32,
    bypass_target: f32,
    bypass_step: f32,
}

impl Chain {
    /// Creates a chain with default configuration (10 ms latency), all effects off.
    pub fn new(sample_rate: f32) -> Self {
        Self::with_config(sample_rate, ChainConfig::default())
    }

    /// Creates a chain with explicit configuration.
    pub fn with_config(sample_rate: f32, config: ChainConfig) -> Self {
        let sr = if sample_rate.is_finite() && sample_rate >= 8_000.0 {
            sample_rate
        } else {
            48_000.0
        };
        let base = ((config.base_delay_ms.max(0.0) * 0.001 * sr).round() as usize).max(4);
        let tape = Tape::new(sr, base, config.seed ^ 0xA5A5);
        let latency = tape.latency_samples();
        let dry_len = (latency + 2).next_power_of_two();
        let mut chain = Self {
            sample_rate: sr,
            params: Params::default(),
            latency,
            trim: Smoothed::with_default_time(1.0, sr),
            out_gain: Smoothed::with_default_time(1.0, sr),
            wet_dry: Smoothed::with_default_time(1.0, sr),
            tape,
            saturation: Saturation::new(sr),
            crusher: Crusher::new(sr, config.seed ^ 0x5A5A),
            filters: FilterBank::new(sr),
            reverb: Reverb::new(sr),
            vinyl: Vinyl::new(sr, config.seed),
            dry_l: vec![0.0; dry_len],
            dry_r: vec![0.0; dry_len],
            dry_mask: dry_len - 1,
            dry_pos: 0,
            bypass_pos: 0.0,
            bypass_target: 0.0,
            bypass_step: 1.0 / (BYPASS_FADE_MS * 0.001 * sr),
        };
        chain.apply_targets(&Params::default());
        chain.snap();
        chain
    }

    /// Sample rate the chain was built for.
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Constant latency in samples. Bypassed output equals the input delayed by this amount.
    pub fn latency_samples(&self) -> usize {
        self.latency
    }

    /// The (sanitised) parameters currently targeted.
    pub fn params(&self) -> &Params {
        &self.params
    }

    /// Sets new parameter targets. Values are sanitised and smoothed per sample (about 10 ms).
    pub fn set_params(&mut self, params: &Params) {
        let p = params.sanitized();
        if p != self.params {
            self.apply_targets(&p);
        }
    }

    fn apply_targets(&mut self, p: &Params) {
        self.params = *p;
        self.trim.set_target(db_to_lin(p.input.trim_db));
        self.tape.set_params(
            p.tape.wow_cents,
            p.tape.wow_hz,
            p.tape.flutter_cents,
            p.tape.flutter_hz,
            p.tape.drift_cents,
        );
        self.saturation
            .set_params(p.saturation.drive, p.saturation.makeup_db);
        self.crusher
            .set_params(p.crush.bits, p.crush.sample_rate_hz, p.crush.dither);
        self.filters.set_params(&p.filters);
        self.reverb
            .set_params(p.reverb.room, p.reverb.damping, p.reverb.mix);
        self.vinyl
            .set_params(p.vinyl.pops_per_sec, p.vinyl.pop_db, p.vinyl.hiss_db);
        self.out_gain.set_target(db_to_lin(p.output.gain_db));
        self.wet_dry.set_target(p.mix.wet_dry);
    }

    /// Starts a 20 ms crossfade to (or from) the delay-compensated dry signal.
    pub fn set_bypass(&mut self, bypass: bool) {
        self.bypass_target = if bypass { 1.0 } else { 0.0 };
    }

    /// True if the chain is bypassed or fading towards bypass.
    pub fn is_bypassed(&self) -> bool {
        self.bypass_target >= 1.0
    }

    /// Pulls the latest parameters and bypass flag from the lock-free shared block.
    /// Call once per audio block.
    pub fn sync_from(&mut self, shared: &SharedParams) {
        self.set_params(&shared.load());
        self.set_bypass(shared.bypass());
    }

    /// Jumps every smoother to its target with no ramp. For offline rendering and tests.
    pub fn snap(&mut self) {
        for s in [&mut self.trim, &mut self.out_gain, &mut self.wet_dry] {
            let t = s.target();
            s.snap(t);
        }
        self.tape.snap();
        self.saturation.snap();
        self.crusher.snap();
        self.filters.snap();
        self.reverb.snap();
        self.vinyl.snap();
        self.bypass_pos = self.bypass_target;
    }

    /// Magnitude response in dB of the filter stages for the current parameters, computed
    /// analytically from the biquad coefficients. Allocates; not for the audio thread.
    pub fn frequency_response(&self, freqs: &[f32]) -> Vec<f32> {
        filters::frequency_response(&self.params.filters, self.sample_rate, freqs)
    }

    #[inline]
    fn wet(&mut self, l: f32, r: f32) -> (f32, f32) {
        let trim = self.trim.tick();
        let (l, r) = self.tape.process(l * trim, r * trim);
        let (l, r) = self.saturation.process(l, r);
        let (l, r) = self.crusher.process(l, r);
        let (l, r) = self.filters.process(l, r);
        let (l, r) = self.reverb.process(l, r);
        let (nl, nr) = self.vinyl.process();
        let g = self.out_gain.tick();
        (soft_limit((l + nl) * g), soft_limit((r + nr) * g))
    }

    #[inline]
    fn advance_bypass(&mut self) {
        if self.bypass_pos < self.bypass_target {
            self.bypass_pos = (self.bypass_pos + self.bypass_step).min(self.bypass_target);
        } else if self.bypass_pos > self.bypass_target {
            self.bypass_pos = (self.bypass_pos - self.bypass_step).max(self.bypass_target);
        }
    }

    /// Processes interleaved stereo `f32` samples in place. A trailing odd sample is left as is.
    /// Never allocates, locks or panics; non-finite input becomes silence.
    pub fn process(&mut self, buf: &mut [f32]) {
        for frame in buf.chunks_exact_mut(2) {
            let (in_l, in_r) = (frame[0], frame[1]);

            // Dry path: an exact integer delay of the raw input.
            self.dry_pos = self.dry_pos.wrapping_add(1);
            self.dry_l[self.dry_pos & self.dry_mask] = in_l;
            self.dry_r[self.dry_pos & self.dry_mask] = in_r;
            let read = self.dry_pos.wrapping_sub(self.latency) & self.dry_mask;
            let (dry_l, dry_r) = (self.dry_l[read], self.dry_r[read]);

            let (wet_l, wet_r) = self.wet(sanitize(in_l), sanitize(in_r));

            self.advance_bypass();
            let w = self.wet_dry.tick() * (1.0 - self.bypass_pos);
            if w == 0.0 {
                frame[0] = sanitize(dry_l);
                frame[1] = sanitize(dry_r);
            } else {
                let d = 1.0 - w;
                frame[0] = sanitize(dry_l * d + wet_l * w);
                frame[1] = sanitize(dry_r * d + wet_r * w);
            }
        }
    }
}
