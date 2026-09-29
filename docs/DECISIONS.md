# Decisions log

Format: **ID - decision** followed by the reason. Newest at the bottom.

**D001 - Workspace root is the repository root.** The brief shows a `lofify/` directory; the repo
is already named `lofi-dac`, so the workspace lives at the repo root instead of nesting a folder.

**D002 - Edition 2024, stable toolchain.** `rust-toolchain.toml` pins `channel = "stable"` with
rustfmt and clippy. Installed toolchain is 1.94.1.

**D003 - No pushing.** The brief says not to push, so all work stays in local commits on the
designated branch until the user says otherwise.

**D004 - Engine core is platform independent, cpal is `cfg(windows|macos)` only.** cpal needs ALSA
headers on Linux and Linux is out of scope. Gating the dependency lets `cargo clippy/test --workspace`
run on any host, and lets the engine logic be tested with a simulated backend.

**D005 - Lints.** `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `dbg_macro` are
denied workspace-wide. Tests are exempted via `clippy.toml`. Startup code that must abort uses a
narrowly scoped `#[allow]` with a message.

**D006 - The muffle low-pass is 4th order (two cascaded biquads).** The brief requires more than
20 dB attenuation at 10 kHz for a 4.5 kHz cutoff. A single RBJ section with Q 0.7 gives only about
14 dB there, the cascade gives about 28 dB. The user-facing Q drives the first section, the second
is Butterworth. The high-pass is a single section (26 dB down at 20 Hz for a 90 Hz cutoff).

**D007 - Parameters cross threads as flat atomics; "off" is an identity state.** Every effect has a
value at which it is transparent (drive 0, 24 bits, sample rate at or above the device rate,
reverb mix 0, hiss at -90 dB, pops 0) so smoothing between "off" and "on" is continuous. Filters
have explicit enable flags that crossfade over 10 ms while the filter keeps running.

**D008 - Bypass keeps the dry path delay-compensated forever.** The wet path has a fixed latency
of the tape base delay (10 ms by default). The dry path is delayed by exactly the same integer
number of samples, so wet/dry mixing and the bypass crossfade never comb-filter. Fully bypassed
output is therefore the input delayed by `Chain::latency_samples()`, bit exact. Dropping to zero
latency when bypassed would skip 10 ms of audio and click.

**D009 - cpal 0.18.2, not 0.17.x.** The brief said to verify the latest 0.17.x API; the latest
release is 0.18.2, so that is what is used. Verified against its source: on WASAPI, recording from a
*playback* endpoint enables loopback but reports no input configs (so the loopback stream config
comes from `default_output_config()`), capture only accepts the device's native format, and output
streams use `AUTOCONVERTPCM`. The `realtime` feature (MMCSS thread priority) is enabled on Windows.

**D010 - Windows "auto-route" is not implemented.** The optional stretch (switch the Windows default
output to the cable on start and restore it on exit, panic and next launch) needs the undocumented
`IPolicyConfig` COM interface. It cannot be tested here and a bug would leave the user with no audio
device selected, so it is skipped, as the brief allows. The docs tell users to switch the default
themselves and how to switch it back.

**D011 - The output callback is the clock master; one `Shared` block per engine.** Capture pushes into
an `rtrb` ring, the output callback pops, resamples, runs the DSP chain in place and converts to the
device format. Metrics and error flags live in one atomic `Shared` that survives stream rebuilds, so
counters keep running across reconnects and reset only on a fresh `start`.

**D012 - Idle capture is silence, not an error.** WASAPI loopback stops delivering packets when
nothing plays. A capture gap longer than 0.5 s is idle: no underrun is counted, the DSP chain keeps
running on silence (so reverb tails ring out) and playback resumes with a 5 ms input fade. Shorter
gaps that recover count as one underrun each. On an underrun the audio already available is faded
out, silence is output and the ring is re-prefilled.

**D013 - Latency budget vs the "under 30 ms" target.** The brief asks for a 10 ms tape base delay, a
20 ms prefill and under 30 ms end to end. These cannot all hold: the defaults add up to about 40-55 ms
(10 ms DSP + 20 ms ring + capture and output device buffers of about 10 ms each). The measured value
is reported honestly in the UI and CLI. `--low-latency` (10 ms prefill, 3 ms tape delay) gets to
about 25-35 ms on stable devices. The 10 ms base delay is a constant of the wet path and is
compensated in the dry path, so it never causes comb filtering.

**D014 - Clock drift is absorbed by a PI controller on the resampling ratio.** A proportional-only
controller left the ring 40 % away from target at 400 ppm and dipped into an underrun. The controller
now has an integral term that learns the clock offset, so the ring settles *at* the 20 ms target
(verified to within 2.5 ms for +-400 ppm in deterministic tests). The ratio is clamped to +-0.5 %.
The resampler works in 64 frame chunks; 256 frame chunks halved the jitter margin (5 ms instead of
10 ms), which showed up as underruns in a real-time soak test.

**D015 - Panics never reach an audio thread.** All calls into cpal and both audio callbacks are wrapped
in `catch_unwind` (cpal's own `description()` contains an `expect`, and a device unplugged during
enumeration must not kill the engine thread). The audio path itself contains no `unwrap`, `expect`
or `panic`, and integration tests with a counting allocator prove it never allocates.

**D016 - A simulated backend exists so the engine can be tested end to end anywhere.** It runs real
threads against a wall clock, drives the real capture writer and output renderer, and can unplug
devices, change the default output, stop capture packets, skew the capture clock and inject stream
errors. `lofify passthrough --simulate` uses it. It does **not** prove anything about real device
timing.

**D017 - Surround downmix is not normalised.** Stereo content on a 5.1/7.1 device only occupies the
front pair and must keep its level, so centre and surrounds are added at -3 dB without dividing by
the channel count. The output clamps, and the DSP limiter guards the wet path.
