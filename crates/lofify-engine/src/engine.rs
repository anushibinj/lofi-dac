//! The engine: a control thread that owns the platform backend and both streams.
//!
//! The UI talks to it through a command channel and reads back status through atomics and one
//! small mutex. The control thread
//!
//! * resolves the selected devices and applies feedback protection before opening anything,
//! * opens capture, then output (the output callback is the clock master),
//! * polls the device list every second or two and rebuilds the streams when a device
//!   disappears, the default output changes, or a stream reports an error,
//! * keeps retrying while devices are missing, and never crashes on device loss.
//!
//! cpal streams are not `Send`, so the backend is created *inside* the control thread from a
//! factory closure and never leaves it.

use crate::backend::{AudioBackend, CaptureSource, OutputRequest, OutputSink, RendererFactory};
use crate::capture::CaptureWriter;
use crate::error::{EngineError, Result};
use crate::feedback::check_feedback;
use crate::metrics::{Metrics, Shared};
use crate::output::{OutputRenderer, RendererConfig};
use crate::types::{
    BufferRequest, CaptureSelection, CaptureTarget, DeviceConfig, DeviceInfo, DeviceList,
    OutputSelection, StreamInfo,
};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use lofify_dsp::chain::ChainConfig;
use lofify_dsp::shared::SharedParams;
use rtrb::RingBuffer;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Ring capacity in frames: 250 ms at the highest supported rate (192 kHz).
const RING_FRAMES: usize = 48_000;

/// Engine tuning knobs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EngineConfig {
    /// Capture audio buffered before playback starts (about 20 ms).
    pub prefill_ms: f32,
    /// Buffered audio above which the oldest frames are dropped.
    pub max_fill_ms: f32,
    /// Fade-in when the engine starts.
    pub startup_fade_ms: f32,
    /// Fade-in whenever audio resumes after a gap.
    pub resume_fade_ms: f32,
    /// Nudge the resampling ratio to absorb clock drift between the two devices.
    pub drift_compensation: bool,
    /// Tape base delay of the DSP chain, which is also its fixed latency (10 ms by default).
    pub base_delay_ms: f32,
    /// Requested output device buffer size.
    pub buffer: BufferRequest,
    /// How often devices are polled for changes.
    pub poll_interval: Duration,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            prefill_ms: 20.0,
            max_fill_ms: 120.0,
            startup_fade_ms: 50.0,
            resume_fade_ms: 5.0,
            drift_compensation: true,
            base_delay_ms: 10.0,
            buffer: BufferRequest::Default,
            poll_interval: Duration::from_millis(1500),
        }
    }
}

impl EngineConfig {
    /// Trades robustness for latency: 10 ms prefill and a 3 ms tape delay. Needs stable devices.
    pub fn low_latency() -> Self {
        Self {
            prefill_ms: 10.0,
            base_delay_ms: 3.0,
            ..Self::default()
        }
    }
}

/// What the engine is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineState {
    /// Not started, or stopped by the user.
    Stopped,
    /// Opening streams.
    Starting,
    /// Audio is flowing.
    Running,
    /// Started, but devices are missing; retrying automatically.
    Waiting(String),
    /// Started, but something must change first (e.g. feedback protection refused).
    Error(String),
}

#[derive(Debug)]
struct StatusInner {
    state: EngineState,
    capture: Option<StreamInfo>,
    output: Option<StreamInfo>,
    restarts: u32,
    backend: String,
}

/// A consistent view of the engine for display.
#[derive(Debug, Clone)]
pub struct Status {
    pub state: EngineState,
    pub capture: Option<StreamInfo>,
    pub output: Option<StreamInfo>,
    /// Number of automatic stream rebuilds since the engine was created.
    pub restarts: u32,
    /// Live counters and meters.
    pub metrics: Metrics,
    /// Backend name, e.g. "wasapi".
    pub backend: String,
}

#[derive(Debug)]
enum Command {
    Start,
    Stop,
    SetDevices(DeviceConfig),
    Refresh,
    Shutdown,
}

/// Handle to the running engine.
#[derive(Debug)]
pub struct Engine {
    tx: Sender<Command>,
    status: Arc<Mutex<StatusInner>>,
    shared: Arc<Shared>,
    params: Arc<SharedParams>,
    thread: Option<JoinHandle<()>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Engine {
    /// Spawns the control thread. `make_backend` runs on that thread so the backend (and the
    /// streams it creates) never has to be `Send`.
    pub fn spawn<F>(make_backend: F, config: EngineConfig, params: Arc<SharedParams>) -> Engine
    where
        F: FnOnce() -> Result<Box<dyn AudioBackend>> + Send + 'static,
    {
        let (tx, rx) = unbounded();
        let shared = Arc::new(Shared::default());
        let status = Arc::new(Mutex::new(StatusInner {
            state: EngineState::Stopped,
            capture: None,
            output: None,
            restarts: 0,
            backend: String::new(),
        }));
        let (t_shared, t_status, t_params) = (shared.clone(), status.clone(), params.clone());
        let thread = thread::Builder::new()
            .name("lofify-engine".into())
            .spawn(move || {
                let mut controller = Controller {
                    backend: None,
                    backend_error: None,
                    config,
                    params: t_params,
                    shared: t_shared,
                    status: t_status,
                    devices: DeviceConfig::default(),
                    wanted: false,
                    session: None,
                    last_message: None,
                    retry_not_before: None,
                };
                match make_backend() {
                    Ok(b) => {
                        lock(&controller.status).backend = b.name().to_string();
                        controller.backend = Some(b);
                    }
                    Err(e) => {
                        tracing::error!("audio backend unavailable: {e}");
                        controller.backend_error = Some(e);
                    }
                }
                controller.run(&rx);
            })
            .ok();
        Engine {
            tx,
            status,
            shared,
            params,
            thread,
        }
    }

    fn send(&self, command: Command) {
        // A closed channel means the control thread is gone; there is nothing useful to do.
        let _ = self.tx.send(command);
    }

    /// Starts (or resumes) processing. Returns immediately; watch [`Engine::status`].
    pub fn start(&self) {
        self.send(Command::Start);
    }

    /// Stops processing and releases both devices.
    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    /// Chooses the capture source and output device, restarting the streams if running.
    pub fn set_devices(&self, devices: DeviceConfig) {
        self.send(Command::SetDevices(devices));
    }

    /// Re-checks devices right now instead of waiting for the next poll.
    pub fn refresh(&self) {
        self.send(Command::Refresh);
    }

    /// Current state, stream formats and metrics.
    pub fn status(&self) -> Status {
        let s = lock(&self.status);
        Status {
            state: s.state.clone(),
            capture: s.capture.clone(),
            output: s.output.clone(),
            restarts: s.restarts,
            metrics: self.shared.snapshot(),
            backend: s.backend.clone(),
        }
    }

    /// Live metrics only (cheap, lock free).
    pub fn metrics(&self) -> Metrics {
        self.shared.snapshot()
    }

    /// The shared parameter block the audio thread reads. Write to it to change effects live.
    pub fn params(&self) -> &Arc<SharedParams> {
        &self.params
    }

    /// Stops everything and joins the control thread.
    pub fn shutdown(mut self) {
        self.close();
    }

    fn close(&mut self) {
        self.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.close();
    }
}

/// Everything that exists only while streams are running. Field order matters: the output stream
/// must stop before the capture stream, so it is declared first and dropped first.
struct Session {
    _output: Box<dyn OutputSink>,
    _capture: Box<dyn CaptureSource>,
    target: CaptureTarget,
    output_device: DeviceInfo,
}

impl Session {
    /// Why the streams should be rebuilt given the current device list, if they should.
    fn stale_reason(&self, list: &DeviceList, devices: &DeviceConfig) -> Option<String> {
        if !list.contains(&self.target.device.id, self.target.device.direction) {
            return Some(format!(
                "capture device \"{}\" disappeared",
                self.target.device.name
            ));
        }
        if !list.contains(&self.output_device.id, self.output_device.direction) {
            return Some(format!(
                "output device \"{}\" disappeared",
                self.output_device.name
            ));
        }
        if devices.output == OutputSelection::Default
            && list.default_output().map(|d| d.id.as_str()) != Some(self.output_device.id.as_str())
        {
            return Some("the default output device changed".to_string());
        }
        if matches!(&devices.capture, CaptureSelection::Loopback(n) if n.trim().is_empty())
            && list.default_output().map(|d| d.id.as_str()) != Some(self.target.device.id.as_str())
        {
            return Some("the default output device changed (loopback source)".to_string());
        }
        None
    }
}

struct Controller {
    backend: Option<Box<dyn AudioBackend>>,
    backend_error: Option<EngineError>,
    config: EngineConfig,
    params: Arc<SharedParams>,
    shared: Arc<Shared>,
    status: Arc<Mutex<StatusInner>>,
    devices: DeviceConfig,
    wanted: bool,
    session: Option<Session>,
    /// Last state message, to avoid logging the same problem every poll.
    last_message: Option<String>,
    /// After a problem that needs the user (an `Error` state), do not retry before this time.
    /// Retrying every poll would, for example, keep re-triggering a pending macOS permission
    /// prompt.
    retry_not_before: Option<Instant>,
}

/// How long to wait before retrying after an `Error` state.
const ERROR_RETRY: Duration = Duration::from_secs(5);

impl Controller {
    fn run(&mut self, rx: &Receiver<Command>) {
        loop {
            match rx.recv_timeout(self.config.poll_interval) {
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(command) => self.handle(command),
                Err(RecvTimeoutError::Timeout) => self.tick(),
            }
        }
        self.end_session();
        self.set_state(EngineState::Stopped);
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Start => {
                self.wanted = true;
                if self.session.is_none() {
                    self.shared.reset_counters();
                    self.try_start();
                }
            }
            Command::Stop => {
                self.wanted = false;
                self.end_session();
                self.set_state(EngineState::Stopped);
            }
            Command::SetDevices(devices) => {
                if devices != self.devices {
                    self.devices = devices;
                    if self.wanted {
                        self.end_session();
                        self.try_start();
                    }
                }
            }
            Command::Refresh => self.tick(),
            Command::Shutdown => {}
        }
    }

    fn end_session(&mut self) {
        self.session = None;
        let mut s = lock(&self.status);
        s.capture = None;
        s.output = None;
    }

    fn set_state(&mut self, state: EngineState) {
        lock(&self.status).state = state;
    }

    fn try_start(&mut self) {
        self.set_state(EngineState::Starting);
        let result = match &self.backend {
            Some(backend) => start_session(
                backend.as_ref(),
                &self.devices,
                &self.config,
                &self.params,
                &self.shared,
            ),
            None => Err(self
                .backend_error
                .clone()
                .unwrap_or_else(|| EngineError::Unsupported("no backend".into()))),
        };
        match result {
            Ok(session) => {
                {
                    let mut s = lock(&self.status);
                    s.capture = Some(session_info(&session, true));
                    s.output = Some(session_info(&session, false));
                }
                let (c, o) = (
                    session.target.device.name.clone(),
                    session.output_device.name.clone(),
                );
                tracing::info!("running: \"{c}\" -> \"{o}\"");
                self.last_message = None;
                self.retry_not_before = None;
                self.session = Some(session);
                self.set_state(EngineState::Running);
            }
            Err(e) => {
                let message = e.to_string();
                if self.last_message.as_deref() != Some(message.as_str()) {
                    tracing::warn!("cannot start: {message}");
                    self.last_message = Some(message.clone());
                }
                let state = match e {
                    EngineError::Feedback(_)
                    | EngineError::Unsupported(_)
                    | EngineError::PermissionDenied(_)
                    | EngineError::UnsupportedFormat(_) => {
                        self.retry_not_before = Some(Instant::now() + ERROR_RETRY);
                        EngineState::Error(message)
                    }
                    _ => {
                        self.retry_not_before = None;
                        EngineState::Waiting(message)
                    }
                };
                self.set_state(state);
            }
        }
    }

    /// Periodic health check: rebuild after stream errors, device loss or default changes,
    /// and keep retrying while nothing is running.
    fn tick(&mut self) {
        if !self.wanted {
            return;
        }
        let Some(session) = &self.session else {
            if self.retry_not_before.is_none_or(|t| Instant::now() >= t) {
                self.try_start();
            }
            return;
        };
        let (flags, message) = self.shared.take_errors();
        if flags != 0 {
            let reason = format!(
                "stream error: {}",
                message.unwrap_or_else(|| "unknown".into())
            );
            self.restart(&reason);
            return;
        }
        let Some(backend) = &self.backend else { return };
        match backend.list_devices() {
            Ok(list) => {
                if let Some(reason) = session.stale_reason(&list, &self.devices) {
                    self.restart(&reason);
                }
            }
            Err(e) => tracing::debug!("device poll failed: {e}"),
        }
    }

    fn restart(&mut self, reason: &str) {
        tracing::warn!("rebuilding audio streams: {reason}");
        self.end_session();
        lock(&self.status).restarts += 1;
        self.set_state(EngineState::Waiting(format!("reconnecting: {reason}")));
        self.try_start();
    }
}

fn session_info(session: &Session, capture: bool) -> StreamInfo {
    if capture {
        session._capture.info().clone()
    } else {
        session._output.info().clone()
    }
}

/// Builds the renderer factory a backend calls once the output format is known.
fn renderer_factory(
    consumer: rtrb::Consumer<f32>,
    params: &Arc<SharedParams>,
    shared: &Arc<Shared>,
    config: &EngineConfig,
    capture_rate: u32,
) -> RendererFactory {
    let (params, shared, config) = (params.clone(), shared.clone(), *config);
    Box::new(move |format| {
        OutputRenderer::new(
            consumer,
            params,
            shared,
            RendererConfig {
                capture_rate,
                output_rate: format.sample_rate,
                prefill_ms: config.prefill_ms,
                max_fill_ms: config.max_fill_ms,
                startup_fade_ms: config.startup_fade_ms,
                resume_fade_ms: config.resume_fade_ms,
                drift_compensation: config.drift_compensation,
                chain: ChainConfig {
                    base_delay_ms: config.base_delay_ms,
                    ..ChainConfig::default()
                },
            },
        )
    })
}

/// Resolves devices, applies feedback protection, then opens capture and output.
///
/// Normally capture opens first so the output can ask for the capture's sample rate. Targets
/// that need the output to exist first (the macOS system tap) reverse the order and use the
/// rate announced in the device listing; a mismatch is reported as an error.
fn start_session(
    backend: &dyn AudioBackend,
    devices: &DeviceConfig,
    config: &EngineConfig,
    params: &Arc<SharedParams>,
    shared: &Arc<Shared>,
) -> Result<Session> {
    let list = backend.list_devices()?;
    let target = devices.capture.resolve(&list)?;
    let output_device = devices.output.resolve(&list)?;
    check_feedback(&target, &output_device)?;

    let (producer, consumer) = RingBuffer::<f32>::new(2 * RING_FRAMES);
    // Forget errors left over from a previous session.
    let _ = shared.take_errors();
    let writer = CaptureWriter::new(producer, shared.clone());
    let buffer = config.buffer;

    let (capture, output) = if backend.capture_needs_output_first(&target) {
        let capture_rate = target.device.sample_rate.unwrap_or(48_000);
        let factory = renderer_factory(consumer, params, shared, config, capture_rate);
        let output = backend.open_output(
            &output_device,
            OutputRequest {
                preferred_rate: capture_rate,
                buffer,
            },
            factory,
        )?;
        let capture = backend.open_capture(&target, writer)?;
        if capture.info().sample_rate != capture_rate {
            return Err(EngineError::UnsupportedFormat(format!(
                "\"{}\" delivers {} Hz but {} Hz was expected",
                target.device.name,
                capture.info().sample_rate,
                capture_rate
            )));
        }
        (capture, output)
    } else {
        let capture = backend.open_capture(&target, writer)?;
        let capture_rate = capture.info().sample_rate;
        let factory = renderer_factory(consumer, params, shared, config, capture_rate);
        let output = backend.open_output(
            &output_device,
            OutputRequest {
                preferred_rate: capture_rate,
                buffer,
            },
            factory,
        )?;
        (capture, output)
    };
    Ok(Session {
        _output: output,
        _capture: capture,
        target,
        output_device,
    })
}
