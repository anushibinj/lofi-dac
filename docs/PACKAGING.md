# Packaging

> **UNTESTED**: the installer script, the bundle script and the release workflow have never been run
> (this was developed on Linux). Their invariants are checked by `crates/lofify-app/tests/packaging.rs`.

## Release profile

`Cargo.toml` sets `opt-level = 3`, fat LTO, one codegen unit and `strip = true` for release builds.
`panic` stays `unwind` because the audio callbacks are wrapped in `catch_unwind`.

```sh
cargo build --release -p lofify-app -p lofify-cli
```

On Windows the C runtime is linked statically (`.cargo/config.toml`), so the installer does not need
the Visual C++ redistributable. The release GUI build has no console window.

## Windows installer (Inno Setup)

Requirements: Rust stable, [Inno Setup 6](https://jrsoftware.org/isinfo.php).

```powershell
powershell -ExecutionPolicy Bypass -File packaging\windows\build-installer.ps1
```

Output: `dist\Lofify-<version>-setup.exe`. The installer:

* installs `lofify-app.exe` and the `lofify.exe` command line tool, the licences and the setup guide,
  per user by default (no administrator prompt; the dialog can switch to all users);
* adds a Start menu entry and an optional desktop shortcut;
* shows a page explaining that a virtual audio cable is needed and must be installed separately;
* removes the "launch at login" registry value on uninstall.

**It never bundles or installs a driver.** VB-Cable's licence does not allow redistribution, and
drivers should come from their author.

Code signing is not configured. Unsigned installers trigger SmartScreen warnings; sign the exe and the
installer (`SignTool=` in the `.iss`) before distributing widely.

## macOS app bundle

```sh
packaging/macos/make-app.sh                 # native architecture
UNIVERSAL=1 packaging/macos/make-app.sh     # arm64 + x86_64
CODESIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" packaging/macos/make-app.sh
```

Output: `dist/Lofify.app` and `dist/Lofify-<version>-macos.zip`. The script writes `Info.plist` from
`packaging/macos/Info.plist.in`, including **`NSAudioCaptureUsageDescription`**, which macOS requires
before an app may record system audio through a tap, and `LSMinimumSystemVersion` 14.2.

* Without `CODESIGN_IDENTITY` the bundle is signed ad hoc, enough to run locally and to keep the
  granted permission between launches.
* Distributing to other people needs a Developer ID signature and notarisation
  (`xcrun notarytool submit ... --wait`, then `xcrun stapler staple dist/Lofify.app`).
* To give the app an icon, put `packaging/macos/Lofify.icns` next to the script; it is picked up
  automatically. (The app draws its own window and tray icons at runtime and needs no assets.)

## Continuous integration

* `.github/workflows/ci.yml`: fmt, clippy and tests on Windows, macOS and Ubuntu (Ubuntu builds only
  `lofify-dsp` and `lofify-cli`), plus a job that renders the sample WAVs and uploads them.
* `.github/workflows/release.yml`: builds the Windows installer and the universal macOS bundle on a
  tag or by hand and uploads them as workflow artifacts.

## Release checklist

1. Bump `version` in `[workspace.package]`.
2. `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
3. Run the listening tests ([LISTENING_TESTS.md](LISTENING_TESTS.md)) and a `lofify passthrough` soak on
   real hardware. Nothing in this repository has been run against real audio devices yet.
4. Build the installer and the bundle, install them on clean machines, walk through the wizard.
5. Tag `vX.Y.Z`.
