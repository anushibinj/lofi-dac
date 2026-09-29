//! End-to-end test of `lofify process` through the real binary.

use std::process::Command;

fn lofify() -> Command {
    Command::new(env!("CARGO_BIN_EXE_lofify"))
}

fn read(path: &std::path::Path) -> (hound::WavSpec, Vec<i16>) {
    let mut r = hound::WavReader::open(path).unwrap();
    (r.spec(), r.samples::<i16>().map(Result::unwrap).collect())
}

#[test]
fn renders_requested_presets_for_generated_signals() {
    let dir = tempfile::tempdir().unwrap();
    let status = lofify()
        .args([
            "process",
            "--seconds",
            "1",
            "--tail",
            "0.2",
            "--preset",
            "Clean",
            "--preset",
            "classic lofi",
            "--out",
        ])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );

    for signal in ["sweep", "pink", "chords"] {
        for variant in ["original", "clean", "classic-lofi"] {
            let f = dir.path().join(format!("{signal}__{variant}.wav"));
            assert!(f.exists(), "missing {}", f.display());
        }
    }
    assert!(!dir.path().join("sweep__cassette.wav").exists());

    // Clean is transparent: identical to the original within one 16-bit step (plus the tail).
    let (spec, orig) = read(&dir.path().join("chords__original.wav"));
    let (_, clean) = read(&dir.path().join("chords__clean.wav"));
    assert_eq!(
        (spec.channels, spec.sample_rate, spec.bits_per_sample),
        (2, 48_000, 16)
    );
    assert_eq!(clean.len(), orig.len() + 2 * 9_600);
    assert!(
        orig.iter()
            .zip(&clean)
            .all(|(a, b)| (i32::from(*a) - i32::from(*b)).abs() <= 1)
    );

    // Classic Lofi is not.
    let (_, lofi) = read(&dir.path().join("chords__classic-lofi.wav"));
    let diff: i64 = orig
        .iter()
        .zip(&lofi)
        .map(|(a, b)| i64::from((a - b).abs()))
        .sum();
    assert!(
        diff > orig.len() as i64 * 50,
        "classic lofi should change the audio, diff {diff}"
    );
}

#[test]
fn renders_a_user_supplied_wav() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("My Song.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&input, spec).unwrap();
    for i in 0..44_100 {
        w.write_sample((f32::from(i as u16 % 200) * 60.0 - 6_000.0) as i16)
            .unwrap();
    }
    w.finalize().unwrap();

    let out = dir.path().join("out");
    let run = lofify()
        .args([
            "process",
            "--no-generated",
            "--preset",
            "Cassette",
            "--tail",
            "0",
            "--input",
        ])
        .arg(&input)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let (spec, samples) = read(&out.join("my-song__cassette.wav"));
    assert_eq!(spec.sample_rate, 44_100);
    assert_eq!(samples.len(), 2 * 44_100);
}

#[test]
fn unknown_preset_fails_with_a_helpful_message() {
    let dir = tempfile::tempdir().unwrap();
    let run = lofify()
        .args(["process", "--preset", "Nope", "--out"])
        .arg(dir.path())
        .output()
        .unwrap();
    assert!(!run.status.success());
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(
        err.contains("unknown preset") && err.contains("Classic Lofi"),
        "{err}"
    );
}

#[test]
fn custom_preset_files_are_rendered() {
    let dir = tempfile::tempdir().unwrap();
    let preset = dir.path().join("mine.toml");
    std::fs::write(
        &preset,
        "name = \"My Crush\"\n[crush]\nbits = 4.0\nsample_rate_hz = 8000.0\n",
    )
    .unwrap();
    let out = dir.path().join("out");
    let run = lofify()
        .args([
            "process",
            "--seconds",
            "1",
            "--tail",
            "0",
            "--preset",
            "Clean",
            "--preset-file",
        ])
        .arg(&preset)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(out.join("chords__my-crush.wav").exists());
}
