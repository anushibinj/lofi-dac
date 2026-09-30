# Windows setup

Windows has no built-in way to *tap* system audio and play it back through effects, so Lofify
uses a **virtual audio cable**. This is a one-time setup and takes about five minutes.

> **Status: UNTESTED on real Windows hardware.** The WASAPI backend compiles for Windows and the
> whole engine (buffering, resampling, reconnect, feedback protection) is tested against a
> simulated backend, but no one has run it against real devices yet. Please report what you find.

## How it works

```
 your apps ──► Windows default playback = "CABLE Input"
                          │  (virtual cable)
                          ▼
              "CABLE Output" (a recording device)
                          │
                          ▼
                       Lofify  ──► lofi effects ──► your REAL speakers / headphones
```

Every app plays to the cable because the cable is the Windows default output. Lofify records the
other end of the cable, processes it, and plays it on your real device.

## 1. Install a virtual cable

Lofify does **not** bundle or install any driver (VB-Cable's licence does not allow
redistribution, and drivers should come from their author).

1. Download **VB-CABLE Virtual Audio Device** from <https://vb-audio.com/Cable/> (it is free,
   donationware).
2. Unzip it, right-click `VBCABLE_Setup_x64.exe` and choose **Run as administrator**, then
   click *Install Driver*.
3. **Reboot.**

After the reboot two new devices exist: **CABLE Input** (a playback device) and
**CABLE Output** (a recording device). Lofify also auto-detects VoiceMeeter and other devices
with "Virtual" in their name, but VB-Cable is the tested path.

## 2. Make the cable your default playback device

Lofify never changes your audio settings for you; you do this yourself:

* **Windows 11**: *Settings → System → Sound → Output*, choose **CABLE Input**.
* **Windows 10 / classic**: press `Win+R`, run `mmsys.cpl`, on the *Playback* tab right-click
  **CABLE Input (VB-Audio Virtual Cable)** and choose **Set as Default Device**.

From now on Windows sounds go into the cable and you will hear **nothing** until Lofify runs.
That is expected.

## 3. Start Lofify and pick your real output

* **App**: the first-run wizard walks through detecting the cable, this step, choosing your real
  speakers or headphones, and confirming audio is detected.
* **Command line**:
  ```powershell
  lofify devices                       # find the exact device names
  lofify passthrough --output "Speakers"   # clean test, prints latency and glitch counters
  lofify live --output "Speakers" --preset "Classic Lofi"
  ```

Choose your real speakers or headphones as the output. **Lofify refuses to start** if the output
is the cable or is the same device it captures, because that would create a feedback loop.

## If Lofify is closed or crashes, you hear silence

Because the cable is the default output, **when Lofify is not running, nothing plays**. To get sound
back, switch the default playback device to your real speakers or headphones (same place as
step 2). Do this before you uninstall or stop using Lofify.

Tips:

* Windows 11 has a quick switcher: click the speaker icon in the tray and pick a device.
* The tray icon menu has **Quit**; quitting does not restore your default device.
* An optional "auto-route" feature (switch the default on start, restore on exit and on next
  launch) is not implemented yet. See [DECISIONS.md](DECISIONS.md).

## Recommended settings

* Set **CABLE Input**, **CABLE Output** and your speakers to the **same sample rate** (for
  example 48000 Hz) in *Sound → Device properties → Advanced*. Lofify resamples if they differ,
  but matching rates is cleanest.
* Do **not** turn on *Listen to this device* for CABLE Output (you would hear everything twice
  and could create a loop).
* Apps using **exclusive mode** (some games, DAWs, ASIO) bypass the Windows mixer and cannot be
  captured. Switch them to shared mode.
* Bluetooth headphones add their own delay (100 ms or more) on top of Lofify's. Wired or USB
  devices give the lowest latency.

## Alternative: loopback capture without the cable's recording side

Windows lets an app record what a *playback* device is playing (WASAPI loopback). To use it:

```powershell
lofify live --capture "loopback:CABLE Input" --output "Speakers"
```

This behaves like recording CABLE Output. Recording loopback of your *real speakers* is not
useful (you would hear the original and the processed audio together), and Lofify refuses to
capture the same device it outputs to.

## Can I skip the cable?

Windows can hand Lofify a copy of what any playback device plays (that is how OBS records desktop
audio, no driver needed). But a copy is all it is: the original keeps playing on that device, and
Windows offers no reliable way to silence it while still copying it. So Lofify needs a playback device that
you **cannot hear** to act as the "tap". Either:

* **The virtual cable** (above). Works on every PC. One install, one time.
* **A playback device you never listen to.** If your PC has one (for example a monitor connected by
  HDMI or DisplayPort that has no speakers, listed under Sound settings, Output), then:
  1. Set that device as the Windows default playback device.
  2. In Lofify, pick **Loopback: \<that device\>** as the capture source and your real speakers or
     headphones as the output.
  Nothing is installed. This has never been run on real hardware (UNTESTED), and it only works if the
  device stays active (a monitor that is switched off may disappear from the list).

Recording loopback of the speakers you are also listening on cannot work, and Lofify refuses it.

## Troubleshooting

| Symptom | What to check |
| --- | --- |
| Silence | Is Lofify running? Is the default playback device **CABLE Input**? Is the output set to your real speakers? |
| "Virtual cable not found" | Reboot after installing; run `lofify devices` and look for **CABLE Output** under capture sources. |
| "refusing to start ... feedback loop" | The output is the cable. Pick your real speakers. |
| Crackles or dropouts | Watch the underrun counter (`lofify passthrough`). Try `--prefill-ms 40`, close CPU-heavy apps, avoid USB hubs. |
| Latency too high | Default is about 40-50 ms. Try `--low-latency` (about 25-30 ms, needs stable devices). |
| Pitch sounds off | Sample rates differ and resampling is off? Match the rates as described above. |
| Some app is not captured | It is probably using exclusive mode. |
| Audio disappears after unplugging headphones | Lofify rebuilds within a couple of seconds; if the Windows default changed to something else, choose the output again. |

## Uninstalling

1. Set your real speakers as the default playback device.
2. Quit Lofify.
3. Uninstall *VB-Audio Virtual Cable* from *Settings → Apps* (or run its installer again and
   choose *Remove Driver*), then reboot.

## Where Lofify keeps things

| What | Location |
| --- | --- |
| Settings | `%APPDATA%\Lofify\config\settings.toml` |
| Log files (one per day) | `%LOCALAPPDATA%\Lofify\data\logs\lofify.log.<date>` |
| Your presets | `%APPDATA%\Lofify\config\presets\` |

(The exact paths come from the OS config directories and are shown at the bottom of the app's Settings panel.)
