//! The output side of the pipeline and the heart of the engine.
//!
//! The output device callback is the **clock master**: every callback calls
//! [`OutputRenderer::render`], which pops captured frames from the ring, resamples them if
//! needed, runs the DSP chain in place and converts the result to the device format.
//!
//! State machine (all in the callback, allocation free):
//!
//! * `Waiting`: output is silence (the DSP chain still runs on silence so reverb tails ring out).
//!   Once the ring holds `prefill` frames *in reserve on top of what this callback consumes* we
//!   switch to `Running` with a short input fade-in.
//! * `Running`: frames are pulled from the ring. If the ring cannot supply a full callback we
//!   fade out what we have, output silence and go back to `Waiting` (re-prefill).
//!
//! `prefill` is the reserve kept in the ring **after** each callback has been served. Capture
//! delivers audio in bursts (one device period at a time), so what protects against a late burst
//! is the level *after* the pull, not the level before it: with only a period's worth of audio
//! at the start of a callback, a burst that lands just after the callback leaves zero margin.
//!
//! A gap in capture data longer than half a second is *idle silence* (WASAPI loopback stops
//! delivering packets when nothing is playing), so it is not counted as an underrun.
//! If the ring ever holds more than `max_fill` frames the oldest frames are discarded, which
//! bounds latency after a stall.

use crate::error::Result;
use crate::metrics::{AUDIO_DETECT_THRESHOLD, ERR_OUTPUT, Shared};
use crate::pcm::Pcm;
use crate::resample::ResampleStage;
use lofify_dsp::chain::{Chain, ChainConfig};
use lofify_dsp::shared::SharedParams;
use rtrb::Consumer;
use std::sync::Arc;
use std::time::Duration;

/// Frames processed per pass; larger device buffers are rendered in pieces.
const SCRATCH_FRAMES: usize = 2048;
/// A capture gap longer than this is idle silence, not a glitch.
const IDLE_SECONDS: f32 = 0.5;
/// Drift controller (PI) proportional gain: a ring 100 % away from its target nudges the ratio by
/// this fraction.
const DRIFT_KP: f64 = 0.004;
/// Drift controller integral gain per second. The integral term learns the clock offset between
/// capture and output devices, so the ring settles *at* its target instead of near it.
/// Together with `DRIFT_KP` this is a critically damped loop of about 0.1 rad/s (settles in
/// roughly 40 s), well below the 1 s fill-level average so the loop stays stable.
const DRIFT_KI: f64 = 2.0e-4;
/// Time constant of the fill-level average used by the drift controller.
const FILL_AVERAGE_SECONDS: f32 = 1.0;
/// Meter fall-off time constant.
const METER_FALL_SECONDS: f32 = 0.3;

/// Everything needed to build a renderer.
#[derive(Debug, Clone, Copy)]
pub struct RendererConfig {
    /// Sample rate of the frames in the ring.
    pub capture_rate: u32,
    /// Sample rate of the output device.
    pub output_rate: u32,
    /// Audio (ms) kept in reserve in the ring after each callback. This is the margin against a
    /// late capture burst, and (with the device buffers and DSP delay) sets the latency.
    pub prefill_ms: f32,
    /// Ring level (ms) above which the oldest frames are dropped.
    pub max_fill_ms: f32,
    /// Fade-in applied to the output when the stream starts.
    pub startup_fade_ms: f32,
    /// Fade applied to the input whenever playback resumes after a gap.
    pub resume_fade_ms: f32,
    /// Nudge the resampling ratio to hold the ring at its target fill.
    pub drift_compensation: bool,
    /// DSP configuration.
    pub chain: ChainConfig,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            capture_rate: 48_000,
            output_rate: 48_000,
            prefill_ms: 20.0,
            max_fill_ms: 120.0,
            startup_fade_ms: 50.0,
            resume_fade_ms: 5.0,
            drift_compensation: true,
            chain: ChainConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Waiting,
    Running,
}

/// Pops, resamples, processes and writes audio inside the output callback.
#[derive(Debug)]
pub struct OutputRenderer {
    consumer: Consumer<f32>,
    stage: Option<ResampleStage>,
    chain: Chain,
    params: Arc<SharedParams>,
    shared: Arc<Shared>,
    capture_rate: f32,
    output_rate: f32,

    state: State,
    /// True while waiting because of an underrun (as opposed to the initial prefill).
    wait_is_glitch: bool,
    wait_since: u64,
    idle_frames: u64,
    frames_rendered: u64,

    /// Frames kept in the ring after each callback: the prefill plus the resampler's lookahead.
    reserve_frames: usize,
    max_fill_frames: usize,
    fill_average: f32,
    /// Frames served by the most recent callback.
    last_frames: usize,
    /// Learned relative clock offset (the drift controller's integral term).
    drift_integral: f64,

    resume_fade_len: usize,
    resume_fade_pos: usize,
    startup_fade_len: usize,
    startup_fade_pos: usize,

    input_meter: f32,
    output_meter: f32,
    scratch: Vec<f32>,
}

fn ms_to_frames(ms: f32, rate: u32) -> usize {
    (ms.max(0.0) * 0.001 * rate as f32).round() as usize
}

/// Multiplies `dst` by a linear ramp that continues across calls until `len` frames have passed.
fn fade_in(dst: &mut [f32], pos: &mut usize, len: usize) {
    for frame in dst.chunks_exact_mut(2) {
        if *pos >= len {
            return;
        }
        let g = (*pos as f32 + 1.0) / len as f32;
        frame[0] *= g;
        frame[1] *= g;
        *pos += 1;
    }
}

impl OutputRenderer {
    /// Builds a renderer. Allocates everything up front; call on the control thread.
    pub fn new(
        consumer: Consumer<f32>,
        params: Arc<SharedParams>,
        shared: Arc<Shared>,
        cfg: RendererConfig,
    ) -> Result<Self> {
        let stage = if cfg.drift_compensation || cfg.capture_rate != cfg.output_rate {
            Some(ResampleStage::new(cfg.capture_rate, cfg.output_rate)?)
        } else {
            None
        };
        let prefill_frames = ms_to_frames(cfg.prefill_ms, cfg.capture_rate).max(1);
        // A callback can pull up to one resampler chunk more input than it outputs, so the
        // reserve includes that lookahead; otherwise it would eat into the jitter margin.
        let lookahead = if stage.is_some() {
            crate::resample::CHUNK_FRAMES
        } else {
            0
        };
        let reserve_frames = prefill_frames + lookahead;
        let max_fill_frames =
            ms_to_frames(cfg.max_fill_ms, cfg.capture_rate).max(reserve_frames + 1);
        shared.set_rates(cfg.capture_rate, cfg.output_rate);
        shared.set_waiting(true);
        Ok(Self {
            consumer,
            stage,
            chain: Chain::with_config(cfg.output_rate as f32, cfg.chain),
            params,
            shared,
            capture_rate: cfg.capture_rate as f32,
            output_rate: cfg.output_rate as f32,
            state: State::Waiting,
            wait_is_glitch: false,
            wait_since: 0,
            idle_frames: (IDLE_SECONDS * cfg.output_rate as f32) as u64,
            frames_rendered: 0,
            reserve_frames,
            max_fill_frames,
            fill_average: reserve_frames as f32,
            last_frames: 0,
            drift_integral: 0.0,
            resume_fade_len: ms_to_frames(cfg.resume_fade_ms, cfg.output_rate),
            resume_fade_pos: 0,
            startup_fade_len: ms_to_frames(cfg.startup_fade_ms, cfg.output_rate),
            startup_fade_pos: 0,
            input_meter: 0.0,
            output_meter: 0.0,
            scratch: vec![0.0; SCRATCH_FRAMES * 2],
        })
    }

    /// Shared metrics and error flags, for the backend's error callback.
    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// Reports that the output stream failed (called from a backend's error callback).
    pub fn report_error(&self, message: &str) {
        self.shared.report_error(ERR_OUTPUT, message);
    }

    /// Publishes the output device's own buffering delay (callback time to playback time).
    pub fn set_device_latency(&self, latency: Duration) {
        self.shared
            .set_output_latency_us(latency.as_micros().min(u128::from(u32::MAX)) as u32);
    }

    /// Constant latency of the DSP chain in frames at the output rate.
    pub fn dsp_latency_frames(&self) -> usize {
        self.chain.latency_samples()
    }

    /// Fills one device buffer of `channels` interleaved channels. Every sample of `out` is
    /// written; on any problem the result is silence, never stale data. Real-time safe.
    pub fn render<T: Pcm>(&mut self, out: &mut [T], channels: usize) {
        if channels == 0 {
            return;
        }
        self.chain.sync_from(&self.params);
        let total = out.len() / channels;
        let mut buf = std::mem::take(&mut self.scratch);
        let mut done = 0;
        while done < total {
            let n = (total - done).min(SCRATCH_FRAMES);
            self.render_chunk(&mut buf[..n * 2]);
            let dst = &mut out[done * channels..(done + n) * channels];
            for (i, frame) in dst.chunks_exact_mut(channels).enumerate() {
                let (l, r) = (buf[2 * i], buf[2 * i + 1]);
                if channels == 1 {
                    frame[0] = T::from_f32((l + r) * 0.5);
                } else {
                    frame[0] = T::from_f32(l);
                    frame[1] = T::from_f32(r);
                    for s in &mut frame[2..] {
                        *s = T::from_f32(0.0);
                    }
                }
            }
            done += n;
        }
        self.scratch = buf;
        for s in &mut out[total * channels..] {
            *s = T::from_f32(0.0);
        }
    }

    fn render_chunk(&mut self, dst: &mut [f32]) {
        let frames = dst.len() / 2;
        self.acquire(dst);
        self.chain.process(dst);
        fade_in(dst, &mut self.startup_fade_pos, self.startup_fade_len);

        let peak = dst.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        let fall = (-(frames as f32) / (self.output_rate * METER_FALL_SECONDS)).exp();
        self.output_meter = peak.max(self.output_meter * fall);
        self.shared.set_output_peak(self.output_meter);

        self.frames_rendered += frames as u64;
        self.shared.add_output_frames(frames as u64);
    }

    /// Fills `dst` with input audio, or silence while waiting or after an underrun.
    fn acquire(&mut self, dst: &mut [f32]) {
        let frames = dst.len() / 2;
        let mut ring = self.consumer.slots() / 2;
        // The level we want right before pulling: the reserve plus what this callback takes.
        let target = self.reserve_frames + frames;
        self.last_frames = frames;

        if ring > self.max_fill_frames.max(2 * target) {
            // Overrun: keep latency bounded by discarding the oldest frames.
            let excess = ring - target;
            self.discard(excess);
            self.shared.add_overrun(1);
            self.shared.add_dropped_frames(excess as u64);
            self.fill_average = self.reserve_frames as f32;
            ring = self.consumer.slots() / 2;
        }
        self.shared.set_ring_fill(ring);

        if self.state == State::Waiting {
            if ring < target {
                dst.fill(0.0);
                self.shared.set_waiting(true);
                self.publish_latency();
                return;
            }
            self.resume();
        }

        let got = self.pull(dst);
        if got < frames {
            // Underrun: fade out whatever we have, silence the rest, re-prefill.
            for (i, frame) in dst.chunks_exact_mut(2).enumerate().take(got) {
                let g = 1.0 - (i as f32 + 1.0) / got as f32;
                frame[0] *= g;
                frame[1] *= g;
            }
            dst[got * 2..].fill(0.0);
            self.state = State::Waiting;
            self.wait_is_glitch = true;
            self.wait_since = self.frames_rendered;
            self.shared.set_waiting(true);
        } else {
            fade_in(dst, &mut self.resume_fade_pos, self.resume_fade_len);
            let peak = dst.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let fall = (-(frames as f32) / (self.output_rate * METER_FALL_SECONDS)).exp();
            self.input_meter = peak.max(self.input_meter * fall);
            self.shared.set_input_peak(self.input_meter);
            if peak > AUDIO_DETECT_THRESHOLD {
                self.shared.mark_audio(self.frames_rendered);
            }
            self.track_drift(frames, ring);
        }
        self.publish_latency();
    }

    /// Leaves `Waiting`: counts a glitch if capture came back quickly and starts the fade-in.
    fn resume(&mut self) {
        if self.wait_is_glitch && self.frames_rendered - self.wait_since < self.idle_frames {
            self.shared.add_underrun(1);
        }
        self.wait_is_glitch = false;
        self.state = State::Running;
        self.resume_fade_pos = 0;
        self.shared.set_waiting(false);
    }

    /// Pulls up to `dst.len() / 2` input frames; returns how many were produced.
    fn pull(&mut self, dst: &mut [f32]) -> usize {
        match &mut self.stage {
            Some(stage) => stage.pull(&mut self.consumer, dst),
            None => {
                let even = dst.len() & !1;
                let (got, _) = self.consumer.pop_partial_slice(&mut dst[..even]);
                got.len() / 2
            }
        }
    }

    fn discard(&mut self, frames: usize) {
        let mut left = frames * 2;
        let mut sink = [0.0f32; 256];
        while left > 0 {
            let n = left.min(sink.len());
            let (got, _) = self.consumer.pop_partial_slice(&mut sink[..n]);
            if got.is_empty() {
                break;
            }
            left -= got.len();
        }
    }

    /// Nudges the resampling ratio so the ring reserve settles at its prefill level (PI
    /// controller). It tracks the level *after* the pull, which is what protects against a late
    /// capture burst.
    fn track_drift(&mut self, frames: usize, ring_before: usize) {
        let dt = frames as f32 / self.capture_rate;
        let alpha = (dt / FILL_AVERAGE_SECONDS).min(1.0);
        let after = ring_before.saturating_sub(frames);
        self.fill_average += alpha * (after as f32 - self.fill_average);
        if let Some(stage) = &mut self.stage {
            let target = self.reserve_frames as f64;
            let error = (f64::from(self.fill_average) - target) / target;
            let limit = crate::resample::MAX_NUDGE;
            self.drift_integral =
                (self.drift_integral + error * DRIFT_KI * f64::from(dt)).clamp(-limit, limit);
            // Ring too full (error > 0) => consume input faster => fewer output frames per input.
            stage.set_relative_ratio(1.0 - (DRIFT_KP * error + self.drift_integral));
        }
    }

    fn publish_latency(&self) {
        let stage_frames = self.stage.as_ref().map_or(0, ResampleStage::delay_frames) as f32;
        // A frame waits, on average, for the reserve plus half a callback's worth of audio.
        let waiting = self.fill_average + self.last_frames as f32 * 0.5;
        let seconds = waiting / self.capture_rate
            + (stage_frames + self.chain.latency_samples() as f32) / self.output_rate;
        let us = seconds * 1.0e6
            + self.shared.capture_latency_us() as f32
            + self.shared.output_latency_us() as f32;
        self.shared.set_latency_us(us.max(0.0) as u32);
    }
}
