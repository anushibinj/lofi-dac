//! The abstraction between the engine and a platform audio API.
//!
//! A backend lists devices and opens capture and output streams. Streams stop when the returned
//! box is dropped. Backends never touch DSP or ring buffers directly: they feed device buffers
//! to a [`CaptureWriter`] and ask an [`OutputRenderer`] to fill device buffers.

use crate::capture::CaptureWriter;
use crate::error::Result;
use crate::output::OutputRenderer;
use crate::types::{
    BufferRequest, CaptureTarget, DeviceInfo, DeviceList, OutputFormat, StreamInfo,
};

/// A running capture stream. Dropping it stops capture.
pub trait CaptureSource {
    /// Negotiated stream format.
    fn info(&self) -> &StreamInfo;
}

/// A running output stream. Dropping it stops playback.
pub trait OutputSink {
    /// Negotiated stream format.
    fn info(&self) -> &StreamInfo;
}

/// Builds the renderer once the backend knows the output stream's real format. Called on the
/// control thread, before the stream's callback exists, so it may allocate.
pub type RendererFactory = Box<dyn FnOnce(OutputFormat) -> Result<OutputRenderer> + Send>;

/// What the engine asks of an output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputRequest {
    /// Preferred sample rate (the capture rate), used when the device can convert to it.
    pub preferred_rate: u32,
    /// Requested device buffer size.
    pub buffer: BufferRequest,
}

/// A platform audio API.
///
/// Backends are created on, and never leave, the engine's control thread, so they need not be
/// `Send` (cpal streams are not).
pub trait AudioBackend {
    /// Short name for logs, e.g. "wasapi".
    fn name(&self) -> &'static str;

    /// Lists every visible endpoint, flagging the system defaults.
    fn list_devices(&self) -> Result<DeviceList>;

    /// True if `target` can only be captured after the output stream exists. The macOS system
    /// tap must exclude Lofify's own process from the tap, and the process only has a Core Audio
    /// object once it is playing audio. For such targets the engine opens the output first and
    /// takes the capture rate from `target.device.sample_rate`.
    fn capture_needs_output_first(&self, _target: &CaptureTarget) -> bool {
        false
    }

    /// Starts capturing `target`, feeding every device buffer to `writer`.
    fn open_capture(
        &self,
        target: &CaptureTarget,
        writer: CaptureWriter,
    ) -> Result<Box<dyn CaptureSource>>;

    /// Starts playback on `device`. The backend negotiates a format (ideally
    /// `request.preferred_rate`), calls `factory` with it, and drives the returned renderer from
    /// the device callback.
    fn open_output(
        &self,
        device: &DeviceInfo,
        request: OutputRequest,
        factory: RendererFactory,
    ) -> Result<Box<dyn OutputSink>>;
}
