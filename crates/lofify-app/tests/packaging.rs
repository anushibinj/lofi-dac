//! The packaging files cannot be run on every machine, but their invariants can be checked:
//! the macOS bundle must declare the audio-capture permission text, the installer must not bundle
//! a driver, and the versions must line up.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"))
}

#[test]
fn info_plist_declares_the_permissions_macos_requires() {
    let plist = read("packaging/macos/Info.plist.in");
    for key in [
        "NSAudioCaptureUsageDescription",
        "NSMicrophoneUsageDescription",
        "CFBundleIdentifier",
        "CFBundleExecutable",
        "CFBundleShortVersionString",
        "LSMinimumSystemVersion",
    ] {
        assert!(
            plist.contains(&format!("<key>{key}</key>")),
            "Info.plist is missing {key}"
        );
    }
    assert!(
        plist.contains("<string>14.2</string>"),
        "process taps need macOS 14.2"
    );
    assert!(
        plist.contains("@VERSION@"),
        "the version placeholder must be substituted by the script"
    );
    for tag in ["dict", "plist"] {
        assert_eq!(
            plist.matches(&format!("<{tag}")).count(),
            plist.matches(&format!("</{tag}>")).count(),
            "unbalanced <{tag}>"
        );
    }
    // every <key> is followed by a value element
    let keys = plist.matches("<key>").count();
    let values = ["<string>", "<true/>", "<false/>"]
        .iter()
        .map(|v| plist.matches(v).count())
        .sum::<usize>();
    assert_eq!(keys, values, "each key needs exactly one value");
}

#[test]
fn the_mac_script_substitutes_the_version_and_signs_the_bundle() {
    let script = read("packaging/macos/make-app.sh");
    assert!(script.starts_with("#!/usr/bin/env bash"));
    assert!(script.contains("set -euo pipefail"));
    assert!(script.contains("sed \"s/@VERSION@/${VERSION}/g\""));
    assert!(script.contains("Info.plist"));
    assert!(script.contains("codesign --force --deep --sign"));
    assert!(script.contains("UNIVERSAL"));
    assert!(
        script.contains("UNTESTED"),
        "unverified scripts must say so"
    );
}

#[cfg(unix)]
#[test]
fn the_mac_script_is_valid_shell() {
    let status = std::process::Command::new("bash")
        .arg("-n")
        .arg(root().join("packaging/macos/make-app.sh"))
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn the_installer_bundles_the_apps_but_no_audio_driver() {
    let iss = read("packaging/windows/lofify.iss");
    assert!(iss.contains("lofify-app.exe") && iss.contains("lofify.exe"));
    assert!(iss.contains("LICENSE-MIT") && iss.contains("LICENSE-APACHE"));
    let lower = iss.to_lowercase();
    for banned in [
        ".sys",
        ".inf",
        ".cat",
        "vbcable",
        "vb-cable_setup",
        "driver",
    ] {
        let hits: Vec<&str> = lower
            .lines()
            .filter(|l| !l.trim_start().starts_with(';') && l.contains(banned))
            .filter(|l| l.contains("source:"))
            .collect();
        assert!(
            hits.is_empty(),
            "the installer must not bundle {banned}: {hits:?}"
        );
    }
    assert!(iss.contains("does NOT bundle"), "the script must say why");
    // The startup registry value the app writes must be cleaned up on uninstall.
    assert!(iss.contains("CurrentVersion\\Run") && iss.contains("'Lofify'"));
}

#[test]
fn the_installer_readme_tells_users_to_install_the_cable_themselves() {
    let text = read("packaging/windows/installer-readme.txt");
    assert!(text.contains("vb-audio.com/Cable"));
    assert!(text.contains("CABLE Input"));
    assert!(text.to_lowercase().contains("hear nothing"));
}

#[test]
fn release_profile_is_optimised_and_stripped() {
    let manifest = read("Cargo.toml");
    let release = manifest
        .split("[profile.release]")
        .nth(1)
        .unwrap()
        .split("\n[")
        .next()
        .unwrap();
    for want in [
        "lto = \"fat\"",
        "strip = true",
        "codegen-units = 1",
        "opt-level = 3",
    ] {
        assert!(release.contains(want), "release profile lacks {want}");
    }
    assert!(
        release.contains("panic = \"unwind\""),
        "callbacks rely on catch_unwind, so panic must stay `unwind`"
    );
}

#[test]
fn windows_builds_link_the_c_runtime_statically() {
    let cfg = read(".cargo/config.toml");
    assert!(cfg.contains("x86_64-pc-windows-msvc") && cfg.contains("+crt-static"));
}

#[test]
fn the_license_files_are_present() {
    for f in ["LICENSE-MIT", "LICENSE-APACHE"] {
        assert!(!read(f).trim().is_empty());
    }
    assert!(read("Cargo.toml").contains("license = \"MIT OR Apache-2.0\""));
}
