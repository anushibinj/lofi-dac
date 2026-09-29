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
