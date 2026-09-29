# Lofify

Turn **everything** your computer plays into lofi.

Lofify captures all system audio, runs it through a real-time lofi effects chain and plays the
result on your real speakers or headphones. Music, videos, games, calls: whatever the computer
plays gets the tape-and-vinyl treatment, with a one-key bypass to hear the original again.

> **Screenshot placeholders** (no desktop was available while this was written; add PNGs to
> `docs/images/` and link them here):
>
> * `docs/images/main-window.png`: power and bypass buttons, preset picker, sliders, level meter.
> * `docs/images/effect-curve.png`: the frequency-response plot next to the parameter sliders.
> * `docs/images/wizard.png`: the first-run wizard, "Find the virtual cable" step.

## What it does

The effects chain, in order (every parameter is live-adjustable, smoothed over 10 ms, no clicks):

1. **Tape wow and flutter**: two LFOs plus filtered noise modulate a fractional delay (Catmull-Rom).
2. **Saturation**: `tanh` soft clipping with a matching gain compensation.
3. **Bitcrusher**: sample-rate reduction (sample and hold) and bit-depth reduction with TPDF dither.
4. **Filters**: high-pass, 4th-order low-pass ("muffle") and a low-shelf for warmth.
5. **Reverb**: Freeverb (8 combs, 4 all-passes) with room size, damping and mix.
6. **Vinyl**: random crackle pops (Poisson timing, shaped spectrum) plus a quiet hiss floor.
7. **Soft limiter**: transparent under -3 dBFS, never above -1 dBFS.

Wet/dry is delay-compensated so the dry signal stays phase-aligned with the tape path. **Bypass**
is a 20 ms crossfade to the (delay-aligned) dry signal, bit-exact once settled.

Five presets ship built in: **Classic Lofi**, **Late Night**, **Vinyl Dust**, **Cassette** and
**Clean** (a passthrough). You can save your own from the app; they are plain TOML files.

The desktop app adds a tray icon (closing the window hides it there), a global bypass hotkey
(default `Ctrl+Alt+L`), start at login, remembered settings and a first-run wizard.

## How it works

Windows has no public "capture everything" switch for a single app to flip, so Lofify uses the
standard approach: a **virtual audio cable** (VB-Cable, not bundled, you install it yourself) becomes
your default playback device, Lofify records from its other end, applies the effects and plays the
result on your real output. On **macOS 14.2+** Lofify instead uses a Core Audio **process tap**, so no
extra driver is needed, only the "System Audio Recording" permission.

```
apps -> default output (CABLE Input) => CABLE Output -> [capture] -> ring buffer -> DSP chain -> speakers
```

The output device is the clock master; a PI controller nudges a resampler to absorb clock drift
between the two devices. Details: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Quick start

### Windows (primary target)

1. Install [VB-Cable](https://vb-audio.com/Cable/) yourself. Lofify never installs drivers.
2. In Windows Sound settings, set **CABLE Input** as the default playback device.
3. Start Lofify. The wizard checks the cable, picks your real speakers as the output and starts.
4. Press the power button. To go back to normal sound, press bypass (`Ctrl+Alt+L`) or switch the
   default playback device back.

Full walkthrough and troubleshooting: [docs/SETUP_WINDOWS.md](docs/SETUP_WINDOWS.md).

### macOS 14.2 or later (second target)

1. Start Lofify and press the power button.
2. Approve the **System Audio Recording** prompt (System Settings, Privacy and Security).

Full walkthrough: [docs/SETUP_MACOS.md](docs/SETUP_MACOS.md). Older macOS versions can use
BlackHole or Soundflower the same way as the Windows cable.

## Build and run

Requires Rust 1.94 or newer (pinned in `rust-toolchain.toml`). On Linux the workspace builds and tests
everything except the real audio backends.

```sh
# build everything
cargo build --release --workspace

# the desktop app
cargo run --release -p lofify-app

# the command line tool (binary name: lofify)
cargo run --release -p lofify-cli -- devices                  # list devices, detect the cable
cargo run --release -p lofify-cli -- live --preset "Classic Lofi"
cargo run --release -p lofify-cli -- live --capture loopback --output "Speakers"
cargo run --release -p lofify-cli -- passthrough --seconds 30 # no effects, prints metrics

# render every preset to WAV files for listening (no audio device needed)
cargo run --release -p lofify-cli -- process                  # writes samples/out/
cargo run --release -p lofify-cli -- process -i song.wav -p "Late Night"

# try the live path on any machine with a simulated Windows audio stack (its default output is the
# cable, so name the speakers explicitly)
cargo run --release -p lofify-cli -- live --simulate --output Speakers --seconds 20
```

Checks that CI runs (and that must pass before a change is merged):

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Installers: see [docs/PACKAGING.md](docs/PACKAGING.md) (Inno Setup for Windows, a script that builds an
unsigned `.app` for macOS).

## Repository layout

| Crate | Purpose |
| --- | --- |
| `crates/lofify-dsp` | Pure DSP, presets, shared atomic parameters. No audio I/O, `#![forbid(unsafe_code)]` |
| `crates/lofify-engine` | Ring buffer, resampler, drift control, device handling and the per-OS backends |
| `crates/lofify-cli` | `lofify` command line tool: `process`, `devices`, `passthrough`, `live` |
| `crates/lofify-app` | egui desktop app: tray icon, hotkey, settings, wizard, start at login |

Other folders: `packaging/` (installer and bundle scripts), `docs/`, `samples/` (rendered output,
git-ignored).

## Status

What has been **verified** (on Linux, by tests and offline rendering):

* The DSP chain: frequency response, pitch modulation depth, bit-exact bypass, delay alignment,
  limiter ceiling, no allocation and no NaN in the audio path (114 tests in `lofify-dsp` alone).
* The engine core against a simulated device layer with real threads: startup, drift, underruns,
  device removal and return, feedback protection, restart, 10-minute soak with a 150 ppm clock
  mismatch ([log](docs/soak-passthrough-10min.log): 3 underruns, 0 overruns, 0 restarts).
* The egui app: rendering and click-through tests with a headless renderer.

What is **UNTESTED** (compile-checked for `x86_64-pc-windows-msvc` and `aarch64-apple-darwin`, never run
on real hardware or an OS with a desktop):

* The Windows WASAPI backend (`cpal`), including loopback capture and device hot-plug.
* The macOS process tap backend and permission flow.
* Tray icon, global hotkey, start at login, single-instance guard.
* The Inno Setup installer, the `.app` bundle script and the release workflow.
* Everything you would have to **hear**: see [docs/LISTENING_TESTS.md](docs/LISTENING_TESTS.md) for a
  checklist.

## Known issues and limits

* **Latency is about 55 ms** with default settings (goal was under 30 ms). `--low-latency` gets to
  about 40 ms on the simulated stack; real devices may need the larger buffers. Fine for music and
  video (Lofify does not delay video), noticeable for games and calls.
* Windows: routing to the cable is a manual step. Automatically switching the default device is
  deliberately not implemented (Lofify does not change system audio settings).
* The app does not start a second instance; it just exits.
* Installers are unsigned, so Windows SmartScreen and macOS Gatekeeper will warn.
* Windows and macOS system-audio capture cannot separate apps, so everything is processed. There
  is no per-application mode.
* Nobody has listened to this build yet. The numbers say the effects behave; whether they sound
  good is what [docs/LISTENING_TESTS.md](docs/LISTENING_TESTS.md) is for.

Design decisions and their reasons are in [docs/DECISIONS.md](docs/DECISIONS.md).

## Docs

* [Plan](docs/PLAN.md) and [decisions](docs/DECISIONS.md)
* [Architecture](docs/ARCHITECTURE.md)
* [Windows setup](docs/SETUP_WINDOWS.md)
* [macOS setup](docs/SETUP_MACOS.md)
* [Packaging](docs/PACKAGING.md)
* [Listening tests](docs/LISTENING_TESTS.md)

## License

Dual licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your option.
