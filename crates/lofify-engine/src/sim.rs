//! A simulated audio backend.
//!
//! Real capture and render threads run against a (scalable) wall clock, feed the real
//! [`CaptureWriter`] and drive the real [`OutputRenderer`]. It exists so the whole engine,
//! including the control thread, reconnect logic and feedback protection, can be tested end to
//! end on any machine, and so `lofify passthrough --simulate` works without audio hardware.
//!
//! The simulated world can add and remove devices, change the default output, stop capture
//! packets (like WASAPI loopback in silence), skew the capture clock, and inject stream errors.

use crate::backend::{AudioBackend, CaptureSource, OutputRequest, OutputSink, RendererFactory};
use crate::capture::CaptureWriter;
use crate::error::{EngineError, Result};
use crate::output::OutputRenderer;
use crate::types::{CaptureTarget, DeviceInfo, DeviceList, Direction, OutputFormat, StreamInfo};
use std::collections::HashMap;
use std::f64::consts::TAU;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// What a simulated capture device plays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SimSignal {
    Silence,
    Sine { hz: f32, amp: f32 },
}

#[derive(Debug, Clone)]
struct SimDevice {
    info: DeviceInfo,
    rate: u32,
    channels: u16,
    /// Output devices that convert any requested rate themselves (WASAPI's AUTOCONVERTPCM).
    accepts_any_rate: bool,
    alive: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
struct World {
    devices: Vec<SimDevice>,
    default_input: Option<String>,
    default_output: Option<String>,
    next_id: u32,
}

#[derive(Debug)]
struct Control {
    time_scale: AtomicU64,
    capture_ppm: AtomicU64,
    signal: Mutex<SimSignal>,
    capture_active: AtomicBool,
    inject_capture_error: AtomicBool,
    inject_output_error: AtomicBool,
    captures_opened: AtomicUsize,
    outputs_opened: AtomicUsize,
    recordings: Mutex<HashMap<String, Vec<f32>>>,
}

/// Cloneable handle to the simulated world, for tests and the CLI to poke at.
#[derive(Debug, Clone)]
pub struct SimWorld {
    world: Arc<Mutex<World>>,
    ctl: Arc<Control>,
}

/// The backend the engine talks to. Create with [`SimWorld::backend`].
#[derive(Debug)]
pub struct SimBackend {
    world: SimWorld,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Default for SimWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl SimWorld {
    /// An empty world at real-time speed playing a 440 Hz tone on every capture device.
    pub fn new() -> Self {
        Self {
            world: Arc::new(Mutex::new(World::default())),
            ctl: Arc::new(Control {
                time_scale: AtomicU64::new(1.0f64.to_bits()),
                capture_ppm: AtomicU64::new(0.0f64.to_bits()),
                signal: Mutex::new(SimSignal::Sine {
                    hz: 440.0,
                    amp: 0.5,
                }),
                capture_active: AtomicBool::new(true),
                inject_capture_error: AtomicBool::new(false),
                inject_output_error: AtomicBool::new(false),
                captures_opened: AtomicUsize::new(0),
                outputs_opened: AtomicUsize::new(0),
                recordings: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// A typical Windows machine with VB-Cable installed and Windows playing through it:
    /// default output is "CABLE Input", plus speakers and headphones, a microphone and
    /// "CABLE Output".
    pub fn windows_like() -> Self {
        let w = Self::new();
        w.add_input("Microphone (Realtek Audio)", 48_000, 2);
        w.add_input("CABLE Output (VB-Audio Virtual Cable)", 48_000, 2);
        let cable = w.add_output("CABLE Input (VB-Audio Virtual Cable)", 48_000, 2, true);
        w.add_output("Speakers (Realtek Audio)", 48_000, 2, true);
        w.add_output("Headphones (USB DAC)", 48_000, 2, true);
        w.set_default_output(&cable);
        w
    }

    /// The backend view of this world.
    pub fn backend(&self) -> SimBackend {
        SimBackend {
            world: self.clone(),
        }
    }

    fn add(
        &self,
        name: &str,
        direction: Direction,
        rate: u32,
        channels: u16,
        accepts_any_rate: bool,
    ) -> String {
        let mut w = lock(&self.world);
        w.next_id += 1;
        let id = format!("sim:{}:{name}", w.next_id);
        let mut info = DeviceInfo::new(id.clone(), name, direction);
        info.sample_rate = Some(rate);
        info.channels = Some(channels);
        w.devices.push(SimDevice {
            info,
            rate,
            channels,
            accepts_any_rate,
            alive: Arc::new(AtomicBool::new(true)),
        });
        let default = match direction {
            Direction::Input => &mut w.default_input,
            Direction::Output => &mut w.default_output,
        };
        if default.is_none() {
            *default = Some(id.clone());
        }
        id
    }

    /// Adds a recording device and returns its id.
    pub fn add_input(&self, name: &str, rate: u32, channels: u16) -> String {
        self.add(name, Direction::Input, rate, channels, false)
    }

    /// Adds a playback device and returns its id. `accepts_any_rate` mimics WASAPI's automatic
    /// rate conversion; without it the device only runs at `rate`.
    pub fn add_output(
        &self,
        name: &str,
        rate: u32,
        channels: u16,
        accepts_any_rate: bool,
    ) -> String {
        self.add(name, Direction::Output, rate, channels, accepts_any_rate)
    }

    /// Unplugs a device: it disappears from listings and its streams report an error.
    pub fn remove_device(&self, id: &str) {
        let mut w = lock(&self.world);
        for d in w.devices.iter().filter(|d| d.info.id == id) {
            d.alive.store(false, Ordering::Release);
        }
        w.devices.retain(|d| d.info.id != id);
        if w.default_output.as_deref() == Some(id) {
            w.default_output = w
                .devices
                .iter()
                .find(|d| d.info.direction == Direction::Output)
                .map(|d| d.info.id.clone());
        }
        if w.default_input.as_deref() == Some(id) {
            w.default_input = w
                .devices
                .iter()
                .find(|d| d.info.direction == Direction::Input)
                .map(|d| d.info.id.clone());
        }
    }

    /// Makes `id` the system default playback device.
    pub fn set_default_output(&self, id: &str) {
        lock(&self.world).default_output = Some(id.to_string());
    }

    /// Looks up a device id by (substring of) name.
    pub fn device_id(&self, name: &str) -> Option<String> {
        lock(&self.world)
            .devices
            .iter()
            .find(|d| d.info.name.contains(name))
            .map(|d| d.info.id.clone())
    }

    /// Runs simulated time `scale` times faster than the wall clock (default 1.0).
    pub fn set_time_scale(&self, scale: f64) {
        self.ctl
            .time_scale
            .store(scale.max(0.01).to_bits(), Ordering::Relaxed);
    }

    /// Makes the capture clock run `ppm` parts per million fast (positive) or slow.
    pub fn set_capture_ppm(&self, ppm: f64) {
        self.ctl.capture_ppm.store(ppm.to_bits(), Ordering::Relaxed);
    }

    /// Chooses what capture devices play.
    pub fn set_signal(&self, signal: SimSignal) {
        *lock(&self.ctl.signal) = signal;
    }

    /// When false, capture devices stop delivering packets entirely, like WASAPI loopback while
    /// nothing is playing.
    pub fn set_capture_active(&self, active: bool) {
        self.ctl.capture_active.store(active, Ordering::Relaxed);
    }

    /// Makes the next capture stream report a fatal error.
    pub fn inject_capture_error(&self) {
        self.ctl.inject_capture_error.store(true, Ordering::Release);
    }

    /// Makes the next output stream report a fatal error.
    pub fn inject_output_error(&self) {
        self.ctl.inject_output_error.store(true, Ordering::Release);
    }

    /// How many capture streams were ever opened.
    pub fn captures_opened(&self) -> usize {
        self.ctl.captures_opened.load(Ordering::Relaxed)
    }

    /// How many output streams were ever opened.
    pub fn outputs_opened(&self) -> usize {
        self.ctl.outputs_opened.load(Ordering::Relaxed)
    }

    /// Everything rendered to `device_id` so far (stereo interleaved, first two channels).
    pub fn recorded(&self, device_id: &str) -> Vec<f32> {
        lock(&self.ctl.recordings)
            .get(device_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Forgets all recordings.
    pub fn clear_recordings(&self) {
        lock(&self.ctl.recordings).clear();
    }

    fn find(&self, id: &str) -> Option<SimDevice> {
        lock(&self.world)
            .devices
            .iter()
            .find(|d| d.info.id == id)
            .cloned()
    }
}

struct Clock {
    start: Instant,
    ctl: Arc<Control>,
    apply_ppm: bool,
}

impl Clock {
    fn new(ctl: &Arc<Control>, apply_ppm: bool) -> Self {
        Self {
            start: Instant::now(),
            ctl: ctl.clone(),
            apply_ppm,
        }
    }

    /// Simulated seconds elapsed.
    fn now(&self) -> f64 {
        let scale = f64::from_bits(self.ctl.time_scale.load(Ordering::Relaxed));
        let ppm = if self.apply_ppm {
            f64::from_bits(self.ctl.capture_ppm.load(Ordering::Relaxed))
        } else {
            0.0
        };
        self.start.elapsed().as_secs_f64() * scale * (1.0 + ppm * 1.0e-6)
    }
}

#[derive(Debug)]
struct StreamThread {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    info: StreamInfo,
}

impl Drop for StreamThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// A running simulated capture stream.
#[derive(Debug)]
pub struct SimCapture(StreamThread);
/// A running simulated output stream.
#[derive(Debug)]
pub struct SimOutput(StreamThread);

impl CaptureSource for SimCapture {
    fn info(&self) -> &StreamInfo {
        &self.0.info
    }
}
impl OutputSink for SimOutput {
    fn info(&self) -> &StreamInfo {
        &self.0.info
    }
}

/// Simulated threads wake up every 2 ms of *simulated* time, so the polling interval shrinks as
/// the simulation is sped up; otherwise fast-forwarding would add scheduling jitter that no real
/// device has.
fn poll_interval(ctl: &Control) -> Duration {
    let scale = f64::from_bits(ctl.time_scale.load(Ordering::Relaxed));
    Duration::from_secs_f64((0.002 / scale).clamp(0.000_05, 0.002))
}

fn capture_loop(stop: &AtomicBool, dev: &SimDevice, ctl: &Arc<Control>, mut writer: CaptureWriter) {
    let clock = Clock::new(ctl, true);
    let period = u64::from(dev.rate / 100).max(1);
    let channels = usize::from(dev.channels);
    let mut produced = 0u64;
    let mut buf = vec![0.0f32; period as usize * channels];
    writer.set_device_latency(Duration::from_millis(10));
    while !stop.load(Ordering::Acquire) {
        thread::sleep(poll_interval(ctl));
        if !dev.alive.load(Ordering::Acquire) {
            writer.report_error("capture device was removed");
            park_until(stop);
            return;
        }
        if ctl.inject_capture_error.swap(false, Ordering::AcqRel) {
            writer.report_error("injected capture error");
            park_until(stop);
            return;
        }
        let due = (clock.now() * f64::from(dev.rate)) as u64;
        while produced + period <= due {
            if ctl.capture_active.load(Ordering::Relaxed) {
                let signal = *lock(&ctl.signal);
                for (i, frame) in buf.chunks_exact_mut(channels).enumerate() {
                    let v = match signal {
                        SimSignal::Silence => 0.0,
                        SimSignal::Sine { hz, amp } => {
                            let t = (produced + i as u64) as f64 / f64::from(dev.rate);
                            (TAU * f64::from(hz) * t).sin() as f32 * amp
                        }
                    };
                    frame.fill(v);
                }
                writer.write(&buf, channels);
            }
            produced += period;
        }
    }
}

fn output_loop(
    stop: &AtomicBool,
    dev: &SimDevice,
    rate: u32,
    ctl: &Arc<Control>,
    world: &SimWorld,
    mut renderer: OutputRenderer,
) {
    let clock = Clock::new(ctl, false);
    let period = u64::from(rate / 100).max(1);
    let channels = usize::from(dev.channels);
    let mut rendered = 0u64;
    let mut buf = vec![0.0f32; period as usize * channels];
    renderer.set_device_latency(Duration::from_millis(10));
    let key = dev.info.id.clone();
    while !stop.load(Ordering::Acquire) {
        thread::sleep(poll_interval(ctl));
        if !dev.alive.load(Ordering::Acquire) {
            renderer.report_error("output device was removed");
            park_until(stop);
            return;
        }
        if ctl.inject_output_error.swap(false, Ordering::AcqRel) {
            renderer.report_error("injected output error");
            park_until(stop);
            return;
        }
        let due = (clock.now() * f64::from(rate)) as u64;
        while rendered + period <= due {
            renderer.render(&mut buf, channels);
            let mut rec = lock(&world.ctl.recordings);
            let out = rec.entry(key.clone()).or_default();
            for frame in buf.chunks_exact(channels) {
                out.push(frame[0]);
                out.push(if channels > 1 { frame[1] } else { frame[0] });
            }
            rendered += period;
        }
    }
}

fn park_until(stop: &AtomicBool) {
    while !stop.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(5));
    }
}

impl AudioBackend for SimBackend {
    fn name(&self) -> &'static str {
        "simulated"
    }

    fn list_devices(&self) -> Result<DeviceList> {
        let w = lock(&self.world.world);
        let mut list = DeviceList::default();
        for d in &w.devices {
            let mut info = d.info.clone();
            let default = match info.direction {
                Direction::Input => &w.default_input,
                Direction::Output => &w.default_output,
            };
            info.is_default = default.as_deref() == Some(info.id.as_str());
            match info.direction {
                Direction::Input => list.inputs.push(info),
                Direction::Output => list.outputs.push(info),
            }
        }
        Ok(list)
    }

    fn open_capture(
        &self,
        target: &CaptureTarget,
        writer: CaptureWriter,
    ) -> Result<Box<dyn CaptureSource>> {
        let dev = self
            .world
            .find(&target.device.id)
            .ok_or_else(|| EngineError::DeviceNotFound(target.device.name.clone()))?;
        let expected = if target.loopback {
            Direction::Output
        } else {
            Direction::Input
        };
        if dev.info.direction != expected {
            return Err(EngineError::Backend(format!(
                "{} cannot be captured that way",
                dev.info.name
            )));
        }
        self.world
            .ctl
            .captures_opened
            .fetch_add(1, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        let info = StreamInfo {
            device_name: dev.info.name.clone(),
            device_id: dev.info.id.clone(),
            sample_rate: dev.rate,
            channels: dev.channels,
            buffer_frames: Some(dev.rate / 100),
        };
        let (s, ctl) = (stop.clone(), self.world.ctl.clone());
        let handle = thread::spawn(move || capture_loop(&s, &dev, &ctl, writer));
        Ok(Box::new(SimCapture(StreamThread {
            stop,
            handle: Some(handle),
            info,
        })))
    }

    fn open_output(
        &self,
        device: &DeviceInfo,
        request: OutputRequest,
        factory: RendererFactory,
    ) -> Result<Box<dyn OutputSink>> {
        let dev = self
            .world
            .find(&device.id)
            .ok_or_else(|| EngineError::DeviceNotFound(device.name.clone()))?;
        if dev.info.direction != Direction::Output {
            return Err(EngineError::Backend(format!(
                "{} is not a playback device",
                dev.info.name
            )));
        }
        let rate = if dev.accepts_any_rate {
            request.preferred_rate
        } else {
            dev.rate
        };
        let renderer = factory(OutputFormat {
            sample_rate: rate,
            channels: dev.channels,
        })?;
        self.world
            .ctl
            .outputs_opened
            .fetch_add(1, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        let info = StreamInfo {
            device_name: dev.info.name.clone(),
            device_id: dev.info.id.clone(),
            sample_rate: rate,
            channels: dev.channels,
            buffer_frames: Some(rate / 100),
        };
        let (s, ctl, world) = (stop.clone(), self.world.ctl.clone(), self.world.clone());
        let handle = thread::spawn(move || output_loop(&s, &dev, rate, &ctl, &world, renderer));
        Ok(Box::new(SimOutput(StreamThread {
            stop,
            handle: Some(handle),
            info,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_like_world_lists_the_expected_devices() {
        let w = SimWorld::windows_like();
        let list = w.backend().list_devices().unwrap();
        assert_eq!(list.inputs.len(), 2);
        assert_eq!(list.outputs.len(), 3);
        assert!(
            list.default_output()
                .unwrap()
                .name
                .starts_with("CABLE Input")
        );
        assert!(list.find_virtual_cable_input().is_some());
    }

    #[test]
    fn removing_a_device_updates_listing_and_default() {
        let w = SimWorld::windows_like();
        let cable = w.device_id("CABLE Input").unwrap();
        w.remove_device(&cable);
        let list = w.backend().list_devices().unwrap();
        assert_eq!(list.outputs.len(), 2);
        assert!(list.default_output().is_some(), "a new default is picked");
        assert!(!list.contains(&cable, Direction::Output));
    }

    #[test]
    fn unknown_device_cannot_be_opened() {
        let w = SimWorld::windows_like();
        let (p, _c) = rtrb::RingBuffer::<f32>::new(64);
        let writer = CaptureWriter::new(p, Arc::new(crate::metrics::Shared::default()));
        let bogus = CaptureTarget {
            device: DeviceInfo::new("nope", "Nope", Direction::Input),
            loopback: false,
        };
        assert!(matches!(
            w.backend().open_capture(&bogus, writer),
            Err(EngineError::DeviceNotFound(_))
        ));
    }
}
