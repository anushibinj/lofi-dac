//! `lofify process`: render presets offline on generated signals and user WAV files.

use crate::wav::{Audio, read_wav, write_wav};
use anyhow::{Context, Result, bail};
use clap::Args;
use lofify_dsp::chain::Chain;
use lofify_dsp::params::Params;
use lofify_dsp::presets::{Preset, builtin_presets};
use lofify_dsp::signals::{chords, peak, pink_noise, rms_dbfs, sine_sweep, sine_tone};
use std::path::{Path, PathBuf};

/// Size of the blocks fed to the chain, similar to a real audio callback.
const BLOCK_FRAMES: usize = 512;

#[derive(Args, Debug)]
pub struct ProcessArgs {
    /// WAV file(s) to render in addition to the generated test signals.
    #[arg(short, long)]
    pub input: Vec<PathBuf>,

    /// Preset name to render, or "all". May be repeated.
    #[arg(short, long, default_value = "all")]
    pub preset: Vec<String>,

    /// TOML preset file(s) to render as well.
    #[arg(long)]
    pub preset_file: Vec<PathBuf>,

    /// Output directory (git-ignored by default).
    #[arg(short, long, default_value = "samples/out")]
    pub out: PathBuf,

    /// Sample rate of the generated test signals.
    #[arg(long, default_value_t = 48_000)]
    pub sample_rate: u32,

    /// Length in seconds of the generated test signals.
    #[arg(long, default_value_t = 10.0)]
    pub seconds: f32,

    /// Skip the generated sweep, pink noise, chords and 440 Hz tone.
    #[arg(long)]
    pub no_generated: bool,

    /// Write 32-bit float WAVs instead of 16-bit PCM.
    #[arg(long)]
    pub float: bool,

    /// Extra seconds of silence rendered after the input so reverb tails can ring out.
    #[arg(long, default_value_t = 2.0)]
    pub tail: f32,
}

/// Turns a display name into a file-name friendly slug: "Classic Lofi" -> "classic-lofi".
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Renders `input` through the chain. The output is aligned with the input (the chain's fixed
/// latency is removed) and extended by `tail_seconds` so reverb tails are audible.
pub fn render(params: &Params, sample_rate: u32, input: &[f32], tail_seconds: f32) -> Vec<f32> {
    let mut chain = Chain::new(sample_rate as f32);
    chain.set_params(params);
    chain.snap();
    let latency = chain.latency_samples();
    let tail = (tail_seconds.max(0.0) * sample_rate as f32) as usize;
    let mut buf = Vec::with_capacity(input.len() + 2 * (tail + latency));
    buf.extend_from_slice(input);
    buf.resize(input.len() + 2 * (tail + latency), 0.0);
    for block in buf.chunks_mut(BLOCK_FRAMES * 2) {
        chain.process(block);
    }
    buf.drain(..latency * 2);
    buf.truncate(input.len() + 2 * tail);
    buf
}

fn resolve_presets(args: &ProcessArgs) -> Result<Vec<Preset>> {
    let builtin = builtin_presets();
    let mut chosen: Vec<Preset> = Vec::new();
    for want in &args.preset {
        if want.eq_ignore_ascii_case("all") {
            for p in &builtin {
                if !chosen.iter().any(|c| c.name == p.name) {
                    chosen.push(p.clone());
                }
            }
        } else if let Some(p) = builtin
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(want.trim()))
        {
            if !chosen.iter().any(|c| c.name == p.name) {
                chosen.push(p.clone());
            }
        } else {
            let names: Vec<&str> = builtin.iter().map(|p| p.name.as_str()).collect();
            bail!("unknown preset {want:?}; available: {}", names.join(", "));
        }
    }
    for file in &args.preset_file {
        let text = std::fs::read_to_string(file)
            .with_context(|| format!("cannot read {}", file.display()))?;
        let preset = Preset::from_toml(&text)
            .with_context(|| format!("invalid preset file {}", file.display()))?;
        chosen.push(preset);
    }
    Ok(chosen)
}

fn generated(args: &ProcessArgs) -> Vec<(String, Audio)> {
    let sr = args.sample_rate as f32;
    let make = |samples| Audio {
        sample_rate: args.sample_rate,
        samples,
    };
    vec![
        (
            "sweep".to_string(),
            make(sine_sweep(sr, args.seconds, 20.0, 20_000.0, 0.5)),
        ),
        (
            "pink".to_string(),
            make(pink_noise(sr, args.seconds, -18.0, 1)),
        ),
        ("chords".to_string(), make(chords(sr, args.seconds, 0.5))),
        (
            "tone".to_string(),
            make(sine_tone(sr, args.seconds, 440.0, 0.25)),
        ),
    ]
}

/// Runs the subcommand.
pub fn run(args: &ProcessArgs) -> Result<()> {
    let presets = resolve_presets(args)?;
    if presets.is_empty() {
        bail!("no presets selected");
    }
    let mut sources: Vec<(String, Audio)> = Vec::new();
    if !args.no_generated {
        sources.extend(generated(args));
    }
    for path in &args.input {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("input");
        sources.push((slug(stem), read_wav(path)?));
    }
    if sources.is_empty() {
        bail!("nothing to render: pass --input or drop --no-generated");
    }

    println!(
        "{:<10} {:<13} {:>9} {:>9} {:>8}   file",
        "signal", "preset", "in RMS", "out RMS", "peak"
    );
    let mut written = 0;
    for (name, audio) in &sources {
        let original: PathBuf = args.out.join(format!("{name}__original.wav"));
        write_wav(&original, audio, args.float)?;
        written += 1;
        for preset in &presets {
            let out = render(&preset.params, audio.sample_rate, &audio.samples, args.tail);
            let file = args.out.join(format!("{name}__{}.wav", slug(&preset.name)));
            write_wav(
                &file,
                &Audio {
                    sample_rate: audio.sample_rate,
                    samples: out.clone(),
                },
                args.float,
            )?;
            written += 1;
            println!(
                "{:<10} {:<13} {:>7.1} dB {:>7.1} dB {:>8.3}   {}",
                name,
                preset.name,
                rms_dbfs(&audio.samples),
                rms_dbfs(&out[..audio.samples.len()]),
                peak(&out),
                display(&file)
            );
        }
    }
    println!("\nwrote {written} files to {}", display(&args.out));
    Ok(())
}

fn display(p: &Path) -> String {
    p.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("Classic Lofi"), "classic-lofi");
        assert_eq!(slug("  Vinyl  Dust! "), "vinyl-dust");
        assert_eq!(slug("Late_Night (v2)"), "late-night-v2");
        assert_eq!(slug("***"), "");
    }

    #[test]
    fn render_is_aligned_and_has_tail() {
        let input = vec![0.25f32; 2 * 4_800];
        let out = render(&Params::default(), 48_000, &input, 0.5);
        assert_eq!(out.len(), input.len() + 2 * 24_000);
        // Clean chain: aligned copy of the input, then silence.
        assert!(
            out[..input.len()]
                .iter()
                .all(|&v| (v - 0.25).abs() < 1.0e-6)
        );
        assert!(out[input.len()..].iter().all(|&v| v == 0.0));
    }

    #[test]
    fn reverb_tail_is_audible_after_the_input_ends() {
        let mut p = Params::default();
        p.reverb.mix = 1.0;
        p.reverb.room = 0.8;
        let mut input = vec![0.0f32; 2 * 4_800];
        input[..2 * 480].iter_mut().for_each(|v| *v = 0.4);
        let out = render(&p, 48_000, &input, 1.0);
        assert!(rms_dbfs(&out[input.len()..input.len() + 2 * 9_600]) > -60.0);
    }

    #[test]
    fn unknown_preset_lists_alternatives() {
        let args = ProcessArgs {
            input: vec![],
            preset: vec!["Nope".into()],
            preset_file: vec![],
            out: PathBuf::from("unused"),
            sample_rate: 48_000,
            seconds: 1.0,
            no_generated: false,
            float: false,
            tail: 0.0,
        };
        let err = resolve_presets(&args).unwrap_err().to_string();
        assert!(
            err.contains("Classic Lofi") && err.contains("Cassette"),
            "{err}"
        );
    }
}
