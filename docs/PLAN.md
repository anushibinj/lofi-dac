# Lofify build plan

Lofify captures **all** system audio, runs it through a real-time lofi effects chain and plays the
result on the user's real speakers/headphones. Everything is Rust, one Cargo workspace.

## Host used for development

| Item | Value |
| --- | --- |
| Host OS | Linux x86_64 (cloud sandbox, no audio hardware) |
| rustc / cargo | 1.94.1 stable |
| git | 2.43 |
| Extra targets installed | `x86_64-pc-windows-msvc`, `aarch64-apple-darwin` (compile-check only) |

Consequences:

* Windows and macOS code **cannot be run** here. It is gated behind `cfg(target_os = ...)`, is
  compile-checked with `cargo check/clippy --target ...`, and is marked **UNTESTED** in the docs.
* The engine core (ring buffer, resampling, channel mapping, prefill/underrun state machine,
  reconnect logic) is platform independent and is tested on Linux through a simulated backend.
* DSP is verified numerically and by offline WAV rendering (nobody here can listen).

## Phases

Each phase ends with the gates `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`
and `cargo test --workspace`, and is committed in many small commits.

| # | Scope | Acceptance |
| --- | --- | --- |
| 0 | Workspace, CI, lints, README skeleton, doc stubs | gates pass |
| 1 | `lofify-dsp`, tests, presets, CLI `process` | DSP tests pass, rendered WAVs exist |
| 2 | Engine on Windows: `devices`, passthrough with metrics, reconnect, feedback protection | passthrough logic tested via simulated backend; Windows path compile-checked (UNTESTED on hardware) |
| 3 | DSP wired into the engine, live parameters, bypass crossfade, presets | CLI `live` runs the chain with a chosen preset |
| 4 | egui app, tray, hotkey, settings, wizard, autostart | app builds; flow documented (UNTESTED on Windows hardware) |
| 5 | macOS Core Audio tap backend + docs | compile-checked for `aarch64-apple-darwin` (UNTESTED on a Mac) |
| 6 | Packaging: Inno Setup script, `.app` bundle script, release profile, ARCHITECTURE.md, final README | scripts present, release profile builds |

## Out of scope

APO driver, Linux/PipeWire, Android, iOS, slowed-down tempo (needs large buffering).
