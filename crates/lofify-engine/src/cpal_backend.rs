//! cpal backend: WASAPI on Windows, Core Audio on macOS.
//!
//! **UNTESTED on real hardware** (this code was written and compile-checked on Linux with
//! `cargo check --target x86_64-pc-windows-msvc` / `aarch64-apple-darwin`).
//!
//! Notes that shaped this file (verified against the cpal 0.18 source):
//!
//! * On WASAPI, recording from a *playback* endpoint transparently switches on loopback. Such
//!   endpoints report no input configs, so the loopback stream config is built from
//!   `default_output_config()`.
//! * WASAPI shared-mode capture only accepts the device's native (mix) format, so capture always
//!   uses the device default. Output streams get `AUTOCONVERTPCM`, so any listed rate works.
//! * Streams are opened from specific endpoints (looked up by id), never from cpal's "default
//!   device" handles, so the engine alone decides when to follow a changed default.
//! * cpal calls into COM and can panic inside `description()` if a device vanishes mid-query, so
//!   every call into cpal is wrapped in `catch_unwind`, and so are the audio callbacks.

use crate::backend::{AudioBackend, CaptureSource, OutputRequest, OutputSink, RendererFactory};
use crate::capture::CaptureWriter;
use crate::error::{EngineError, Result};
use crate::metrics::{ERR_CAPTURE, ERR_OUTPUT, Shared};
use crate::output::OutputRenderer;
use crate::pcm::{Pcm, clamp_unit};
use crate::types::{
    BufferRequest, CaptureTarget, DeviceInfo, DeviceList, Direction, OutputFormat, StreamInfo,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Data, ErrorKind, InputCallbackInfo, OutputCallbackInfo, Sample, SampleFormat,
    SizedSample, StreamConfig, SupportedBufferSize,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

impl Pcm for cpal::I24 {
    #[inline]
    fn to_f32(self) -> f32 {
        Sample::to_sample::<f32>(self)
    }
    #[inline]
    fn from_f32(v: f32) -> Self {
        Sample::from_sample(clamp_unit(v))
    }
}

impl Pcm for cpal::U24 {
    #[inline]
    fn to_f32(self) -> f32 {
        Sample::to_sample::<f32>(self)
    }
    #[inline]
    fn from_f32(v: f32) -> Self {
        Sample::from_sample(clamp_unit(v))
    }
}

/// The platform backend built on cpal.
pub struct CpalBackend {
    host: cpal::Host,
}

impl std::fmt::Debug for CpalBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CpalBackend").finish_non_exhaustive()
    }
}

impl Default for CpalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CpalBackend {
    /// Connects to the default host (WASAPI / Core Audio).
    pub fn new() -> Self {
        Self {
            host: cpal::default_host(),
        }
    }

    fn find_device(&self, id: &str) -> Result<cpal::Device> {
        let devices = self.host.devices().map_err(map_error)?;
        for d in devices {
            if d.id().ok().map(|i| i.to_string()).as_deref() == Some(id) {
                return Ok(d);
            }
        }
        Err(EngineError::DeviceNotFound(id.to_string()))
    }
}

fn map_error(e: cpal::Error) -> EngineError {
    let message = e.to_string();
    match e.kind() {
        ErrorKind::DeviceNotAvailable => EngineError::DeviceNotFound(message),
        ErrorKind::PermissionDenied => EngineError::PermissionDenied(message),
        ErrorKind::UnsupportedConfig | ErrorKind::UnsupportedOperation => {
            EngineError::UnsupportedFormat(message)
        }
        _ => EngineError::Backend(message),
    }
}

/// Runs `f`, turning a panic inside the audio library into an error.
fn guard<T>(what: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        Err(EngineError::Backend(format!(
            "{what} failed inside the audio library"
        )))
    })
}

fn is_usable_format(f: SampleFormat) -> bool {
    matches!(
        f,
        SampleFormat::F32
            | SampleFormat::I16
            | SampleFormat::I32
            | SampleFormat::F64
            | SampleFormat::U16
            | SampleFormat::I8
            | SampleFormat::U8
            | SampleFormat::I24
            | SampleFormat::U24
    )
}

/// Lower is better: float, then 32-bit, then 16-bit.
fn format_rank(f: SampleFormat) -> u8 {
    match f {
        SampleFormat::F32 => 0,
        SampleFormat::I32 => 1,
        SampleFormat::I16 => 2,
        SampleFormat::F64 => 3,
        _ => 4,
    }
}

fn error_handler(
    shared: Arc<Shared>,
    flag: u32,
    label: &'static str,
) -> impl FnMut(cpal::Error) + Send + 'static {
    move |e| match e.kind() {
        // Informational: the stream keeps running.
        ErrorKind::DeviceChanged | ErrorKind::RealtimeDenied | ErrorKind::Xrun => {
            tracing::debug!("{label} stream notice: {e}");
        }
        _ => {
            tracing::warn!("{label} stream error: {e}");
            shared.report_error(flag, &e.to_string());
        }
    }
}

fn write_as<T: Pcm + SizedSample>(data: &Data, writer: &mut CaptureWriter, channels: usize) {
    if let Some(samples) = data.as_slice::<T>() {
        writer.write(samples, channels);
    }
}

fn render_as<T: Pcm + SizedSample>(
    data: &mut Data,
    renderer: &mut OutputRenderer,
    channels: usize,
) {
    if let Some(samples) = data.as_slice_mut::<T>() {
        renderer.render(samples, channels);
    }
}

/// A running cpal capture stream.
struct CpalCapture {
    info: StreamInfo,
    _stream: cpal::Stream,
}

/// A running cpal output stream.
struct CpalOutput {
    info: StreamInfo,
    _stream: cpal::Stream,
}

impl CaptureSource for CpalCapture {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
}
impl OutputSink for CpalOutput {
    fn info(&self) -> &StreamInfo {
        &self.info
    }
}

impl AudioBackend for CpalBackend {
    fn name(&self) -> &'static str {
        if cfg!(target_os = "windows") {
            "wasapi"
        } else {
            "coreaudio"
        }
    }

    fn list_devices(&self) -> Result<DeviceList> {
        guard("device enumeration", || {
            let id_of =
                |d: Option<cpal::Device>| d.and_then(|d| d.id().ok()).map(|i| i.to_string());
            let default_in = id_of(self.host.default_input_device());
            let default_out = id_of(self.host.default_output_device());
            let mut list = DeviceList::default();
            for d in self.host.devices().map_err(map_error)? {
                let Ok(id) = d.id() else { continue };
                let Ok(description) = d.description() else {
                    continue;
                };
                let id = id.to_string();
                let name = description.name().to_string();
                if d.supports_input() {
                    let mut info = DeviceInfo::new(id.clone(), name.clone(), Direction::Input);
                    info.is_default = default_in.as_deref() == Some(id.as_str());
                    if let Ok(cfg) = d.default_input_config() {
                        info.sample_rate = Some(cfg.sample_rate());
                        info.channels = Some(cfg.channels());
                    }
                    list.inputs.push(info);
                }
                if d.supports_output() {
                    let mut info = DeviceInfo::new(id.clone(), name, Direction::Output);
                    info.is_default = default_out.as_deref() == Some(id.as_str());
                    if let Ok(cfg) = d.default_output_config() {
                        info.sample_rate = Some(cfg.sample_rate());
                        info.channels = Some(cfg.channels());
                    }
                    list.outputs.push(info);
                }
            }
            #[cfg(target_os = "macos")]
            if crate::mac_tap::supported() {
                // Clocked by the default output device, so it delivers at that device's rate.
                let rate = list.default_output().and_then(|d| d.sample_rate);
                list.inputs.push(crate::types::system_audio_device(rate));
            }
            Ok(list)
        })
    }

    fn open_capture(
        &self,
        target: &CaptureTarget,
        writer: CaptureWriter,
    ) -> Result<Box<dyn CaptureSource>> {
        guard("opening the capture stream", || {
            let device = self.find_device(&target.device.id)?;
            // Loopback: a playback endpoint recorded through WASAPI loopback has no input
            // configs, so use its output (mix) format.
            let supported = if target.loopback {
                device.default_output_config()
            } else {
                device.default_input_config()
            }
            .map_err(map_error)?;
            let format = supported.sample_format();
            if !is_usable_format(format) {
                return Err(EngineError::UnsupportedFormat(format!(
                    "capture sample format {format:?}"
                )));
            }
            let config = StreamConfig {
                channels: supported.channels(),
                sample_rate: supported.sample_rate(),
                buffer_size: BufferSize::Default,
            };
            let channels = usize::from(config.channels);
            let shared = writer.shared().clone();
            let mut writer = writer;
            let stream = device
                .build_input_stream_raw(
                    config,
                    format,
                    move |data: &Data, info: &InputCallbackInfo| {
                        let _ = catch_unwind(AssertUnwindSafe(|| {
                            let ts = info.timestamp();
                            writer.set_device_latency(
                                ts.callback.saturating_duration_since(ts.capture),
                            );
                            match data.sample_format() {
                                SampleFormat::F32 => write_as::<f32>(data, &mut writer, channels),
                                SampleFormat::I16 => write_as::<i16>(data, &mut writer, channels),
                                SampleFormat::I32 => write_as::<i32>(data, &mut writer, channels),
                                SampleFormat::F64 => write_as::<f64>(data, &mut writer, channels),
                                SampleFormat::U16 => write_as::<u16>(data, &mut writer, channels),
                                SampleFormat::I8 => write_as::<i8>(data, &mut writer, channels),
                                SampleFormat::U8 => write_as::<u8>(data, &mut writer, channels),
                                SampleFormat::I24 => {
                                    write_as::<cpal::I24>(data, &mut writer, channels)
                                }
                                SampleFormat::U24 => {
                                    write_as::<cpal::U24>(data, &mut writer, channels)
                                }
                                _ => {}
                            }
                        }));
                    },
                    error_handler(shared, ERR_CAPTURE, "capture"),
                    None,
                )
                .map_err(map_error)?;
            stream.play().map_err(map_error)?;
            let info = StreamInfo {
                device_name: target.device.name.clone(),
                device_id: target.device.id.clone(),
                sample_rate: config.sample_rate,
                channels: config.channels,
                buffer_frames: stream.buffer_size().ok(),
            };
            Ok(Box::new(CpalCapture {
                info,
                _stream: stream,
            }) as Box<dyn CaptureSource>)
        })
    }

    fn open_output(
        &self,
        device_info: &DeviceInfo,
        request: OutputRequest,
        factory: RendererFactory,
    ) -> Result<Box<dyn OutputSink>> {
        guard("opening the output stream", || {
            let device = self.find_device(&device_info.id)?;
            let default = device.default_output_config().map_err(map_error)?;
            let channels = default.channels();

            // Prefer the capture rate so no rate conversion is needed; WASAPI lists the common
            // rates it can convert to. Otherwise use the device's own rate.
            let ranges: Vec<_> = device
                .supported_output_configs()
                .map(Iterator::collect)
                .unwrap_or_default();
            let best = ranges
                .iter()
                .filter(|r| {
                    r.channels() == channels
                        && r.contains_rate(request.preferred_rate)
                        && is_usable_format(r.sample_format())
                })
                .min_by_key(|r| format_rank(r.sample_format()));
            let (rate, format, buffer_range) = match best {
                Some(r) => (request.preferred_rate, r.sample_format(), *r.buffer_size()),
                None => (
                    default.sample_rate(),
                    default.sample_format(),
                    *default.buffer_size(),
                ),
            };
            if !is_usable_format(format) {
                return Err(EngineError::UnsupportedFormat(format!(
                    "output sample format {format:?}"
                )));
            }
            let buffer_size = match (request.buffer, buffer_range) {
                (BufferRequest::Frames(n), SupportedBufferSize::Range { min, max })
                    if (min..=max).contains(&n) =>
                {
                    BufferSize::Fixed(n)
                }
                _ => BufferSize::Default,
            };
            let config = StreamConfig {
                channels,
                sample_rate: rate,
                buffer_size,
            };

            let mut renderer = factory(OutputFormat {
                sample_rate: rate,
                channels,
            })?;
            let shared = renderer.shared().clone();
            let ch = usize::from(channels);
            let stream = device
                .build_output_stream_raw(
                    config,
                    format,
                    move |data: &mut Data, info: &OutputCallbackInfo| {
                        // A panic must never unwind into the OS audio thread. The buffer arrives
                        // pre-filled with silence, so on failure the listener just hears nothing.
                        let _ = catch_unwind(AssertUnwindSafe(|| {
                            let ts = info.timestamp();
                            renderer.set_device_latency(
                                ts.playback.saturating_duration_since(ts.callback),
                            );
                            match data.sample_format() {
                                SampleFormat::F32 => render_as::<f32>(data, &mut renderer, ch),
                                SampleFormat::I16 => render_as::<i16>(data, &mut renderer, ch),
                                SampleFormat::I32 => render_as::<i32>(data, &mut renderer, ch),
                                SampleFormat::F64 => render_as::<f64>(data, &mut renderer, ch),
                                SampleFormat::U16 => render_as::<u16>(data, &mut renderer, ch),
                                SampleFormat::I8 => render_as::<i8>(data, &mut renderer, ch),
                                SampleFormat::U8 => render_as::<u8>(data, &mut renderer, ch),
                                SampleFormat::I24 => {
                                    render_as::<cpal::I24>(data, &mut renderer, ch)
                                }
                                SampleFormat::U24 => {
                                    render_as::<cpal::U24>(data, &mut renderer, ch)
                                }
                                _ => {}
                            }
                        }));
                    },
                    error_handler(shared, ERR_OUTPUT, "output"),
                    None,
                )
                .map_err(map_error)?;
            stream.play().map_err(map_error)?;
            let info = StreamInfo {
                device_name: device_info.name.clone(),
                device_id: device_info.id.clone(),
                sample_rate: rate,
                channels,
                buffer_frames: stream.buffer_size().ok(),
            };
            Ok(Box::new(CpalOutput {
                info,
                _stream: stream,
            }) as Box<dyn OutputSink>)
        })
    }
}
