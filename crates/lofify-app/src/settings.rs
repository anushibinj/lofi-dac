//! Persistent settings: a TOML file in the OS config directory.
//!
//! Devices are stored by *name* (ids change when drivers are reinstalled), the current effect
//! parameters and preset name are stored so the app comes back exactly as it was left, and a
//! missing or corrupt file never stops the app from starting.

use directories::ProjectDirs;
use lofify_dsp::params::Params;
use lofify_dsp::presets::{DEFAULT_PRESET, builtin_preset};
use lofify_engine::types::{CaptureSelection, DeviceConfig, OutputSelection};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// Where audio is captured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureKind {
    /// Detect the virtual cable's recording side.
    #[default]
    Auto,
    /// A named recording device.
    Input,
    /// Loopback of a named (or the default) playback device.
    Loopback,
}

/// Device and latency choices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub capture_kind: CaptureKind,
    /// Device name for `Input` / `Loopback` (empty loopback = default playback device).
    pub capture_device: String,
    /// Output device name; empty means the system default.
    pub output_device: String,
    /// 10 ms prefill and 3 ms tape delay instead of the defaults.
    pub low_latency: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            capture_kind: CaptureKind::Auto,
            capture_device: String::new(),
            output_device: String::new(),
            low_latency: false,
        }
    }
}

impl AudioSettings {
    /// The engine device selection these settings describe.
    pub fn device_config(&self) -> DeviceConfig {
        let name = self.capture_device.trim().to_string();
        let capture = match self.capture_kind {
            CaptureKind::Auto => CaptureSelection::Auto,
            CaptureKind::Input if name.is_empty() => CaptureSelection::Auto,
            CaptureKind::Input => CaptureSelection::Input(name),
            CaptureKind::Loopback => CaptureSelection::Loopback(name),
        };
        let out = self.output_device.trim();
        let output = if out.is_empty() {
            OutputSelection::Default
        } else {
            OutputSelection::Named(out.to_string())
        };
        DeviceConfig { capture, output }
    }
}

/// Behaviour of the app itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    /// Name of the selected preset (built-in or user).
    pub preset: String,
    /// Global bypass hotkey, e.g. "Ctrl+Alt+L".
    pub hotkey: String,
    /// Start hidden in the tray.
    pub start_minimized: bool,
    /// Register with the OS to launch at login.
    pub launch_at_login: bool,
    /// The first-run wizard has been completed (or skipped).
    pub wizard_done: bool,
    /// Start processing as soon as the app launches.
    pub auto_start: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            preset: DEFAULT_PRESET.to_string(),
            hotkey: "Ctrl+Alt+L".to_string(),
            start_minimized: false,
            launch_at_login: false,
            wizard_done: false,
            auto_start: true,
        }
    }
}

/// Everything persisted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub general: GeneralSettings,
    pub audio: AudioSettings,
    /// The live effect parameters (which may differ from the named preset after tweaking).
    pub params: Params,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            general: GeneralSettings::default(),
            audio: AudioSettings::default(),
            params: builtin_preset(DEFAULT_PRESET)
                .map(|p| p.params)
                .unwrap_or_default(),
        }
    }
}

/// Result of loading the settings file.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub settings: Settings,
    /// Set when the file existed but could not be used and defaults were substituted.
    pub warning: Option<String>,
}

impl Settings {
    /// Reads settings from `path`. A missing file gives defaults silently; a corrupt file gives
    /// defaults plus a warning (and the bad file is kept as `<name>.bad` so nothing is lost).
    pub fn load(path: &Path) -> Loaded {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Loaded {
                    settings: Settings::default(),
                    warning: None,
                };
            }
            Err(e) => {
                return Loaded {
                    settings: Settings::default(),
                    warning: Some(format!("cannot read {}: {e}", path.display())),
                };
            }
        };
        match toml::from_str::<Settings>(&text) {
            Ok(mut settings) => {
                settings.params = settings.params.sanitized();
                Loaded {
                    settings,
                    warning: None,
                }
            }
            Err(e) => {
                let bad = path.with_extension("toml.bad");
                let kept = std::fs::copy(path, &bad).is_ok();
                Loaded {
                    settings: Settings::default(),
                    warning: Some(format!(
                        "settings file {} is invalid ({e}); using defaults{}",
                        path.display(),
                        if kept {
                            format!(", old file kept as {}", bad.display())
                        } else {
                            String::new()
                        }
                    )),
                }
            }
        }
    }

    /// Writes the settings atomically (temp file, then rename).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| io::Error::other(e.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }
}

/// Where the app keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub settings_file: PathBuf,
    pub presets_dir: PathBuf,
    pub log_dir: PathBuf,
}

impl AppPaths {
    /// The OS-specific locations (`%APPDATA%`, `~/Library/Application Support`, ...).
    pub fn discover() -> Option<AppPaths> {
        let dirs = ProjectDirs::from("", "", "Lofify")?;
        let data = dirs.data_local_dir().to_path_buf();
        Some(Self::in_dirs(
            dirs.config_dir().to_path_buf(),
            data.join("logs"),
        ))
    }

    /// Everything under explicit directories (tests, portable mode).
    pub fn in_dirs(config_dir: PathBuf, log_dir: PathBuf) -> AppPaths {
        AppPaths {
            settings_file: config_dir.join("settings.toml"),
            presets_dir: config_dir.join("presets"),
            config_dir,
            log_dir,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_classic_lofi_sound_with_the_documented_hotkey() {
        let s = Settings::default();
        assert_eq!(s.general.preset, "Classic Lofi");
        assert_eq!(s.general.hotkey, "Ctrl+Alt+L");
        assert!(!s.general.wizard_done && s.general.auto_start);
        assert_eq!(s.params.filters.lpf_hz, 4_500.0);
        assert_eq!(s.audio.device_config(), DeviceConfig::default());
    }

    #[test]
    fn roundtrip_through_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg").join("settings.toml");
        let mut s = Settings::default();
        s.general.start_minimized = true;
        s.general.preset = "Cassette".into();
        s.audio.capture_kind = CaptureKind::Loopback;
        s.audio.capture_device = "CABLE Input".into();
        s.audio.output_device = "Speakers (Realtek Audio)".into();
        s.params.reverb.mix = 0.4;
        s.save(&path).unwrap();
        let loaded = Settings::load(&path);
        assert_eq!(loaded.warning, None);
        assert_eq!(loaded.settings, s);
    }

    #[test]
    fn missing_file_gives_defaults_without_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = Settings::load(&dir.path().join("nope.toml"));
        assert_eq!(loaded.settings, Settings::default());
        assert!(loaded.warning.is_none());
    }

    #[test]
    fn corrupt_file_gives_defaults_a_warning_and_keeps_the_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "this = is [not valid toml").unwrap();
        let loaded = Settings::load(&path);
        assert_eq!(loaded.settings, Settings::default());
        assert!(loaded.warning.as_deref().unwrap().contains("invalid"));
        assert!(dir.path().join("settings.toml.bad").exists());
    }

    #[test]
    fn partial_and_out_of_range_files_are_repaired() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(
            &path,
            "[general]\nhotkey = \"Ctrl+Shift+B\"\n[params.reverb]\nmix = 50.0\n",
        )
        .unwrap();
        let loaded = Settings::load(&path);
        assert_eq!(loaded.warning, None);
        assert_eq!(loaded.settings.general.hotkey, "Ctrl+Shift+B");
        assert_eq!(
            loaded.settings.general.preset, "Classic Lofi",
            "missing fields keep defaults"
        );
        assert_eq!(
            loaded.settings.params.reverb.mix, 1.0,
            "out of range values are clamped"
        );
    }

    #[test]
    fn save_is_atomic_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        Settings::default().save(&path).unwrap();
        Settings::default().save(&path).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["settings.toml"]);
    }

    #[test]
    fn device_config_mapping() {
        let mut a = AudioSettings::default();
        assert_eq!(a.device_config().capture, CaptureSelection::Auto);
        assert_eq!(a.device_config().output, OutputSelection::Default);

        a.capture_kind = CaptureKind::Input;
        assert_eq!(
            a.device_config().capture,
            CaptureSelection::Auto,
            "empty name falls back to auto"
        );
        a.capture_device = " CABLE Output ".into();
        assert_eq!(
            a.device_config().capture,
            CaptureSelection::Input("CABLE Output".into())
        );

        a.capture_kind = CaptureKind::Loopback;
        a.capture_device.clear();
        assert_eq!(
            a.device_config().capture,
            CaptureSelection::Loopback(String::new())
        );

        a.output_device = "Headphones".into();
        assert_eq!(
            a.device_config().output,
            OutputSelection::Named("Headphones".into())
        );
    }

    #[test]
    fn app_paths_are_consistent() {
        let p = AppPaths::in_dirs(PathBuf::from("/c"), PathBuf::from("/l"));
        assert_eq!(p.settings_file, PathBuf::from("/c/settings.toml"));
        assert_eq!(p.presets_dir, PathBuf::from("/c/presets"));
        assert_eq!(p.log_dir, PathBuf::from("/l"));
    }

    #[test]
    fn discovered_paths_live_under_a_lofify_directory() {
        // On CI runners without a home directory this may be None; when present it must be sane.
        if let Some(p) = AppPaths::discover() {
            assert!(
                p.config_dir
                    .to_string_lossy()
                    .to_lowercase()
                    .contains("lofify")
            );
            assert!(
                p.log_dir
                    .to_string_lossy()
                    .to_lowercase()
                    .contains("lofify")
            );
        }
    }
}
