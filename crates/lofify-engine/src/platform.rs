//! Picks the audio backend for the current operating system.

use crate::backend::AudioBackend;
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
use crate::error::EngineError;
use crate::error::Result;
use crate::types::DeviceList;

/// Creates the platform backend: WASAPI (via cpal) on Windows, Core Audio on macOS.
///
/// Call it on the thread that will use it; backends are not `Send`.
pub fn default_backend() -> Result<Box<dyn AudioBackend>> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        Ok(Box::new(crate::cpal_backend::CpalBackend::new()))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Err(EngineError::Unsupported(
            "Lofify supports Windows and macOS only (run with --simulate to try the engine without hardware)".into(),
        ))
    }
}

/// Lists all audio devices using a temporary backend on the calling thread.
pub fn list_devices() -> Result<DeviceList> {
    default_backend()?.list_devices()
}

/// True on platforms that have a real backend.
pub const fn is_supported() -> bool {
    cfg!(any(target_os = "windows", target_os = "macos"))
}
