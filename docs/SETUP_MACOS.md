# macOS setup

macOS 14.2 and newer can tap the system's audio without any virtual driver, so there is nothing to
install. Lofify captures **all** audio your Mac plays, mutes the original, and plays the processed
result on your real speakers or headphones.

> **Status: UNTESTED on a real Mac.** The Core Audio tap code was written against the
> `objc2-core-audio` bindings and is only compile-checked (`cargo check --target
> aarch64-apple-darwin`). The engine logic around it (ordering, buffering, reconnect) is tested
> against a simulated backend. Expect rough edges and please report what you find.

## Requirements

* macOS **14.2 (Sonoma) or newer**. Older systems get the message *"system audio capture needs
  macOS 14.2 or newer"*; see the BlackHole alternative below.
* Apple Silicon or Intel.

## How it works

```
 all apps' audio ──► Core Audio process tap ──► Lofify ──► effects ──► default output (your speakers)
                       (global stereo mix,           ▲
                        Lofify itself excluded,      └─ original audio is muted while the tap runs
                        muted when tapped)
```

1. Lofify opens its output stream on the default output device first. It has to be playing before
   macOS gives the process an audio object.
2. It creates a **global stereo tap that excludes Lofify's own process**, with the mute behaviour
   *muted when tapped*. Without the exclusion Lofify would record its own output.
3. It wraps the tap in a private aggregate device (drift compensation on, clocked by the default
   output device) and reads audio from it.

Because the tap belongs to the Lofify process, **if Lofify quits or crashes macOS removes the tap and
your normal audio comes back** by itself. (Expected from how taps work; not verified.) This is the
big difference from Windows, where quitting Lofify leaves you silent until you change the default
device.

## Permission

The first time Lofify starts the tap, macOS asks whether it may record system audio. Allow it.
If you missed the prompt or said no, enable it here:

**System Settings > Privacy & Security > Screen & System Audio Recording** and switch on
**System Audio Recording Only** for Lofify (or for your terminal, see below).

Lofify shows *"macOS refused to create the system audio tap"* while permission is missing and
retries every few seconds, so it starts working soon after you grant it.

### Terminal versus app bundle

* A bare binary started with `cargo run` inherits the permission of the terminal it runs from
  (Terminal, iTerm, VS Code...). Grant it to that program.
* An `.app` bundle needs its own permission and an `NSAudioCaptureUsageDescription` entry in its
  `Info.plist`, which `packaging/macos/make-app.sh` writes for you (see [PACKAGING.md](PACKAGING.md)).

## Running

```sh
cargo run --release -p lofify-cli -- devices                 # lists "System Audio (Core Audio tap)"
cargo run --release -p lofify-cli -- passthrough --output "MacBook Pro Speakers"
cargo run --release -p lofify-cli -- live --preset "Classic Lofi" --output "MacBook Pro Speakers"
cargo run --release -p lofify-app                            # the app
```

`--capture auto` (the default) picks the system tap automatically; `--capture system` names it
explicitly. Choose your real speakers or headphones as the output.

The tap is clocked by the **default output device**. If you change the default output while Lofify
runs, it rebuilds its streams within a couple of seconds.

## Notes

* Bluetooth headphones add their own delay (often 100 ms or more). Wired or built-in output gives
  the lowest latency.
* Apps that play DRM-protected content may be silent in the tap.
* System sounds and every app are captured; there is no per-app selection.
* Lofify refuses to use BlackHole, Soundflower or anything with "CABLE" in its name as its
  *output*, because that would feed its own audio back into itself.

## If you are on macOS older than 14.2

There is no tap, so use a virtual audio driver such as [BlackHole](https://existential.audio/blackhole/)
(installed by you, from its author):

1. Install BlackHole 2ch and set it as the system output (Sound settings).
2. `lofify live --capture "BlackHole 2ch" --output "MacBook Pro Speakers"`.

You will hear nothing whenever Lofify is not running, exactly as on Windows.

## Troubleshooting

| Symptom | What to check |
| --- | --- |
| "macOS refused to create the system audio tap" | Grant *System Audio Recording* permission (above). |
| "system audio capture needs macOS 14.2" | Update macOS or use BlackHole. |
| Silence and "waiting for audio" | Play something. Check *Underruns* in the app or `lofify passthrough`. |
| Crackles | Try `--prefill-ms 40`. |
| Original sound still audible together with the effect | The tap mutes only while running; check that Lofify shows *Running*. |
