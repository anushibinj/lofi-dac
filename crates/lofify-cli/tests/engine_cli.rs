//! End-to-end tests of `lofify devices`, `passthrough` and `live` (all in simulation mode, so
//! they run on any OS without audio hardware).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::{Command, Output};

fn lofify(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lofify"))
        .args(args)
        .env("RUST_BACKTRACE", "0")
        .output()
        .unwrap()
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn devices_lists_the_simulated_machine_and_the_cable() {
    let o = lofify(&["devices", "--simulate"]);
    assert!(o.status.success(), "{}", text(&o));
    let t = text(&o);
    assert!(t.contains("Speakers (Realtek Audio)"));
    assert!(t.contains("Virtual cable detected"));
}

#[test]
fn devices_on_an_unsupported_os_is_a_clean_error() {
    if lofify_engine::platform::is_supported() {
        return;
    }
    let o = lofify(&["devices"]);
    assert!(!o.status.success());
    let t = text(&o);
    assert!(
        t.starts_with("error: ") && t.contains("--simulate") && !t.contains("panicked"),
        "{t}"
    );
}

#[test]
fn passthrough_runs_and_reports_metrics() {
    let o = lofify(&[
        "passthrough",
        "--simulate",
        "--output",
        "Speakers",
        "--seconds",
        "3",
        "--prefill-ms",
        "40",
    ]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("running") && t.contains("lat "), "{t}");
    assert!(t.contains("summary") && t.contains("underruns"), "{t}");
    assert!(
        t.contains("output          Speakers (Realtek Audio) (48000 Hz, 2 ch)"),
        "{t}"
    );
    assert!(t.contains("mean latency"), "{t}");
}

#[test]
fn passthrough_refuses_the_cable_as_output() {
    let o = lofify(&["passthrough", "--simulate", "--seconds", "2"]);
    let t = text(&o);
    assert!(
        o.status.success(),
        "the command itself runs; the engine reports the refusal: {t}"
    );
    assert!(
        t.contains("ERROR: refusing to start to avoid a feedback loop"),
        "{t}"
    );
    assert!(t.contains("hint: choose your real speakers"), "{t}");
    assert!(
        !t.contains("running  lat"),
        "must never reach the running state: {t}"
    );
}

#[test]
fn live_runs_a_preset_and_rejects_unknown_ones() {
    let o = lofify(&[
        "live",
        "--simulate",
        "--output",
        "Headphones",
        "--preset",
        "cassette",
        "--seconds",
        "2",
        "--prefill-ms",
        "40",
    ]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("preset: Cassette"), "{t}");

    let o = lofify(&["live", "--simulate", "--preset", "Nope"]);
    assert!(!o.status.success());
    assert!(text(&o).contains("unknown preset") && text(&o).contains("Classic Lofi"));
}

#[test]
fn live_can_start_bypassed() {
    let o = lofify(&[
        "live",
        "--simulate",
        "--output",
        "Speakers",
        "--bypass",
        "--seconds",
        "2",
        "--prefill-ms",
        "40",
    ]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("(bypassed)"), "{t}");
}
