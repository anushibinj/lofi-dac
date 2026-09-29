//! Audio I/O engine for Lofify: capture, ring buffer, DSP, output.

pub mod backend;
pub mod capture;
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub mod cpal_backend;
pub mod engine;
pub mod error;
pub mod feedback;
#[cfg(target_os = "macos")]
pub mod mac_tap;
pub mod mapping;
pub mod metrics;
pub mod output;
pub mod pcm;
pub mod platform;
pub mod resample;
pub mod sim;
pub mod types;
