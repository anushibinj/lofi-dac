//! Lock-free parameter sharing between the UI/control threads and the audio thread.
//!
//! Every parameter lives in an `AtomicU32` holding the bits of an `f32` (booleans are 0.0/1.0).
//! Writers store individual values; the audio thread loads a whole [`Params`] once per block.
//! A torn read across two parameters is harmless because every parameter is smoothed.

use crate::params::{
    CrushParams, FilterParams, InputParams, MixParams, OutputParams, Params, ReverbParams,
    SaturationParams, TapeParams, VinylParams,
};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const N: usize = 26;

fn b(x: bool) -> f32 {
    if x { 1.0 } else { 0.0 }
}

impl Params {
    /// Flattens the parameters into a fixed-order array (booleans as 0.0 / 1.0).
    fn to_array(self) -> [f32; N] {
        [
            self.input.trim_db,
            self.tape.wow_cents,
            self.tape.wow_hz,
            self.tape.flutter_cents,
            self.tape.flutter_hz,
            self.tape.drift_cents,
            self.saturation.drive,
            self.saturation.makeup_db,
            self.crush.bits,
            self.crush.sample_rate_hz,
            b(self.crush.dither),
            b(self.filters.hpf_enabled),
            self.filters.hpf_hz,
            b(self.filters.lpf_enabled),
            self.filters.lpf_hz,
            self.filters.lpf_q,
            self.filters.shelf_hz,
            self.filters.shelf_db,
            self.reverb.room,
            self.reverb.damping,
            self.reverb.mix,
            self.vinyl.pops_per_sec,
            self.vinyl.pop_db,
            self.vinyl.hiss_db,
            self.output.gain_db,
            self.mix.wet_dry,
        ]
    }

    /// Inverse of [`Params::to_array`].
    fn from_array(a: &[f32; N]) -> Params {
        Params {
            input: InputParams { trim_db: a[0] },
            tape: TapeParams {
                wow_cents: a[1],
                wow_hz: a[2],
                flutter_cents: a[3],
                flutter_hz: a[4],
                drift_cents: a[5],
            },
            saturation: SaturationParams {
                drive: a[6],
                makeup_db: a[7],
            },
            crush: CrushParams {
                bits: a[8],
                sample_rate_hz: a[9],
                dither: a[10] >= 0.5,
            },
            filters: FilterParams {
                hpf_enabled: a[11] >= 0.5,
                hpf_hz: a[12],
                lpf_enabled: a[13] >= 0.5,
                lpf_hz: a[14],
                lpf_q: a[15],
                shelf_hz: a[16],
                shelf_db: a[17],
            },
            reverb: ReverbParams {
                room: a[18],
                damping: a[19],
                mix: a[20],
            },
            vinyl: VinylParams {
                pops_per_sec: a[21],
                pop_db: a[22],
                hiss_db: a[23],
            },
            output: OutputParams { gain_db: a[24] },
            mix: MixParams { wet_dry: a[25] },
        }
    }
}

/// Atomic parameter block plus the bypass flag.
#[derive(Debug)]
pub struct SharedParams {
    slots: [AtomicU32; N],
    bypass: AtomicBool,
}

impl SharedParams {
    /// Creates a block initialised with `params` (sanitised) and bypass off.
    pub fn new(params: &Params) -> Self {
        let a = params.sanitized().to_array();
        Self {
            slots: std::array::from_fn(|i| AtomicU32::new(a[i].to_bits())),
            bypass: AtomicBool::new(false),
        }
    }

    /// Publishes a full parameter set (sanitised first).
    pub fn store(&self, params: &Params) {
        let a = params.sanitized().to_array();
        for (slot, v) in self.slots.iter().zip(a) {
            slot.store(v.to_bits(), Ordering::Relaxed);
        }
    }

    /// Reads the current parameter set. Allocation free; safe on the audio thread.
    pub fn load(&self) -> Params {
        let mut a = [0.0f32; N];
        for (v, slot) in a.iter_mut().zip(&self.slots) {
            *v = f32::from_bits(slot.load(Ordering::Relaxed));
        }
        Params::from_array(&a)
    }

    /// Enables or disables bypass (a 20 ms crossfade to the delay-compensated dry signal).
    pub fn set_bypass(&self, bypass: bool) {
        self.bypass.store(bypass, Ordering::Relaxed);
    }

    /// Current bypass state.
    pub fn bypass(&self) -> bool {
        self.bypass.load(Ordering::Relaxed)
    }
}

impl Default for SharedParams {
    fn default() -> Self {
        Self::new(&Params::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    fn random_params(rng: &mut Rng) -> Params {
        let mut a = [0.0f32; N];
        for v in &mut a {
            *v = rng.uniform();
        }
        // Booleans must be exactly 0/1 to round-trip.
        for i in [10, 11, 13] {
            a[i] = if a[i] > 0.5 { 1.0 } else { 0.0 };
        }
        Params::from_array(&a)
    }

    #[test]
    fn array_roundtrip_covers_every_field() {
        let mut rng = Rng::new(3);
        for _ in 0..50 {
            let p = random_params(&mut rng);
            assert_eq!(Params::from_array(&p.to_array()), p);
        }
    }

    #[test]
    fn every_slot_is_distinct() {
        // Guards against two fields being written to the same slot (transposition bugs).
        let mut rng = Rng::new(9);
        let a = random_params(&mut rng).to_array();
        for i in 0..N {
            for j in (i + 1)..N {
                if i != j && ![10, 11, 13].contains(&i) && ![10, 11, 13].contains(&j) {
                    assert_ne!(a[i], a[j]);
                }
            }
        }
    }

    #[test]
    fn store_and_load_roundtrip() {
        let shared = SharedParams::default();
        let mut p = Params::default();
        p.tape.wow_cents = 8.0;
        p.crush.dither = true;
        p.filters.lpf_enabled = true;
        p.vinyl.hiss_db = -50.0;
        shared.store(&p);
        assert_eq!(shared.load(), p);
    }

    #[test]
    fn store_sanitises() {
        let shared = SharedParams::default();
        let mut p = Params::default();
        p.reverb.mix = 99.0;
        shared.store(&p);
        assert_eq!(shared.load().reverb.mix, 1.0);
    }

    #[test]
    fn bypass_flag() {
        let shared = SharedParams::default();
        assert!(!shared.bypass());
        shared.set_bypass(true);
        assert!(shared.bypass());
    }
}
