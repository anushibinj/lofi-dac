//! Lock-free state shared between the audio callbacks, the control thread and the UI.
//!
//! Audio callbacks only ever touch atomics here. The single `Mutex` (last error message) is used
//! from stream *error* callbacks, which are rare events, never from the data callbacks.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Bit set in [`Shared::take_errors`] when the capture stream failed.
pub const ERR_CAPTURE: u32 = 1;
/// Bit set in [`Shared::take_errors`] when the output stream failed.
pub const ERR_OUTPUT: u32 = 2;

/// Input level (linear peak) above which we say "audio detected": about -70 dBFS.
pub const AUDIO_DETECT_THRESHOLD: f32 = 3.16e-4;

/// Everything the audio threads publish.
#[derive(Debug)]
pub struct Shared {
    underruns: AtomicU64,
    overruns: AtomicU64,
    dropped_frames: AtomicU64,
    resample_errors: AtomicU64,
    capture_frames: AtomicU64,
    output_frames: AtomicU64,
    /// Output frame counter value when audio was last heard; `u64::MAX` = never.
    last_audio_frame: AtomicU64,
    ring_fill_frames: AtomicU32,
    latency_us: AtomicU32,
    capture_latency_us: AtomicU32,
    output_latency_us: AtomicU32,
    output_peak: AtomicU32,
    input_peak: AtomicU32,
    capture_rate: AtomicU32,
    output_rate: AtomicU32,
    waiting: AtomicBool,
    errors: AtomicU32,
    last_error: Mutex<Option<String>>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            underruns: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            dropped_frames: AtomicU64::new(0),
            resample_errors: AtomicU64::new(0),
            capture_frames: AtomicU64::new(0),
            output_frames: AtomicU64::new(0),
            last_audio_frame: AtomicU64::new(u64::MAX),
            ring_fill_frames: AtomicU32::new(0),
            latency_us: AtomicU32::new(0),
            capture_latency_us: AtomicU32::new(0),
            output_latency_us: AtomicU32::new(0),
            output_peak: AtomicU32::new(0),
            input_peak: AtomicU32::new(0),
            capture_rate: AtomicU32::new(0),
            output_rate: AtomicU32::new(0),
            waiting: AtomicBool::new(true),
            errors: AtomicU32::new(0),
            last_error: Mutex::new(None),
        }
    }
}

/// A plain-data copy of the metrics for display.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Metrics {
    /// Glitches: the output ran dry while capture was still delivering.
    pub underruns: u64,
    /// Times frames had to be dropped to keep latency bounded.
    pub overruns: u64,
    /// Frames dropped by overruns.
    pub dropped_frames: u64,
    pub resample_errors: u64,
    pub capture_frames: u64,
    pub output_frames: u64,
    /// Frames waiting between capture and output, in milliseconds.
    pub ring_fill_ms: f32,
    /// Estimated end to end latency in milliseconds.
    pub latency_ms: f32,
    pub capture_latency_ms: f32,
    pub output_latency_ms: f32,
    /// Output peak, linear, with a slow fall-off.
    pub output_peak: f32,
    /// Input peak, linear, with a slow fall-off.
    pub input_peak: f32,
    /// True when the input carried audible signal in the last half second.
    pub audio_detected: bool,
    /// True while the output is waiting for capture data (idle or prefilling).
    pub waiting_for_audio: bool,
    pub capture_rate: u32,
    pub output_rate: u32,
}

macro_rules! counter {
    ($add:ident, $get:ident, $field:ident) => {
        /// Adds to the counter.
        #[inline]
        pub fn $add(&self, n: u64) {
            self.$field.fetch_add(n, Ordering::Relaxed);
        }
        /// Reads the counter.
        pub fn $get(&self) -> u64 {
            self.$field.load(Ordering::Relaxed)
        }
    };
}

fn load_f32(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}

impl Shared {
    counter!(add_underrun, underruns, underruns);
    counter!(add_overrun, overruns, overruns);
    counter!(add_dropped_frames, dropped_frames, dropped_frames);
    counter!(add_resample_error, resample_errors, resample_errors);
    counter!(add_capture_frames, capture_frames, capture_frames);
    counter!(add_output_frames, output_frames, output_frames);

    /// Publishes the current ring fill in frames.
    #[inline]
    pub fn set_ring_fill(&self, frames: usize) {
        self.ring_fill_frames
            .store(frames.min(u32::MAX as usize) as u32, Ordering::Relaxed);
    }

    /// Publishes the estimated end-to-end latency.
    #[inline]
    pub fn set_latency_us(&self, us: u32) {
        self.latency_us.store(us, Ordering::Relaxed);
    }

    /// Publishes the capture device's own buffering delay.
    #[inline]
    pub fn set_capture_latency_us(&self, us: u32) {
        self.capture_latency_us.store(us, Ordering::Relaxed);
    }

    /// Capture device delay in microseconds.
    #[inline]
    pub fn capture_latency_us(&self) -> u32 {
        self.capture_latency_us.load(Ordering::Relaxed)
    }

    /// Publishes the output device's own buffering delay.
    #[inline]
    pub fn set_output_latency_us(&self, us: u32) {
        self.output_latency_us.store(us, Ordering::Relaxed);
    }

    /// Output device delay in microseconds.
    #[inline]
    pub fn output_latency_us(&self) -> u32 {
        self.output_latency_us.load(Ordering::Relaxed)
    }

    /// Publishes the output meter value.
    #[inline]
    pub fn set_output_peak(&self, v: f32) {
        self.output_peak.store(v.to_bits(), Ordering::Relaxed);
    }

    /// Publishes the input meter value.
    #[inline]
    pub fn set_input_peak(&self, v: f32) {
        self.input_peak.store(v.to_bits(), Ordering::Relaxed);
    }

    /// Records the negotiated sample rates.
    pub fn set_rates(&self, capture: u32, output: u32) {
        self.capture_rate.store(capture, Ordering::Relaxed);
        self.output_rate.store(output, Ordering::Relaxed);
    }

    /// Marks that audio was heard at output frame `frame`.
    #[inline]
    pub fn mark_audio(&self, frame: u64) {
        self.last_audio_frame.store(frame, Ordering::Relaxed);
    }

    /// Sets whether the output is waiting for capture data.
    #[inline]
    pub fn set_waiting(&self, waiting: bool) {
        self.waiting.store(waiting, Ordering::Relaxed);
    }

    /// Flags a stream failure. Called from cpal-style *error* callbacks (rare, not real-time).
    pub fn report_error(&self, flag: u32, message: &str) {
        if let Ok(mut slot) = self.last_error.try_lock() {
            *slot = Some(message.to_string());
        }
        self.errors.fetch_or(flag, Ordering::Release);
    }

    /// Returns and clears the pending error flags together with the last message.
    pub fn take_errors(&self) -> (u32, Option<String>) {
        let flags = self.errors.swap(0, Ordering::Acquire);
        if flags == 0 {
            return (0, None);
        }
        let message = self.last_error.lock().ok().and_then(|mut m| m.take());
        (flags, message)
    }

    /// Zeroes the counters (not the meters) for a fresh run.
    pub fn reset_counters(&self) {
        for c in [
            &self.underruns,
            &self.overruns,
            &self.dropped_frames,
            &self.resample_errors,
            &self.capture_frames,
            &self.output_frames,
        ] {
            c.store(0, Ordering::Relaxed);
        }
        self.last_audio_frame.store(u64::MAX, Ordering::Relaxed);
    }

    /// Copies everything into a plain struct.
    pub fn snapshot(&self) -> Metrics {
        let out_rate = self.output_rate.load(Ordering::Relaxed);
        let in_rate = self.capture_rate.load(Ordering::Relaxed);
        let frames = self.output_frames.load(Ordering::Relaxed);
        let last_audio = self.last_audio_frame.load(Ordering::Relaxed);
        let ms = |us: &AtomicU32| us.load(Ordering::Relaxed) as f32 / 1000.0;
        Metrics {
            underruns: self.underruns(),
            overruns: self.overruns(),
            dropped_frames: self.dropped_frames(),
            resample_errors: self.resample_errors(),
            capture_frames: self.capture_frames(),
            output_frames: frames,
            ring_fill_ms: if in_rate > 0 {
                self.ring_fill_frames.load(Ordering::Relaxed) as f32 * 1000.0 / in_rate as f32
            } else {
                0.0
            },
            latency_ms: ms(&self.latency_us),
            capture_latency_ms: ms(&self.capture_latency_us),
            output_latency_ms: ms(&self.output_latency_us),
            output_peak: load_f32(&self.output_peak),
            input_peak: load_f32(&self.input_peak),
            audio_detected: last_audio != u64::MAX
                && out_rate > 0
                && frames.saturating_sub(last_audio) < u64::from(out_rate / 2),
            waiting_for_audio: self.waiting.load(Ordering::Relaxed),
            capture_rate: in_rate,
            output_rate: out_rate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_accumulate() {
        let s = Shared::default();
        s.add_underrun(1);
        s.add_underrun(2);
        s.add_overrun(1);
        s.add_dropped_frames(480);
        let m = s.snapshot();
        assert_eq!((m.underruns, m.overruns, m.dropped_frames), (3, 1, 480));
    }

    #[test]
    fn counters_can_be_reset() {
        let s = Shared::default();
        s.add_underrun(4);
        s.add_output_frames(100);
        s.mark_audio(5);
        s.reset_counters();
        let m = s.snapshot();
        assert_eq!((m.underruns, m.output_frames), (0, 0));
        assert!(!m.audio_detected);
    }

    #[test]
    fn errors_are_reported_once() {
        let s = Shared::default();
        assert_eq!(s.take_errors(), (0, None));
        s.report_error(ERR_OUTPUT, "device unplugged");
        s.report_error(ERR_CAPTURE, "capture failed");
        let (flags, msg) = s.take_errors();
        assert_eq!(flags, ERR_CAPTURE | ERR_OUTPUT);
        assert_eq!(msg.as_deref(), Some("capture failed"));
        assert_eq!(s.take_errors(), (0, None));
    }

    #[test]
    fn audio_detection_uses_the_output_clock() {
        let s = Shared::default();
        s.set_rates(48_000, 48_000);
        assert!(!s.snapshot().audio_detected, "never heard anything");
        s.add_output_frames(1_000);
        s.mark_audio(1_000);
        assert!(s.snapshot().audio_detected);
        s.add_output_frames(23_000);
        assert!(s.snapshot().audio_detected, "still within half a second");
        s.add_output_frames(2_000);
        assert!(!s.snapshot().audio_detected, "more than half a second ago");
    }

    #[test]
    fn snapshot_converts_units() {
        let s = Shared::default();
        s.set_rates(48_000, 48_000);
        s.set_ring_fill(960);
        s.set_latency_us(31_500);
        s.set_output_peak(0.5);
        let m = s.snapshot();
        assert!((m.ring_fill_ms - 20.0).abs() < 1.0e-3);
        assert!((m.latency_ms - 31.5).abs() < 1.0e-3);
        assert_eq!(m.output_peak, 0.5);
        assert!(m.waiting_for_audio);
    }
}
