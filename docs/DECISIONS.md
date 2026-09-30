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
20 ms prefill and under 30 ms end to end. These cannot all hold. Measured in the simulation with two
10 ms device buffers, the default configuration is about **55 ms**: ~10 ms DSP delay, ~30 ms of audio
waiting in the ring (the 20 ms reserve, plus half a 10 ms callback, plus the 1.3 ms resampler
lookahead, on average), and 20 ms of device buffering. `--low-latency` (10 ms reserve, 3 ms tape
delay) is about 40 ms. Real numbers depend on the devices and are shown live in the UI and CLI. The
10 ms base delay is a constant of the wet path and is compensated in the dry path, so it never causes
comb filtering.

**D014 - Clock drift is absorbed by a PI controller on the resampling ratio.** A proportional-only
controller left the ring 40 % away from target at 400 ppm and dipped into an underrun. The controller
now has an integral term that learns the clock offset, so the ring settles *at* its target (verified
to within 2.5 ms for +-400 ppm in deterministic tests). The ratio is clamped to +-0.5 %.

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

**D018 - The prefill is a reserve kept *after* each callback.** Capture delivers audio in bursts (one
device period at a time), so the ring level right *before* a pull depends on whether a burst landed
just before or just after the callback. With "start when the ring holds 20 ms", a burst arriving just
after a callback left zero margin, which showed up as roughly one underrun every 10 seconds in a
real-time soak test and was reproduced with a bursty-capture model. Playback now starts when the ring
holds the reserve *plus* the audio this callback consumes, the overrun trim and the drift controller
aim at the same level, and the resampler's chunk lookahead (64 frames; 256 frames halved the margin)
is included in the reserve. A late capture burst is now tolerated by up to about the reserve (18 ms
at every phase in the tests).

**D019 - GUI stack: eframe 0.35 with the `glow` renderer only, no AccessKit.** The default `wgpu`
renderer roughly doubles the build and brings nothing a small control panel needs. AccessKit
(screen-reader support) is left out to keep the build lean; it can be enabled with one feature flag.
egui_plot 0.36 is used because 0.37 needs a newer Rust than the installed 1.94.

**D020 - Hide-to-tray relies on `App::logic`.** eframe 0.35 documents that `App::logic` also runs while
the window is hidden after `request_repaint()`. The tray menu and the hotkey handlers send an
`AppCommand` on a channel and request a repaint; `logic` drains the channel, so Show, Toggle bypass
and Quit work while the window is hidden. This is the one place where correct behaviour depends on
OS event-loop details that could not be tested here (UNTESTED on real desktops).

**D021 - The first-run wizard exists only on Windows.** It is the virtual-cable flow. macOS uses a
Core Audio tap and needs no cable, so on other systems the wizard is skipped and marked done.

**D022 - Single instance is a plain exit.** A second launch prints a message and exits with success;
it does not signal the first instance to show its window (that needs IPC and was not worth the risk).
The tray icon is the way back to the window.

**D023 - Settings are TOML, saved atomically and debounced.** A corrupt settings file is never fatal:
defaults are used, a warning is shown and the bad file is kept as `settings.toml.bad`. Devices are stored
by name because ids change when drivers are reinstalled. Values loaded from disk are clamped to the
valid ranges.

**D024 - macOS capture is a Core Audio process tap, opened after the output.** The tap must exclude
Lofify's own process, which only has a Core Audio object once it is playing, so backends can declare
`capture_needs_output_first`; the engine then opens the output first and uses the sample rate from
the device listing (the default output device's rate, which also clocks the aggregate device). A rate
mismatch is reported as an error. The system tap appears as the pseudo input device "System Audio
(Core Audio tap)" and `Auto` prefers it over any cable. The macOS-only module is compile-checked
and clippy-clean but UNTESTED: nothing in it has ever run against Core Audio.

**D025 - Error states retry every 5 seconds, waiting states every poll.** Problems that need the user
(feedback protection, unsupported platform, missing permission) are retried slowly so a pending macOS
permission prompt is not re-triggered every 1.5 s. Missing devices are retried at the normal poll rate.

**D026 - BlackHole and Soundflower count as virtual drivers for feedback protection.** The brief
requires refusing outputs whose name contains "CABLE"; on macOS the common equivalents are BlackHole
and Soundflower, so they are refused as outputs and ranked with VoiceMeeter for auto-detection.

**D027 - Windows needs a virtual device: loopback alone cannot silence the original sound.** OBS
records desktop audio with no driver because WASAPI loopback is a *tap* on a playback endpoint's
mix: it copies the audio and the original still reaches the speakers. OBS only listens, so that is
fine for it. Lofify must *replace* the sound, so playing the processed copy on the same speakers
would mix it with the untouched original, about 55 ms late (echo and comb filtering). Options
considered (research only, nothing of this could be run here):

* Mute or lower the speaker endpoint: whether loopback still sees the audio is inconsistent between
  drivers and Windows versions, and it would mute Lofify's own output on the same endpoint.
* Mute every other app's audio session and capture with process loopback (Windows 10 2004+, exclude
  our own process tree): session volume and mute are applied before the mix, so muting an app also
  changes what a loopback hears, and process loopback's behaviour there is undocumented. It would also
  leave every app muted in the Volume Mixer after a crash. Not built.
* Exclusive-mode output: shared-mode apps go silent, so there is nothing left to capture.
* Cancel the original with an inverted copy: the copy is only known after the original has already
  been mixed, so it can never arrive in time.
* Ship our own virtual audio driver: still a driver install, and needs driver signing.

Result: one virtual device (VB-Cable, or any existing one) stays the supported route, as in the brief.
Two cases need no install at all: a playback endpoint nobody listens to (for example a monitor's
HDMI audio with no speakers) can play the "tap" role, see docs/SETUP_WINDOWS.md. macOS is different
because Core Audio's process tap can mute what it captures (D024), which is why it needs no driver.
The remaining onboarding friction (switching the default playback device by hand) is what the
optional auto-route in D010 would remove.

