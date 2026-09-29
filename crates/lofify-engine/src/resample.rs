//! Resampling stage between the capture ring and the DSP chain.
//!
//! It always runs (even when both devices use the same nominal rate) because two clocks never
//! agree exactly: the ratio is nudged by a few hundred ppm to keep the ring at its target fill,
//! which absorbs clock drift without ever dropping or repeating audio.
//!
//! * Equal rates: cubic polynomial interpolation (about 2 frames of delay).
//! * Different rates: windowed-sinc interpolation (about 64 frames of delay).

use crate::error::{EngineError, Result};
use rtrb::Consumer;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Adjustable, Async, FixedAsync, PolynomialDegree, Resampler, SincInterpolationParameters,
    SincInterpolationType, WindowFunction,
};

/// Output frames produced per resampler call.
pub const CHUNK_FRAMES: usize = 256;
/// Largest `pull` request the stage is sized for.
pub const MAX_PULL_FRAMES: usize = 4096;
/// The ratio may be nudged by at most this fraction either way.
pub const MAX_NUDGE: f64 = 0.005;

/// Stereo resampler with an output FIFO so callers can ask for arbitrary frame counts.
pub struct ResampleStage {
    resampler: Async<f32>,
    in_buf: Vec<f32>,
    out_buf: Vec<f32>,
    fifo: Vec<f32>,
    fifo_frames: usize,
    relative: f64,
    errors: u64,
}

impl std::fmt::Debug for ResampleStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResampleStage")
            .field("fifo_frames", &self.fifo_frames)
            .field("relative", &self.relative)
            .finish_non_exhaustive()
    }
}

impl ResampleStage {
    /// Creates a stage converting `rate_in` to `rate_out`.
    pub fn new(rate_in: u32, rate_out: u32) -> Result<Self> {
        if rate_in == 0 || rate_out == 0 {
            return Err(EngineError::UnsupportedFormat("sample rate of 0 Hz".into()));
        }
        let ratio = f64::from(rate_out) / f64::from(rate_in);
        let max_relative = 1.0 + 2.0 * MAX_NUDGE;
        let resampler = if rate_in == rate_out {
            Async::<f32>::new_poly(
                ratio,
                max_relative,
                PolynomialDegree::Cubic,
                CHUNK_FRAMES,
                2,
                FixedAsync::Output,
            )
        } else {
            let params = SincInterpolationParameters {
                sinc_len: 128,
                f_cutoff: Some(0.95),
                oversampling_factor: 128,
                interpolation: SincInterpolationType::Cubic,
                window: WindowFunction::BlackmanHarris2,
            };
            Async::<f32>::new_sinc(
                ratio,
                max_relative,
                &params,
                CHUNK_FRAMES,
                2,
                FixedAsync::Output,
            )
        }
        .map_err(|e| EngineError::UnsupportedFormat(format!("cannot create resampler: {e}")))?;

        let in_buf = vec![0.0; resampler.input_frames_max() * 2];
        let out_buf = vec![0.0; resampler.output_frames_max() * 2];
        let fifo = vec![0.0; (MAX_PULL_FRAMES + resampler.output_frames_max() + CHUNK_FRAMES) * 2];
        Ok(Self {
            resampler,
            in_buf,
            out_buf,
            fifo,
            fifo_frames: 0,
            relative: 1.0,
            errors: 0,
        })
    }

    /// Frames of delay this stage adds (interpolator delay plus frames waiting in the FIFO).
    pub fn delay_frames(&self) -> usize {
        self.resampler.output_delay() + self.fifo_frames
    }

    /// Nudges the conversion ratio relative to nominal. `1.0` is exact; values above 1.0 make
    /// the stage produce more output per input frame. Clamped to `1 +- MAX_NUDGE`.
    pub fn set_relative_ratio(&mut self, relative: f64) {
        let relative = relative.clamp(1.0 - MAX_NUDGE, 1.0 + MAX_NUDGE);
        if (relative - self.relative).abs() < 1.0e-7 {
            return;
        }
        self.relative = relative;
        if self
            .resampler
            .set_resample_ratio_relative(relative, true)
            .is_err()
        {
            self.errors += 1;
        }
    }

    /// Resampler errors seen so far (should stay zero).
    pub fn errors(&self) -> u64 {
        self.errors
    }

    /// Fills `dst` (stereo interleaved) with up to `dst.len() / 2` frames, pulling input from
    /// `input` as needed. Returns the number of frames produced; fewer than requested means the
    /// ring ran dry.
    pub fn pull(&mut self, input: &mut Consumer<f32>, dst: &mut [f32]) -> usize {
        let want = (dst.len() / 2).min(MAX_PULL_FRAMES);
        while self.fifo_frames < want {
            let need_in = self.resampler.input_frames_next();
            if need_in > self.in_buf.len() / 2 || input.slots() < need_in * 2 {
                break;
            }
            let (got, _) = input.pop_partial_slice(&mut self.in_buf[..need_in * 2]);
            if got.len() != need_in * 2 {
                break;
            }
            let out_frames = self.resampler.output_frames_next();
            let result = match (
                InterleavedSlice::new(&self.in_buf[..need_in * 2], 2, need_in),
                InterleavedSlice::new_mut(&mut self.out_buf[..out_frames * 2], 2, out_frames),
            ) {
                (Ok(inp), Ok(mut out)) => self.resampler.process_into_buffer(&inp, &mut out, None),
                _ => {
                    self.errors += 1;
                    break;
                }
            };
            match result {
                Ok((_, produced)) => {
                    let room = self.fifo.len() / 2 - self.fifo_frames;
                    let produced = produced.min(room);
                    let at = self.fifo_frames * 2;
                    self.fifo[at..at + produced * 2].copy_from_slice(&self.out_buf[..produced * 2]);
                    self.fifo_frames += produced;
                }
                Err(_) => {
                    self.errors += 1;
                    break;
                }
            }
        }
        let n = want.min(self.fifo_frames);
        dst[..n * 2].copy_from_slice(&self.fifo[..n * 2]);
        self.fifo.copy_within(n * 2..self.fifo_frames * 2, 0);
        self.fifo_frames -= n;
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtrb::RingBuffer;
    use std::f32::consts::TAU;

    fn feed_tone(p: &mut rtrb::Producer<f32>, rate: u32, hz: f32, frames: usize, offset: usize) {
        let data: Vec<f32> = (offset..offset + frames)
            .flat_map(|i| {
                let v = (TAU * hz * i as f32 / rate as f32).sin() * 0.5;
                [v, v]
            })
            .collect();
        let (pushed, rest) = p.push_partial_slice(&data);
        assert_eq!(pushed.len(), data.len());
        assert!(rest.is_empty());
    }

    /// Pulls `frames` frames in chunks the way the renderer does.
    fn pull_frames(
        stage: &mut ResampleStage,
        c: &mut rtrb::Consumer<f32>,
        frames: usize,
    ) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * 2);
        let mut remaining = frames;
        while remaining > 0 {
            let n = remaining.min(1_000);
            let mut buf = vec![0.0f32; n * 2];
            let got = stage.pull(c, &mut buf);
            out.extend_from_slice(&buf[..got * 2]);
            if got < n {
                break;
            }
            remaining -= n;
        }
        out
    }

    /// Estimates frequency from rising zero crossings.
    fn frequency(samples: &[f32], rate: u32) -> f32 {
        let left: Vec<f32> = samples.chunks_exact(2).map(|f| f[0]).collect();
        let crossings: Vec<usize> = (1..left.len())
            .filter(|&i| left[i - 1] < 0.0 && left[i] >= 0.0)
            .collect();
        let (first, last) = (crossings[0], crossings[crossings.len() - 1]);
        (crossings.len() - 1) as f32 * rate as f32 / (last - first) as f32
    }

    #[test]
    fn same_rate_is_transparent_apart_from_a_tiny_delay() {
        let (mut p, mut c) = RingBuffer::<f32>::new(2 * 48_000);
        feed_tone(&mut p, 48_000, 1_000.0, 20_000, 0);
        let mut stage = ResampleStage::new(48_000, 48_000).unwrap();
        let out = pull_frames(&mut stage, &mut c, 10_000);
        assert_eq!(out.len(), 2 * 10_000);
        let f = frequency(&out[2 * 2_000..], 48_000);
        assert!((f - 1_000.0).abs() < 2.0, "{f} Hz");
        let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.5).abs() < 0.01, "peak {peak}");
    }

    #[test]
    fn upsampling_44k1_to_48k_keeps_the_pitch() {
        let (mut p, mut c) = RingBuffer::<f32>::new(2 * 48_000);
        feed_tone(&mut p, 44_100, 440.0, 30_000, 0);
        let mut stage = ResampleStage::new(44_100, 48_000).unwrap();
        let out = pull_frames(&mut stage, &mut c, 20_000);
        assert_eq!(out.len(), 2 * 20_000);
        let f = frequency(&out[2 * 2_000..], 48_000);
        assert!((f - 440.0).abs() < 1.0, "{f} Hz");
    }

    #[test]
    fn downsampling_96k_to_48k_keeps_the_pitch() {
        let (mut p, mut c) = RingBuffer::<f32>::new(2 * 100_000);
        feed_tone(&mut p, 96_000, 1_000.0, 60_000, 0);
        let mut stage = ResampleStage::new(96_000, 48_000).unwrap();
        let out = pull_frames(&mut stage, &mut c, 20_000);
        assert_eq!(out.len(), 2 * 20_000);
        let f = frequency(&out[2 * 2_000..], 48_000);
        assert!((f - 1_000.0).abs() < 2.0, "{f} Hz");
    }

    #[test]
    fn running_dry_returns_a_short_count() {
        let (mut p, mut c) = RingBuffer::<f32>::new(2 * 48_000);
        feed_tone(&mut p, 48_000, 1_000.0, 1_000, 0);
        let mut stage = ResampleStage::new(48_000, 48_000).unwrap();
        let mut out = vec![0.0f32; 2 * 4_000];
        let n = stage.pull(&mut c, &mut out);
        assert!(n > 0 && n < 1_000, "{n} frames");
        assert_eq!(stage.errors(), 0);
    }

    #[test]
    fn small_and_odd_pull_sizes_are_seamless() {
        // Pulling in odd-sized pieces must give the same audio as one big pull.
        let make = || {
            let (mut p, c) = RingBuffer::<f32>::new(2 * 48_000);
            feed_tone(&mut p, 48_000, 700.0, 20_000, 0);
            (c, ResampleStage::new(48_000, 48_000).unwrap())
        };
        let (mut c1, mut s1) = make();
        let big = pull_frames(&mut s1, &mut c1, 9_000);
        assert_eq!(big.len(), 2 * 9_000);

        let (mut c2, mut s2) = make();
        let mut pieces = Vec::new();
        for size in [1usize, 7, 480, 333, 1024, 2, 3000, 4000] {
            let mut buf = vec![0.0f32; 2 * size];
            assert_eq!(s2.pull(&mut c2, &mut buf), size);
            pieces.extend(buf);
        }
        let n = pieces.len().min(big.len());
        assert!(n >= 2 * 8_000);
        assert_eq!(&pieces[..n], &big[..n]);
    }

    #[test]
    fn ratio_nudge_changes_the_consumption_rate() {
        let consumed = |relative: f64| {
            let (mut p, mut c) = RingBuffer::<f32>::new(2 * 200_000);
            feed_tone(&mut p, 48_000, 500.0, 150_000, 0);
            let mut stage = ResampleStage::new(48_000, 48_000).unwrap();
            stage.set_relative_ratio(relative);
            let mut out = vec![0.0f32; 2 * 4_000];
            // let the ramp finish
            for _ in 0..5 {
                stage.pull(&mut c, &mut out);
            }
            let before = c.slots();
            for _ in 0..20 {
                stage.pull(&mut c, &mut out);
            }
            (before - c.slots()) / 2
        };
        let nominal = consumed(1.0);
        let slower = consumed(1.004); // more output per input => consumes less input
        let faster = consumed(0.996);
        assert!(nominal > 79_000 && nominal < 81_000, "{nominal}");
        assert!(slower < nominal - 200, "slower {slower} nominal {nominal}");
        assert!(faster > nominal + 200, "faster {faster} nominal {nominal}");
    }

    #[test]
    fn nudge_is_clamped() {
        let mut stage = ResampleStage::new(48_000, 48_000).unwrap();
        stage.set_relative_ratio(2.0);
        stage.set_relative_ratio(0.1);
        assert_eq!(stage.errors(), 0);
    }

    #[test]
    fn zero_rate_is_rejected() {
        assert!(ResampleStage::new(0, 48_000).is_err());
        assert!(ResampleStage::new(48_000, 0).is_err());
    }
}
