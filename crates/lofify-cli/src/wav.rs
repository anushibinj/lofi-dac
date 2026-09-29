//! Minimal WAV reading and writing (via `hound`), always in interleaved stereo `f32`.

use anyhow::{Context, Result, bail};
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use std::path::Path;

/// Decoded stereo audio.
#[derive(Debug, Clone, PartialEq)]
pub struct Audio {
    pub sample_rate: u32,
    /// Interleaved stereo samples.
    pub samples: Vec<f32>,
}

/// Reads any integer (8-32 bit) or 32-bit float WAV file and converts it to stereo.
/// Mono is duplicated to both channels; for more than two channels only the first two are used.
pub fn read_wav(path: &Path) -> Result<Audio> {
    let mut reader =
        WavReader::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let spec = reader.spec();
    if spec.channels == 0 {
        bail!("{} has no channels", path.display());
    }
    let raw: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()?
        }
    };
    let ch = usize::from(spec.channels);
    let mut samples = Vec::with_capacity(raw.len() / ch * 2);
    for frame in raw.chunks_exact(ch) {
        let l = frame[0];
        let r = if ch > 1 { frame[1] } else { l };
        samples.push(l);
        samples.push(r);
    }
    Ok(Audio {
        sample_rate: spec.sample_rate,
        samples,
    })
}

/// Writes 16-bit PCM (clamped) or, with `float`, 32-bit float.
pub fn write_wav(path: &Path, audio: &Audio, float: bool) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let spec = WavSpec {
        channels: 2,
        sample_rate: audio.sample_rate,
        bits_per_sample: if float { 32 } else { 16 },
        sample_format: if float {
            SampleFormat::Float
        } else {
            SampleFormat::Int
        },
    };
    let mut writer = WavWriter::create(path, spec)
        .with_context(|| format!("cannot create {}", path.display()))?;
    for &s in &audio.samples {
        if float {
            writer.write_sample(s)?;
        } else {
            writer.write_sample((s.clamp(-1.0, 1.0) * 32_767.0).round() as i16)?;
        }
    }
    writer.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_audio() -> Audio {
        Audio {
            sample_rate: 44_100,
            samples: (0..200).map(|i| (i as f32 * 0.05).sin() * 0.5).collect(),
        }
    }

    #[test]
    fn float_roundtrip_is_exact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        let a = sample_audio();
        write_wav(&path, &a, true).unwrap();
        assert_eq!(read_wav(&path).unwrap(), a);
    }

    #[test]
    fn pcm16_roundtrip_is_within_one_lsb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        let a = sample_audio();
        write_wav(&path, &a, false).unwrap();
        let b = read_wav(&path).unwrap();
        assert_eq!(b.sample_rate, 44_100);
        for (x, y) in a.samples.iter().zip(&b.samples) {
            assert!((x - y).abs() < 1.0 / 32_768.0 + 1.0e-6);
        }
    }

    #[test]
    fn mono_is_duplicated_and_surround_truncated() {
        let dir = tempfile::tempdir().unwrap();
        for (channels, name) in [(1u16, "mono.wav"), (3, "tri.wav")] {
            let path = dir.path().join(name);
            let spec = WavSpec {
                channels,
                sample_rate: 8_000,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            };
            let mut w = WavWriter::create(&path, spec).unwrap();
            for frame in 0..4i16 {
                for c in 0..channels {
                    w.write_sample((frame + 1) * 1000 + i16::try_from(c).unwrap())
                        .unwrap();
                }
            }
            w.finalize().unwrap();
            let a = read_wav(&path).unwrap();
            assert_eq!(a.samples.len() / 2, 4);
            if channels == 1 {
                assert_eq!(a.samples[0], a.samples[1]);
            } else {
                assert_ne!(a.samples[0], a.samples[1]);
            }
        }
    }

    #[test]
    fn missing_file_is_an_error() {
        assert!(read_wav(Path::new("/definitely/not/here.wav")).is_err());
    }

    #[test]
    fn writer_clamps_out_of_range_samples() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hot.wav");
        let a = Audio {
            sample_rate: 8_000,
            samples: vec![3.0, -3.0],
        };
        write_wav(&path, &a, false).unwrap();
        let b = read_wav(&path).unwrap();
        assert!(b.samples[0] > 0.99 && b.samples[1] < -0.99);
    }
}
