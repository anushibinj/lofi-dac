# Architecture

Lofify is one Cargo workspace of four crates. Everything is Rust.

```
crates/
  lofify-dsp      pure DSP, no I/O, #![forbid(unsafe_code)]
  lofify-engine   audio I/O: devices, ring buffer, resampling, control thread, backends
  lofify-cli      `lofify` command line: process, devices, passthrough, live
  lofify-app      egui desktop app: tray, hotkey, settings, wizard
```

Dependencies only point downwards: `app -> engine -> dsp` and `cli -> engine -> dsp`.

## Data flow

```
 capture device                                                   output device
      |  (capture callback)                                            ^  (output callback = CLOCK MASTER)
      v                                                                |
 CaptureWriter --> rtrb ring buffer (interleaved stereo f32) --> OutputRenderer
   any format/channels -> f32 stereo      lock-free SPSC          pop -> resample (drift PI) -> DSP chain -> device format
```

* The **output callback drives everything**. Each callback pops what it needs from the ring,
  resamples (always: it nudges the ratio by a few hundred ppm to absorb clock drift, and converts
  between different device rates), runs the DSP chain in place and converts to the device's
  sample format and channel count.
* Internally everything is **interleaved `f32` stereo**. Any channel count is mapped to stereo
  (ITU-style downmix) and back (front pair only).
* Audio callbacks never allocate, lock, log, do I/O or block. Two integration tests with a counting
  global allocator prove this for the DSP chain and for capture + render (including the resampler,
  every state transition and every sample format).

## Threads

| Thread | Owns | Talks to others via |
| --- | --- | --- |
| UI (egui main thread) | window, tray, hotkey | command channel to the engine, atomics for parameters and metrics |
| Engine control thread | the backend, both streams, reconnect logic | `crossbeam-channel` commands in, atomics + one mutex (status) out |
| Capture callback (OS thread) | `CaptureWriter` | ring buffer producer, atomics |
| Output callback (OS thread) | `OutputRenderer`, DSP `Chain` | ring buffer consumer, atomics |

cpal streams may not be `Send`, so the backend is created *inside* the control thread from a
factory closure and never leaves it.

Parameters cross threads as `AtomicU32` bit patterns (`SharedParams`); the audio thread loads the
whole set once per callback and every parameter is smoothed per sample (10 ms one-pole), so torn
reads and slider drags never zipper. Status flows the other way through `Shared` (atomics).

## The DSP chain (`lofify-dsp`)

```
input -> trim -> tape wow/flutter -> saturation -> bitcrusher -> HPF/LPF/shelf -> reverb
      -> + crackle/hiss -> output gain -> soft limiter -> [wet/dry] -> out
input -> [dry delay = tape base delay] ----------------------------^
```

* The wet path has a fixed latency (the tape's 10 ms base delay). The **dry path is delayed by the
  same number of samples**, so wet/dry mixing and the bypass crossfade never comb-filter. Fully
  bypassed output is the input delayed by `Chain::latency_samples()`, bit exact.
* Every effect has an identity setting, so "off" is continuous with "on" and the `Clean` preset is
  transparent (tested bit-exactly).
* The muffle low-pass is 4th order (two biquads) because one section only gives ~14 dB at 10 kHz.
* `frequency_response()` is computed analytically from the biquad coefficients; the UI plots it.
* Denormals are flushed inside feedback paths, NaN/inf are replaced by silence, and the wet path
  is soft-limited to -1 dBFS.

See [DECISIONS.md](DECISIONS.md) for why the numbers are what they are.

## The engine (`lofify-engine`)

| Module | Job |
| --- | --- |
| `types`, `feedback` | device model, selections, virtual-cable detection, feedback protection |
| `pcm`, `mapping` | sample-format and channel-count conversion |
| `capture` | `CaptureWriter`: device buffer -> stereo f32 -> ring |
| `resample` | rubato-based stage with a PI drift controller on the ratio |
| `output` | `OutputRenderer`: prefill, underrun, overrun, idle handling, fades, DSP, metering |
| `metrics` | lock-free counters, meters, stream-error flags |
| `backend` | `AudioBackend`, `CaptureSource`, `OutputSink` traits |
| `engine` | control thread: start/stop, device polling, automatic rebuilds |
| `cpal_backend` | WASAPI / Core Audio via cpal (`cfg(windows or macos)`) |
| `mac_tap` | Core Audio process tap capture (`cfg(macos)`) |
| `sim` | simulated backend used by tests and `--simulate` |

### Output state machine

```
 Waiting --(ring >= reserve + this callback)--> Running
 Running --(ring cannot supply a callback)----> Waiting   (fade out what we have, count a glitch
                                                           only if capture returns within 0.5 s)
```

`reserve` (the prefill, 20 ms by default) is kept in the ring **after** each callback: capture
delivers audio in bursts, so it is the level after a pull that protects against a late burst.
A ring above `max_fill` drops its oldest frames, which bounds latency after a stall.

### Reconnect logic

The control thread polls the device list every 1.5 s (and reacts immediately to stream errors) and
rebuilds both streams when a device disappears, the default output changes, or a stream reports a
fatal error. While devices are missing it keeps retrying in the `Waiting` state; problems that need
the user (feedback protection, unsupported platform, permission) go to `Error`. Before anything is
opened, `check_feedback` refuses an output that is a virtual cable (name contains "CABLE") or is
the same endpoint as the capture source.

## Platform backends

| Platform | Capture | Output | Status |
| --- | --- | --- | --- |
| Windows | virtual cable's recording side (or WASAPI loopback of a playback device) | WASAPI via cpal | compile-checked, **untested on hardware** |
| macOS 14.2+ | Core Audio process tap, global stereo tap excluding Lofify, muted while tapped | Core Audio via cpal | compile-checked, **untested on hardware** |
| other | none (`--simulate` only) | | out of scope |

## Testing strategy

Nobody who wrote this could hear it or run it on real devices, so:

* **DSP**: numeric tests (filter attenuation, pitch deviation, bit-exact bypass, impulse alignment,
  NaN safety, timing, no allocation) and offline WAV rendering for listening
  ([LISTENING_TESTS.md](LISTENING_TESTS.md)).
* **Engine**: deterministic simulated-clock tests of the renderer (bursty capture, clock drift with a
  negative control, idle gaps, overruns), end-to-end tests of the control thread against the
  simulated backend, a real-time soak test (`lofify passthrough --simulate`).
* **App**: pure-logic tests (settings, hotkey parsing, wizard rules) and headless egui frames.
* **Platform glue** (WASAPI, Core Audio, tray, hotkey, autostart): compile-checked with
  `cargo check/clippy --target x86_64-pc-windows-msvc` and `aarch64-apple-darwin` only.
