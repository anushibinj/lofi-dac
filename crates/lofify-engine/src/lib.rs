//! Audio I/O engine for Lofify: capture, ring buffer, DSP, output.

pub mod backend;
pub mod capture;
pub mod engine;
pub mod error;
pub mod feedback;
pub mod mapping;
pub mod metrics;
pub mod output;
pub mod pcm;
pub mod platform;
pub mod resample;
pub mod sim;
pub mod types;
