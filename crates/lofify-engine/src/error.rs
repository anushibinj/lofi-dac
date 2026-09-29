//! Engine error type.

/// Everything that can go wrong while setting up or running the audio engine.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    /// There is no audio backend for this operating system.
    #[error("no audio backend is available on this platform: {0}")]
    Unsupported(String),
    /// The platform audio API reported a problem.
    #[error("audio backend error: {0}")]
    Backend(String),
    /// A named or default device does not exist (any more).
    #[error("audio device not found: {0}")]
    DeviceNotFound(String),
    /// Auto-detection found no virtual audio cable input.
    #[error("no virtual audio cable found (install VB-Cable, see docs/SETUP_WINDOWS.md)")]
    NoVirtualCable,
    /// The system has no default playback device.
    #[error("there is no default output device")]
    NoDefaultOutput,
    /// Starting would create an audio feedback loop.
    #[error("refusing to start to avoid a feedback loop: {0}")]
    Feedback(String),
    /// The device cannot deliver the format the engine needs.
    #[error("audio format not supported: {0}")]
    UnsupportedFormat(String),
    /// The operating system denied access to audio capture.
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    /// The control thread is gone.
    #[error("the audio engine has shut down")]
    EngineStopped,
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, EngineError>;
