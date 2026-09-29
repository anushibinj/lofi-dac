//! Effect parameters: the serde model used for presets, value ranges and sanitising.
//!
//! [`Params::default`] is the "Clean" preset: every effect is off and the chain is transparent.

use crate::util::clamp_or;
use serde::{Deserialize, Serialize};
use std::ops::RangeInclusive;

/// Valid ranges for every parameter. The UI reuses these for its sliders.
pub mod range {
    use std::ops::RangeInclusive;
    type R = RangeInclusive<f32>;

    pub const TRIM_DB: R = -24.0..=12.0;
    pub const WOW_CENTS: R = 0.0..=20.0;
    pub const WOW_HZ: R = 0.3..=3.0;
    pub const FLUTTER_CENTS: R = 0.0..=10.0;
    pub const FLUTTER_HZ: R = 2.0..=12.0;
    pub const DRIFT_CENTS: R = 0.0..=6.0;
    pub const DRIVE: R = 0.0..=8.0;
    pub const MAKEUP_DB: R = -12.0..=12.0;
    /// 24 bits and above disables the quantiser.
    pub const BITS: R = 2.0..=24.0;
    /// At or above the device sample rate the sample-rate reducer is transparent.
    pub const CRUSH_RATE_HZ: R = 1_000.0..=192_000.0;
    pub const HPF_HZ: R = 20.0..=1_000.0;
    pub const LPF_HZ: R = 500.0..=20_000.0;
    pub const LPF_Q: R = 0.3..=4.0;
    pub const SHELF_HZ: R = 60.0..=800.0;
    pub const SHELF_DB: R = -12.0..=12.0;
    pub const ROOM: R = 0.0..=1.0;
    pub const DAMPING: R = 0.0..=1.0;
    pub const REVERB_MIX: R = 0.0..=1.0;
    pub const POPS_PER_SEC: R = 0.0..=40.0;
    pub const POP_DB: R = -70.0..=-10.0;
    /// At or below this the hiss is fully off.
    pub const HISS_DB: R = -90.0..=-30.0;
    pub const OUTPUT_DB: R = -24.0..=12.0;
    pub const WET_DRY: R = 0.0..=1.0;
}

/// Input trim.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InputParams {
    pub trim_db: f32,
}

/// Tape wow, flutter and slow random drift. All depths are peak pitch deviation in cents.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TapeParams {
    pub wow_cents: f32,
    pub wow_hz: f32,
    pub flutter_cents: f32,
    pub flutter_hz: f32,
    pub drift_cents: f32,
}

/// `tanh` saturation: `y = tanh(drive * x) / drive`, then makeup gain. `drive == 0` is off.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SaturationParams {
    pub drive: f32,
    pub makeup_db: f32,
}

/// Bitcrusher and sample-rate reducer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CrushParams {
    pub bits: f32,
    pub sample_rate_hz: f32,
    pub dither: bool,
}

/// High-pass, low-pass (the "muffle") and low-shelf "warmth".
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterParams {
    pub hpf_enabled: bool,
    pub hpf_hz: f32,
    pub lpf_enabled: bool,
    pub lpf_hz: f32,
    pub lpf_q: f32,
    pub shelf_hz: f32,
    pub shelf_db: f32,
}

/// Freeverb-style reverb.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReverbParams {
    pub room: f32,
    pub damping: f32,
    pub mix: f32,
}

/// Vinyl crackle and hiss.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VinylParams {
    pub pops_per_sec: f32,
    pub pop_db: f32,
    pub hiss_db: f32,
}

/// Output gain ahead of the soft limiter.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputParams {
    pub gain_db: f32,
}

/// Global wet/dry mix (1.0 = fully processed).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MixParams {
    pub wet_dry: f32,
}

/// The complete parameter set of the chain.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Params {
    pub input: InputParams,
    pub tape: TapeParams,
    pub saturation: SaturationParams,
    pub crush: CrushParams,
    pub filters: FilterParams,
    pub reverb: ReverbParams,
    pub vinyl: VinylParams,
    pub output: OutputParams,
    pub mix: MixParams,
}

impl Default for InputParams {
    fn default() -> Self {
        Self { trim_db: 0.0 }
    }
}
impl Default for TapeParams {
    fn default() -> Self {
        Self {
            wow_cents: 0.0,
            wow_hz: 0.5,
            flutter_cents: 0.0,
            flutter_hz: 6.5,
            drift_cents: 0.0,
        }
    }
}
impl Default for SaturationParams {
    fn default() -> Self {
        Self {
            drive: 0.0,
            makeup_db: 0.0,
        }
    }
}
impl Default for CrushParams {
    fn default() -> Self {
        Self {
            bits: 24.0,
            sample_rate_hz: 192_000.0,
            dither: false,
        }
    }
}
impl Default for FilterParams {
    fn default() -> Self {
        Self {
            hpf_enabled: false,
            hpf_hz: 90.0,
            lpf_enabled: false,
            lpf_hz: 4_500.0,
            lpf_q: 0.7,
            shelf_hz: 200.0,
            shelf_db: 0.0,
        }
    }
}
impl Default for ReverbParams {
    fn default() -> Self {
        Self {
            room: 0.5,
            damping: 0.5,
            mix: 0.0,
        }
    }
}
impl Default for VinylParams {
    fn default() -> Self {
        Self {
            pops_per_sec: 0.0,
            pop_db: -34.0,
            hiss_db: -90.0,
        }
    }
}
impl Default for OutputParams {
    fn default() -> Self {
        Self { gain_db: 0.0 }
    }
}
impl Default for MixParams {
    fn default() -> Self {
        Self { wet_dry: 1.0 }
    }
}

fn fix(x: f32, r: &RangeInclusive<f32>, fallback: f32) -> f32 {
    clamp_or(x, *r.start(), *r.end(), fallback)
}

impl Params {
    /// Returns a copy with every value clamped into its valid range; NaN falls back to the default.
    pub fn sanitized(&self) -> Params {
        let d = Params::default();
        Params {
            input: InputParams {
                trim_db: fix(self.input.trim_db, &range::TRIM_DB, d.input.trim_db),
            },
            tape: TapeParams {
                wow_cents: fix(self.tape.wow_cents, &range::WOW_CENTS, d.tape.wow_cents),
                wow_hz: fix(self.tape.wow_hz, &range::WOW_HZ, d.tape.wow_hz),
                flutter_cents: fix(
                    self.tape.flutter_cents,
                    &range::FLUTTER_CENTS,
                    d.tape.flutter_cents,
                ),
                flutter_hz: fix(self.tape.flutter_hz, &range::FLUTTER_HZ, d.tape.flutter_hz),
                drift_cents: fix(self.tape.drift_cents, &range::DRIFT_CENTS, d.tape.drift_cents),
            },
            saturation: SaturationParams {
                drive: fix(self.saturation.drive, &range::DRIVE, d.saturation.drive),
                makeup_db: fix(
                    self.saturation.makeup_db,
                    &range::MAKEUP_DB,
                    d.saturation.makeup_db,
                ),
            },
            crush: CrushParams {
                bits: fix(self.crush.bits, &range::BITS, d.crush.bits),
                sample_rate_hz: fix(
                    self.crush.sample_rate_hz,
                    &range::CRUSH_RATE_HZ,
                    d.crush.sample_rate_hz,
                ),
                dither: self.crush.dither,
            },
            filters: FilterParams {
                hpf_enabled: self.filters.hpf_enabled,
                hpf_hz: fix(self.filters.hpf_hz, &range::HPF_HZ, d.filters.hpf_hz),
                lpf_enabled: self.filters.lpf_enabled,
                lpf_hz: fix(self.filters.lpf_hz, &range::LPF_HZ, d.filters.lpf_hz),
                lpf_q: fix(self.filters.lpf_q, &range::LPF_Q, d.filters.lpf_q),
                shelf_hz: fix(self.filters.shelf_hz, &range::SHELF_HZ, d.filters.shelf_hz),
                shelf_db: fix(self.filters.shelf_db, &range::SHELF_DB, d.filters.shelf_db),
            },
            reverb: ReverbParams {
                room: fix(self.reverb.room, &range::ROOM, d.reverb.room),
                damping: fix(self.reverb.damping, &range::DAMPING, d.reverb.damping),
                mix: fix(self.reverb.mix, &range::REVERB_MIX, d.reverb.mix),
            },
            vinyl: VinylParams {
                pops_per_sec: fix(
                    self.vinyl.pops_per_sec,
                    &range::POPS_PER_SEC,
                    d.vinyl.pops_per_sec,
                ),
                pop_db: fix(self.vinyl.pop_db, &range::POP_DB, d.vinyl.pop_db),
                hiss_db: fix(self.vinyl.hiss_db, &range::HISS_DB, d.vinyl.hiss_db),
            },
            output: OutputParams {
                gain_db: fix(self.output.gain_db, &range::OUTPUT_DB, d.output.gain_db),
            },
            mix: MixParams {
                wet_dry: fix(self.mix.wet_dry, &range::WET_DRY, d.mix.wet_dry),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_within_ranges() {
        let d = Params::default();
        assert_eq!(d, d.sanitized());
    }

    #[test]
    fn sanitize_clamps_and_replaces_nan() {
        let mut p = Params::default();
        p.tape.wow_cents = 1.0e9;
        p.filters.lpf_hz = -5.0;
        p.reverb.mix = f32::NAN;
        p.output.gain_db = f32::INFINITY;
        let s = p.sanitized();
        assert_eq!(s.tape.wow_cents, *range::WOW_CENTS.end());
        assert_eq!(s.filters.lpf_hz, *range::LPF_HZ.start());
        assert_eq!(s.reverb.mix, Params::default().reverb.mix);
        // Non-finite values fall back to the default rather than the range end.
        assert_eq!(s.output.gain_db, Params::default().output.gain_db);
    }

    #[test]
    fn partial_toml_uses_defaults() {
        let p: Params = toml::from_str("[reverb]\nmix = 0.3\n").unwrap();
        assert_eq!(p.reverb.mix, 0.3);
        assert_eq!(p.reverb.room, Params::default().reverb.room);
        assert_eq!(p.filters, Params::default().filters);
    }

    #[test]
    fn toml_roundtrip() {
        let mut p = Params::default();
        p.crush.bits = 12.0;
        p.crush.dither = true;
        p.filters.lpf_enabled = true;
        let text = toml::to_string_pretty(&p).unwrap();
        let back: Params = toml::from_str(&text).unwrap();
        assert_eq!(p, back);
    }
}
