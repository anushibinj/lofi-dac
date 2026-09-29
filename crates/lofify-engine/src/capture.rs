//! The capture side of the pipeline: device samples in, interleaved stereo `f32` frames out to
//! the ring buffer. Runs inside the capture callback, so it never allocates, locks or logs.

use crate::mapping::frame_to_stereo;
use crate::metrics::{ERR_CAPTURE, Shared};
use crate::pcm::Pcm;
use rtrb::Producer;
use std::sync::Arc;
use std::time::Duration;

/// Frames converted per pass; bigger device buffers are processed in pieces.
const SCRATCH_FRAMES: usize = 2048;
/// Channels beyond this are ignored when downmixing.
const MAX_CHANNELS: usize = 16;

/// Converts device buffers of any format and channel count and pushes them into the ring.
#[derive(Debug)]
pub struct CaptureWriter {
    producer: Producer<f32>,
    shared: Arc<Shared>,
    scratch: Vec<f32>,
}

impl CaptureWriter {
    /// Wraps the producer half of the capture ring.
    pub fn new(producer: Producer<f32>, shared: Arc<Shared>) -> Self {
        Self {
            producer,
            shared,
            scratch: vec![0.0; SCRATCH_FRAMES * 2],
        }
    }

    /// Handle for reporting stream errors and publishing device latency.
    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// Reports that the capture stream failed (called from a backend's error callback).
    pub fn report_error(&self, message: &str) {
        self.shared.report_error(ERR_CAPTURE, message);
    }

    /// Publishes the device's own buffering delay (capture timestamp to callback time).
    pub fn set_device_latency(&self, latency: Duration) {
        self.shared
            .set_capture_latency_us(latency.as_micros().min(u128::from(u32::MAX)) as u32);
    }

    /// Consumes one device buffer of interleaved samples with `channels` channels.
    ///
    /// If the ring is full the newest frames are dropped and counted as an overrun; the output
    /// side trims the ring back to its target fill, which drops the *oldest* frames.
    pub fn write<T: Pcm>(&mut self, data: &[T], channels: usize) {
        if channels == 0 {
            return;
        }
        let used = channels.min(MAX_CHANNELS);
        let total_frames = data.len() / channels;
        let mut done = 0;
        while done < total_frames {
            let frames = (total_frames - done).min(SCRATCH_FRAMES);
            let src = &data[done * channels..(done + frames) * channels];
            for (i, frame) in src.chunks_exact(channels).enumerate() {
                let (l, r) = if channels == 2 {
                    (frame[0].to_f32(), frame[1].to_f32())
                } else {
                    let mut tmp = [0.0f32; MAX_CHANNELS];
                    for (t, s) in tmp.iter_mut().zip(&frame[..used]) {
                        *t = s.to_f32();
                    }
                    frame_to_stereo(&tmp[..used])
                };
                self.scratch[2 * i] = l;
                self.scratch[2 * i + 1] = r;
            }
            self.push(frames);
            done += frames;
        }
    }

    fn push(&mut self, frames: usize) {
        let want = frames * 2;
        // Only whole frames may enter the ring so left and right can never swap.
        let room = self.producer.slots() & !1;
        let n = want.min(room);
        let (pushed, _) = self.producer.push_partial_slice(&self.scratch[..n]);
        self.shared.add_capture_frames((pushed.len() / 2) as u64);
        if n < want {
            self.shared.add_overrun(1);
            self.shared.add_dropped_frames(((want - n) / 2) as u64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtrb::RingBuffer;

    fn writer(capacity: usize) -> (CaptureWriter, rtrb::Consumer<f32>, Arc<Shared>) {
        let (p, c) = RingBuffer::<f32>::new(capacity);
        let shared = Arc::new(Shared::default());
        (CaptureWriter::new(p, shared.clone()), c, shared)
    }

    fn drain(c: &mut rtrb::Consumer<f32>) -> Vec<f32> {
        let mut v = vec![0.0; c.slots()];
        let (got, _) = c.pop_partial_slice(&mut v);
        let n = got.len();
        v.truncate(n);
        v
    }

    #[test]
    fn stereo_f32_passes_through_unchanged() {
        let (mut w, mut c, shared) = writer(64);
        w.write(&[0.1f32, -0.2, 0.3, -0.4], 2);
        assert_eq!(drain(&mut c), [0.1, -0.2, 0.3, -0.4]);
        assert_eq!(shared.capture_frames(), 2);
        assert_eq!(shared.overruns(), 0);
    }

    #[test]
    fn i16_is_converted() {
        let (mut w, mut c, _) = writer(64);
        w.write(&[16_384i16, -16_384], 2);
        let out = drain(&mut c);
        assert!((out[0] - 0.5).abs() < 1.0e-3 && (out[1] + 0.5).abs() < 1.0e-3);
    }

    #[test]
    fn mono_is_duplicated() {
        let (mut w, mut c, _) = writer(64);
        w.write(&[0.25f32, 0.5], 1);
        assert_eq!(drain(&mut c), [0.25, 0.25, 0.5, 0.5]);
    }

    #[test]
    fn surround_is_downmixed() {
        let (mut w, mut c, _) = writer(64);
        // one 5.1 frame: FL FR FC LFE BL BR
        w.write(&[0.5f32, 0.25, 0.0, 1.0, 0.0, 0.0], 6);
        let out = drain(&mut c);
        assert_eq!(out, [0.5, 0.25], "LFE must be dropped, FL/FR kept");
    }

    #[test]
    fn partial_trailing_frame_is_ignored() {
        let (mut w, mut c, shared) = writer(64);
        w.write(&[0.1f32, 0.2, 0.3], 2);
        assert_eq!(drain(&mut c), [0.1, 0.2]);
        assert_eq!(shared.capture_frames(), 1);
    }

    #[test]
    fn buffers_larger_than_the_scratch_are_handled() {
        let (mut w, mut c, shared) = writer(2 * 6_000 + 2);
        let data: Vec<f32> = (0..2 * 5_000).map(|i| i as f32 / 20_000.0).collect();
        w.write(&data, 2);
        assert_eq!(drain(&mut c), data);
        assert_eq!(shared.capture_frames(), 5_000);
    }

    #[test]
    fn full_ring_drops_newest_and_counts_an_overrun() {
        let (mut w, mut c, shared) = writer(8); // 4 frames
        w.write(&[1.0f32; 12], 2); // 6 frames offered
        assert_eq!(shared.overruns(), 1);
        assert_eq!(shared.dropped_frames(), 2);
        assert_eq!(shared.capture_frames(), 4);
        assert_eq!(drain(&mut c).len(), 8);
    }

    #[test]
    fn zero_channels_is_a_no_op() {
        let (mut w, mut c, _) = writer(8);
        w.write(&[1.0f32, 2.0], 0);
        assert!(drain(&mut c).is_empty());
    }

    #[test]
    fn errors_reach_the_shared_state() {
        let (w, _c, shared) = writer(8);
        w.report_error("boom");
        let (flags, msg) = shared.take_errors();
        assert_eq!(flags, ERR_CAPTURE);
        assert_eq!(msg.as_deref(), Some("boom"));
    }

    #[test]
    fn latency_is_published() {
        let (w, _c, shared) = writer(8);
        w.set_device_latency(Duration::from_micros(12_500));
        assert_eq!(shared.capture_latency_us(), 12_500);
    }
}
