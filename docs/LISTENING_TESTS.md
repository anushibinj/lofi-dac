# Listening tests

The DSP is verified numerically (see `cargo test -p lofify-dsp`), but nobody building Lofify could
*hear* it. These tests are for you. They take about 15 minutes with headphones.

## 1. Render the files

```powershell
cargo run --release -p lofify-cli -- process
```

This writes about 24 WAV files to `samples/out/` (git-ignored) and prints a table of levels. Files
are named `<signal>__<preset>.wav`. Every signal also has a `__original.wav` to compare against.

| Signal | What it is | Best for judging |
| --- | --- | --- |
| `tone` | steady 440 Hz sine | wow, flutter, drift, saturation, bit crushing |
| `sweep` | 20 Hz - 20 kHz sine sweep, 10 s | where the filters cut, aliasing from the crusher |
| `pink` | pink noise | overall tone, hiss, crackle |
| `chords` | plucked chord loop with bass and hat | the overall vibe, reverb, level |

Render your own music too. It is the most honest test:

```powershell
cargo run --release -p lofify-cli -- process --no-generated --input "C:\Music\song.wav"
```

Options: `--preset "Classic Lofi"` (repeatable), `--preset-file my.toml`, `--float` for 32-bit
float output, `--seconds 30`, `--tail 3` for a longer reverb tail. Any WAV works (mono, stereo,
8/16/24/32-bit, 32-bit float); anything above stereo is reduced to its first two channels.

## 2. What to listen for

Play the `__original` file, then the processed one, at a low-to-moderate volume.

### Classic Lofi (`*__classic-lofi.wav`)

| File | You should hear | Red flag |
| --- | --- | --- |
| `tone` | A slow, gentle pitch wobble (about one wave every two seconds) with a faster, subtler shimmer on top. Slight grit. Occasional soft ticks. | Steady vibrato that sounds like a musical effect; sudden pitch jumps; any pops louder than a soft tick |
| `sweep` | The sweep gets dull above about 3 kHz and is almost gone by 8 kHz. The bottom below about 80 Hz fades. Weak aliasing "birdies" are fine. | Loud whistles or a second sweep running the opposite direction (bad aliasing) |
| `pink` | Noise that sounds like it comes through a wall: a soft rumble with little top end, faint hiss and crackle. | Harsh digital fizz |
| `chords` | The lofi sound: warm, slightly muffled, a touch of room, faint crackle, the whole thing gently wandering in pitch. Roughly the same loudness as the original. | Much louder or quieter than the original; the hat disappearing completely; harshness |

### Other presets

| Preset | Character to expect |
| --- | --- |
| **Late Night** | Darker than Classic (cut around 2.8 kHz), a bigger, softer room, much less crackle. Calm. |
| **Vinyl Dust** | Brighter (cut around 6.5 kHz) with obvious pops, ticks and surface hiss, like a worn record. |
| **Cassette** | The most unstable: strong wow and flutter (on `tone` it should sound seasick), warm tape saturation, steady hiss, no crackle. |
| **Clean** | Should be indistinguishable from the original (a fixed 10 ms of latency is removed in the render). If you hear a difference, that is a bug. |

## 3. Checklist of things that must NOT happen

- [ ] Clicks, thumps or "zipper" noise at the start or end of any file.
- [ ] DC offset thump (a low push when a loud note starts).
- [ ] Distortion on quiet material; distortion should only appear on loud material.
- [ ] Any file clipping (the level table prints peaks, all must be at or below 0.891 = -1 dBFS for effect presets).
- [ ] Loudness jumping by more than about 3 dB between presets on `chords`.
- [ ] Stereo image collapsing to mono in `chords` (the plucks are panned left and right).
- [ ] Reverb tail cut off abruptly at the end of a file.
- [ ] Anything that changes over time in an unpleasant way in the middle of a steady `tone` other than the intended wobble.

## 4. Live listening tests (Windows, after setup)

Do these once Lofify is running with your virtual cable (see [SETUP_WINDOWS.md](SETUP_WINDOWS.md)).

1. **Passthrough sanity**: preset `Clean`, play music. It should sound like normal audio with no
   crackles or dropouts for at least a minute. If you hear glitches, watch the underrun counter in
   the app or in `lofify passthrough`.
2. **Bypass**: toggle bypass (Ctrl+Alt+L) while music plays. The change should be smooth, no click,
   and the music must not jump in time.
3. **Live sliders**: while playing a steady tone, drag each slider. There must be no zipper noise
   (a rapid stepping sound); changes should glide.
4. **Silence**: pause playback. After a moment you should hear only the faint hiss and crackle of
   the preset, not stuck noise or buzzing. Resume playback: audio returns without a burst.
5. **Lip sync**: play a video. A little delay is normal, but speech should still look in sync.
   The app shows the measured latency; the target is below about 50 ms with default settings.
6. **Device changes**: unplug headphones while running. Lofify should recover on its own within a
   couple of seconds.

## 5. Reporting problems

Include the file name, the preset, the timestamp and what you heard, plus the printed level table.
For live problems include the log file (see the README for its location).
