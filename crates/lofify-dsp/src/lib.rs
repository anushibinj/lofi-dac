//! Pure DSP for Lofify: a real-time lofi effects chain with no audio I/O.
#![forbid(unsafe_code)]

pub mod biquad;
pub mod chain;
pub mod crusher;
pub mod filters;
pub mod limiter;
pub mod params;
pub mod presets;
pub mod reverb;
pub mod rng;
pub mod saturation;
pub mod shared;
pub mod tape;
pub mod util;
pub mod vinyl;
