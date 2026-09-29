//! Sample format conversion. Internally everything is interleaved `f32` in `[-1, 1]`.

/// A device sample format the engine can read from or write to.
pub trait Pcm: Copy + Send + 'static {
    /// Converts to `f32` in `[-1, 1)`.
    fn to_f32(self) -> f32;
    /// Converts from `f32`, clamping to the valid range. NaN becomes silence.
    fn from_f32(v: f32) -> Self;
}

/// Clamps to `[-1, 1]`; NaN becomes 0.
#[inline]
pub fn clamp_unit(v: f32) -> f32 {
    if v > 1.0 {
        1.0
    } else if v < -1.0 {
        -1.0
    } else if v.is_nan() {
        0.0
    } else {
        v
    }
}

impl Pcm for f32 {
    #[inline]
    fn to_f32(self) -> f32 {
        self
    }
    #[inline]
    fn from_f32(v: f32) -> Self {
        clamp_unit(v)
    }
}

impl Pcm for f64 {
    #[inline]
    fn to_f32(self) -> f32 {
        self as f32
    }
    #[inline]
    fn from_f32(v: f32) -> Self {
        f64::from(clamp_unit(v))
    }
}

impl Pcm for i16 {
    #[inline]
    fn to_f32(self) -> f32 {
        f32::from(self) / 32_768.0
    }
    #[inline]
    fn from_f32(v: f32) -> Self {
        (clamp_unit(v) * 32_767.0).round() as i16
    }
}

impl Pcm for i32 {
    #[inline]
    fn to_f32(self) -> f32 {
        self as f32 / 2_147_483_648.0
    }
    #[inline]
    fn from_f32(v: f32) -> Self {
        (f64::from(clamp_unit(v)) * 2_147_483_647.0).round() as i32
    }
}

impl Pcm for i8 {
    #[inline]
    fn to_f32(self) -> f32 {
        f32::from(self) / 128.0
    }
    #[inline]
    fn from_f32(v: f32) -> Self {
        (clamp_unit(v) * 127.0).round() as i8
    }
}

impl Pcm for u8 {
    #[inline]
    fn to_f32(self) -> f32 {
        (f32::from(self) - 128.0) / 128.0
    }
    #[inline]
    fn from_f32(v: f32) -> Self {
        (clamp_unit(v) * 127.0 + 128.0).round() as u8
    }
}

impl Pcm for u16 {
    #[inline]
    fn to_f32(self) -> f32 {
        (f32::from(self) - 32_768.0) / 32_768.0
    }
    #[inline]
    fn from_f32(v: f32) -> Self {
        (clamp_unit(v) * 32_767.0 + 32_768.0).round() as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T: Pcm>(v: f32) -> f32 {
        T::from_f32(v).to_f32()
    }

    #[test]
    fn integer_formats_roundtrip_within_one_step() {
        for v in [-1.0f32, -0.75, -0.1, 0.0, 0.3, 0.999, 1.0] {
            assert!((roundtrip::<i16>(v) - v).abs() <= 1.0 / 16_384.0, "i16 {v}");
            assert!((roundtrip::<u16>(v) - v).abs() <= 1.0 / 16_384.0, "u16 {v}");
            assert!((roundtrip::<i32>(v) - v).abs() <= 1.0e-6, "i32 {v}");
            assert!((roundtrip::<i8>(v) - v).abs() <= 1.0 / 64.0, "i8 {v}");
            assert!((roundtrip::<u8>(v) - v).abs() <= 1.0 / 64.0, "u8 {v}");
            assert!((roundtrip::<f32>(v) - v).abs() < 1.0e-9);
            assert!((roundtrip::<f64>(v) - v).abs() < 1.0e-9);
        }
    }

    #[test]
    fn out_of_range_is_clamped_not_wrapped() {
        assert_eq!(i16::from_f32(5.0), i16::MAX);
        assert_eq!(i16::from_f32(-5.0), -i16::MAX);
        assert_eq!(i32::from_f32(1.0e9), i32::MAX);
        assert_eq!(u8::from_f32(9.0), 255);
        assert_eq!(f32::from_f32(3.0), 1.0);
        assert_eq!(u16::from_f32(-3.0), 1);
    }

    #[test]
    fn nan_and_infinity_become_safe_values() {
        assert_eq!(i16::from_f32(f32::NAN), 0);
        assert_eq!(f32::from_f32(f32::NAN), 0.0);
        assert_eq!(i16::from_f32(f32::INFINITY), i16::MAX);
        assert_eq!(f32::from_f32(f32::NEG_INFINITY), -1.0);
    }

    #[test]
    fn silence_is_zero() {
        assert_eq!(0i16.to_f32(), 0.0);
        assert_eq!(32_768u16.to_f32(), 0.0);
        assert_eq!(128u8.to_f32(), 0.0);
        assert_eq!(u16::from_f32(0.0), 32_768);
    }

    #[test]
    fn full_scale_input_reaches_full_scale_output() {
        assert!((i16::MIN.to_f32() + 1.0).abs() < 1.0e-6);
        assert!((i16::MAX.to_f32() - 1.0).abs() < 1.0e-4);
        assert!((i32::MIN.to_f32() + 1.0).abs() < 1.0e-6);
    }
}
