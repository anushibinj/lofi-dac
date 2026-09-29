//! Pure DSP for Lofify: a real-time lofi effects chain with no audio I/O.
#![forbid(unsafe_code)]

pub mod biquad;
pub mod params;
pub mod rng;
pub mod shared;
pub mod tape;
pub mod util;
