//! Channel mapping between any device channel count and the engine's stereo format.
//!
//! Downmix follows the usual ITU-style coefficients (centre and surrounds at -3 dB, LFE dropped)
//! for the default Windows channel orders:
//!
//! | channels | order |
//! | --- | --- |
//! | 3 | FL FR FC |
//! | 4 | FL FR BL BR |
//! | 5 | FL FR FC BL BR |
//! | 6 | FL FR FC LFE BL BR |
//! | 7 | FL FR FC LFE BC SL SR |
//! | 8+ | FL FR FC LFE BL BR SL SR (extra channels ignored) |
//!
//! The downmix is deliberately *not* normalised: stereo content on a 5.1/7.1 device only occupies
//! FL/FR and must keep its level. The engine clamps the final output.

use std::f32::consts::FRAC_1_SQRT_2;

const K: f32 = FRAC_1_SQRT_2;

/// Left/right coefficients of one source channel.
type Coef = (f32, f32);

const FL: Coef = (1.0, 0.0);
const FR: Coef = (0.0, 1.0);
const FC: Coef = (K, K);
const LFE: Coef = (0.0, 0.0);
const BL: Coef = (K, 0.0);
const BR: Coef = (0.0, K);
const BC: Coef = (0.5, 0.5);
const NONE: Coef = (0.0, 0.0);

fn coefficients(channels: usize, index: usize) -> Coef {
    let table: &[Coef] = match channels {
        3 => &[FL, FR, FC],
        4 => &[FL, FR, BL, BR],
        5 => &[FL, FR, FC, BL, BR],
        6 => &[FL, FR, FC, LFE, BL, BR],
        7 => &[FL, FR, FC, LFE, BC, BL, BR],
        _ => &[FL, FR, FC, LFE, BL, BR, BL, BR],
    };
    table.get(index).copied().unwrap_or(NONE)
}

/// Downmixes one interleaved device frame to `(left, right)`.
#[inline]
pub fn frame_to_stereo(frame: &[f32]) -> (f32, f32) {
    match frame.len() {
        0 => (0.0, 0.0),
        1 => (frame[0], frame[0]),
        2 => (frame[0], frame[1]),
        n => {
            let (mut l, mut r) = (0.0, 0.0);
            for (i, &s) in frame.iter().enumerate() {
                let (cl, cr) = coefficients(n, i);
                l += cl * s;
                r += cr * s;
            }
            (l, r)
        }
    }
}

/// Writes a stereo sample pair into one interleaved device frame. Mono devices get the average,
/// stereo devices L/R, and surround devices L/R on the front pair with silence elsewhere.
#[inline]
pub fn stereo_to_frame(l: f32, r: f32, frame: &mut [f32]) {
    match frame.len() {
        0 => {}
        1 => frame[0] = (l + r) * 0.5,
        _ => {
            frame[0] = l;
            frame[1] = r;
            for s in &mut frame[2..] {
                *s = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_is_duplicated() {
        assert_eq!(frame_to_stereo(&[0.4]), (0.4, 0.4));
    }

    #[test]
    fn stereo_is_untouched() {
        assert_eq!(frame_to_stereo(&[0.1, -0.2]), (0.1, -0.2));
    }

    #[test]
    fn empty_frame_is_silence() {
        assert_eq!(frame_to_stereo(&[]), (0.0, 0.0));
    }

    #[test]
    fn stereo_content_on_a_surround_device_keeps_its_level() {
        for channels in [3usize, 4, 5, 6, 7, 8, 10] {
            let mut frame = vec![0.0f32; channels];
            frame[0] = 0.5;
            frame[1] = -0.25;
            assert_eq!(frame_to_stereo(&frame), (0.5, -0.25), "{channels} channels");
        }
    }

    #[test]
    fn centre_is_folded_into_both_sides_at_minus_3db() {
        let mut frame = [0.0f32; 6];
        frame[2] = 1.0; // FC in 5.1
        let (l, r) = frame_to_stereo(&frame);
        assert!((l - FRAC_1_SQRT_2).abs() < 1.0e-6 && (r - FRAC_1_SQRT_2).abs() < 1.0e-6);
    }

    #[test]
    fn lfe_is_dropped() {
        let mut frame = [0.0f32; 6];
        frame[3] = 1.0;
        assert_eq!(frame_to_stereo(&frame), (0.0, 0.0));
    }

    #[test]
    fn rear_channels_go_to_their_own_side() {
        let mut frame = [0.0f32; 6];
        frame[4] = 1.0; // BL
        let (l, r) = frame_to_stereo(&frame);
        assert!(l > 0.7 && r == 0.0);
        let mut frame = [0.0f32; 8];
        frame[7] = 1.0; // SR in 7.1
        let (l, r) = frame_to_stereo(&frame);
        assert!(l == 0.0 && r > 0.7);
    }

    #[test]
    fn upmix_to_mono_averages() {
        let mut f = [9.0f32];
        stereo_to_frame(0.2, 0.6, &mut f);
        assert!((f[0] - 0.4).abs() < 1.0e-6);
    }

    #[test]
    fn upmix_to_surround_fills_front_pair_only() {
        let mut f = [9.0f32; 6];
        stereo_to_frame(0.2, 0.6, &mut f);
        assert_eq!(f, [0.2, 0.6, 0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn empty_output_frame_is_a_no_op() {
        stereo_to_frame(1.0, 1.0, &mut []);
    }
}
