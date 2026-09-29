//! Device and stream descriptions shared by the engine, backends and UIs.

use crate::error::{EngineError, Result};

/// Whether an endpoint captures or plays audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// A recording endpoint (microphone, "CABLE Output").
    Input,
    /// A playback endpoint (speakers, "CABLE Input").
    Output,
}

/// One audio endpoint as reported by a backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable identifier, unique per endpoint within a backend.
    pub id: String,
    /// Human readable name, e.g. "CABLE Output (VB-Audio Virtual Cable)".
    pub name: String,
    pub direction: Direction,
    /// True for the system default of its direction.
    pub is_default: bool,
    /// Native / mix-format sample rate when known.
    pub sample_rate: Option<u32>,
    /// Native / mix-format channel count when known.
    pub channels: Option<u16>,
}

impl DeviceInfo {
    /// Creates a description with no default flag and no format information.
    pub fn new(id: impl Into<String>, name: impl Into<String>, direction: Direction) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            direction,
            is_default: false,
            sample_rate: None,
            channels: None,
        }
    }

    /// True if this endpoint belongs to a virtual audio cable (name contains "CABLE").
    pub fn is_virtual_cable(&self) -> bool {
        is_virtual_cable_name(&self.name)
    }

    /// True for the macOS system audio tap pseudo device.
    pub fn is_system_audio(&self) -> bool {
        self.id == SYSTEM_AUDIO_ID
    }
}

/// Id of the pseudo capture device that stands for "all system audio" captured through a Core
/// Audio process tap (macOS 14.2+). It needs no virtual cable and no driver.
pub const SYSTEM_AUDIO_ID: &str = "coreaudio:system-audio-tap";

/// Display name of the system audio tap, also what `--capture system` selects.
pub const SYSTEM_AUDIO_NAME: &str = "System Audio (Core Audio tap)";

/// Describes the system audio tap. `sample_rate` is the rate the tap will deliver, which is the
/// rate of the default output device it is clocked from.
pub fn system_audio_device(sample_rate: Option<u32>) -> DeviceInfo {
    DeviceInfo {
        sample_rate,
        channels: Some(2),
        ..DeviceInfo::new(SYSTEM_AUDIO_ID, SYSTEM_AUDIO_NAME, Direction::Input)
    }
}

/// The feedback-protection test from the brief: a name containing "CABLE" (any case).
pub fn is_virtual_cable_name(name: &str) -> bool {
    name.to_ascii_uppercase().contains("CABLE")
}

/// How well a name matches a known virtual audio device family. Lower is better.
/// Used to auto-detect the capture side of a virtual cable.
pub fn virtual_rank(name: &str) -> Option<u8> {
    let upper = name.to_ascii_uppercase();
    if upper.contains("CABLE") {
        Some(0)
    } else if upper.contains("VOICEMEETER") {
        Some(1)
    } else if upper.contains("VIRTUAL") {
        Some(2)
    } else {
        None
    }
}

/// All endpoints a backend can see.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceList {
    pub inputs: Vec<DeviceInfo>,
    pub outputs: Vec<DeviceInfo>,
}

fn find_by_name<'a>(devices: &'a [DeviceInfo], name: &str) -> Option<&'a DeviceInfo> {
    let wanted = name.trim();
    devices
        .iter()
        .find(|d| d.name == wanted)
        .or_else(|| devices.iter().find(|d| d.name.eq_ignore_ascii_case(wanted)))
        .or_else(|| {
            let lower = wanted.to_ascii_lowercase();
            devices
                .iter()
                .find(|d| d.name.to_ascii_lowercase().contains(&lower))
        })
}

impl DeviceList {
    /// The system default playback device.
    pub fn default_output(&self) -> Option<&DeviceInfo> {
        self.outputs.iter().find(|d| d.is_default)
    }

    /// The system default recording device.
    pub fn default_input(&self) -> Option<&DeviceInfo> {
        self.inputs.iter().find(|d| d.is_default)
    }

    /// Finds a recording device by name (exact, then case-insensitive, then substring).
    pub fn find_input(&self, name: &str) -> Option<&DeviceInfo> {
        find_by_name(&self.inputs, name)
    }

    /// Finds a playback device by name (exact, then case-insensitive, then substring).
    pub fn find_output(&self, name: &str) -> Option<&DeviceInfo> {
        find_by_name(&self.outputs, name)
    }

    /// The macOS system audio tap, when this machine supports it.
    pub fn find_system_audio(&self) -> Option<&DeviceInfo> {
        self.inputs.iter().find(|d| d.is_system_audio())
    }

    /// Finds the recording side of a virtual cable: CABLE, then VoiceMeeter, then "Virtual".
    pub fn find_virtual_cable_input(&self) -> Option<&DeviceInfo> {
        self.inputs
            .iter()
            .filter_map(|d| virtual_rank(&d.name).map(|rank| (rank, d)))
            .min_by_key(|(rank, _)| *rank)
            .map(|(_, d)| d)
    }

    /// True if an endpoint with this id exists in the given direction.
    pub fn contains(&self, id: &str, direction: Direction) -> bool {
        let list = match direction {
            Direction::Input => &self.inputs,
            Direction::Output => &self.outputs,
        };
        list.iter().any(|d| d.id == id)
    }
}

/// Where audio is captured from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CaptureSelection {
    /// The best source for this machine: the system audio tap on macOS 14.2+, otherwise a
    /// virtual cable's recording endpoint (CABLE, VoiceMeeter, "Virtual").
    #[default]
    Auto,
    /// A recording device by name, e.g. "CABLE Output (VB-Audio Virtual Cable)".
    Input(String),
    /// Loopback capture of a playback device by name (Windows). An empty name means the default
    /// playback device.
    Loopback(String),
}

/// Where processed audio is played.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OutputSelection {
    /// The system default playback device.
    #[default]
    Default,
    /// A playback device by name.
    Named(String),
}

/// A pair of selections, the unit the UI stores and the engine applies.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceConfig {
    pub capture: CaptureSelection,
    pub output: OutputSelection,
}

/// A resolved capture endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureTarget {
    pub device: DeviceInfo,
    /// True when a playback endpoint is recorded through loopback.
    pub loopback: bool,
}

impl CaptureSelection {
    /// Resolves this selection against the currently visible devices.
    pub fn resolve(&self, devices: &DeviceList) -> Result<CaptureTarget> {
        match self {
            CaptureSelection::Auto => devices
                .find_system_audio()
                .or_else(|| devices.find_virtual_cable_input())
                .map(|d| CaptureTarget {
                    device: d.clone(),
                    loopback: false,
                })
                .ok_or(EngineError::NoVirtualCable),
            CaptureSelection::Input(name) => devices
                .find_input(name)
                .map(|d| CaptureTarget {
                    device: d.clone(),
                    loopback: false,
                })
                .ok_or_else(|| EngineError::DeviceNotFound(name.clone())),
            CaptureSelection::Loopback(name) => {
                let device = if name.trim().is_empty() {
                    devices.default_output()
                } else {
                    devices.find_output(name)
                };
                device
                    .map(|d| CaptureTarget {
                        device: d.clone(),
                        loopback: true,
                    })
                    .ok_or_else(|| {
                        if name.trim().is_empty() {
                            EngineError::NoDefaultOutput
                        } else {
                            EngineError::DeviceNotFound(name.clone())
                        }
                    })
            }
        }
    }
}

impl OutputSelection {
    /// Resolves this selection against the currently visible devices.
    pub fn resolve(&self, devices: &DeviceList) -> Result<DeviceInfo> {
        match self {
            OutputSelection::Default => devices
                .default_output()
                .cloned()
                .ok_or(EngineError::NoDefaultOutput),
            OutputSelection::Named(name) => devices
                .find_output(name)
                .cloned()
                .ok_or_else(|| EngineError::DeviceNotFound(name.clone())),
        }
    }
}

/// Format an output stream ended up with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

/// Requested device buffer size for the output stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BufferRequest {
    /// Let the backend choose.
    #[default]
    Default,
    /// A specific number of frames per callback.
    Frames(u32),
}

/// What an opened stream looks like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    pub device_name: String,
    pub device_id: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// Device buffer size in frames when the backend knows it.
    pub buffer_frames: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, name: &str, dir: Direction, default: bool) -> DeviceInfo {
        DeviceInfo {
            is_default: default,
            ..DeviceInfo::new(id, name, dir)
        }
    }

    fn windows_like() -> DeviceList {
        DeviceList {
            inputs: vec![
                dev(
                    "in-mic",
                    "Microphone (Realtek Audio)",
                    Direction::Input,
                    true,
                ),
                dev(
                    "in-cable",
                    "CABLE Output (VB-Audio Virtual Cable)",
                    Direction::Input,
                    false,
                ),
            ],
            outputs: vec![
                dev(
                    "out-cable",
                    "CABLE Input (VB-Audio Virtual Cable)",
                    Direction::Output,
                    true,
                ),
                dev(
                    "out-spk",
                    "Speakers (Realtek Audio)",
                    Direction::Output,
                    false,
                ),
                dev("out-hp", "Headphones (USB DAC)", Direction::Output, false),
            ],
        }
    }

    #[test]
    fn cable_detection_by_name() {
        assert!(is_virtual_cable_name(
            "CABLE Input (VB-Audio Virtual Cable)"
        ));
        assert!(is_virtual_cable_name("cable-a output"));
        assert!(!is_virtual_cable_name("Speakers (Realtek Audio)"));
        assert!(!is_virtual_cable_name("Headphones"));
    }

    #[test]
    fn auto_detect_prefers_cable_then_voicemeeter_then_virtual() {
        let mut list = DeviceList::default();
        list.inputs
            .push(dev("a", "Some Virtual Thing", Direction::Input, false));
        assert_eq!(
            list.find_virtual_cable_input().map(|d| d.id.as_str()),
            Some("a")
        );
        list.inputs
            .push(dev("b", "VoiceMeeter Output", Direction::Input, false));
        assert_eq!(
            list.find_virtual_cable_input().map(|d| d.id.as_str()),
            Some("b")
        );
        list.inputs.push(dev(
            "c",
            "CABLE Output (VB-Audio Virtual Cable)",
            Direction::Input,
            false,
        ));
        assert_eq!(
            list.find_virtual_cable_input().map(|d| d.id.as_str()),
            Some("c")
        );
        assert!(DeviceList::default().find_virtual_cable_input().is_none());
    }

    #[test]
    fn name_lookup_is_forgiving() {
        let list = windows_like();
        assert_eq!(
            list.find_output("Speakers (Realtek Audio)")
                .map(|d| d.id.as_str()),
            Some("out-spk")
        );
        assert_eq!(
            list.find_output("speakers (realtek audio)")
                .map(|d| d.id.as_str()),
            Some("out-spk")
        );
        assert_eq!(
            list.find_output("headphones").map(|d| d.id.as_str()),
            Some("out-hp")
        );
        assert!(list.find_output("nothing like this").is_none());
    }

    #[test]
    fn capture_selection_resolves() {
        let list = windows_like();
        let t = CaptureSelection::Auto.resolve(&list).unwrap();
        assert_eq!((t.device.id.as_str(), t.loopback), ("in-cable", false));

        let t = CaptureSelection::Input("microphone".into())
            .resolve(&list)
            .unwrap();
        assert_eq!(t.device.id, "in-mic");

        let t = CaptureSelection::Loopback(String::new())
            .resolve(&list)
            .unwrap();
        assert_eq!((t.device.id.as_str(), t.loopback), ("out-cable", true));

        let t = CaptureSelection::Loopback("Speakers".into())
            .resolve(&list)
            .unwrap();
        assert_eq!((t.device.id.as_str(), t.loopback), ("out-spk", true));
    }

    #[test]
    fn capture_selection_errors() {
        let empty = DeviceList::default();
        assert_eq!(
            CaptureSelection::Auto.resolve(&empty).unwrap_err(),
            EngineError::NoVirtualCable
        );
        assert_eq!(
            CaptureSelection::Input("x".into())
                .resolve(&empty)
                .unwrap_err(),
            EngineError::DeviceNotFound("x".into())
        );
        assert_eq!(
            CaptureSelection::Loopback(String::new())
                .resolve(&empty)
                .unwrap_err(),
            EngineError::NoDefaultOutput
        );
    }

    #[test]
    fn output_selection_resolves() {
        let list = windows_like();
        assert_eq!(
            OutputSelection::Default.resolve(&list).unwrap().id,
            "out-cable"
        );
        assert_eq!(
            OutputSelection::Named("Headphones".into())
                .resolve(&list)
                .unwrap()
                .id,
            "out-hp"
        );
        assert_eq!(
            OutputSelection::Named("Nope".into())
                .resolve(&list)
                .unwrap_err(),
            EngineError::DeviceNotFound("Nope".into())
        );
        assert_eq!(
            OutputSelection::Default
                .resolve(&DeviceList::default())
                .unwrap_err(),
            EngineError::NoDefaultOutput
        );
    }

    #[test]
    fn auto_prefers_the_system_tap_over_a_cable() {
        let mut list = windows_like();
        assert!(
            !CaptureSelection::Auto
                .resolve(&list)
                .unwrap()
                .device
                .is_system_audio()
        );
        list.inputs.push(system_audio_device(Some(48_000)));
        let t = CaptureSelection::Auto.resolve(&list).unwrap();
        assert!(t.device.is_system_audio() && !t.loopback);
        assert_eq!(t.device.sample_rate, Some(48_000));
        // and it can be chosen by name (`--capture system`)
        let named = CaptureSelection::Input(SYSTEM_AUDIO_NAME.into())
            .resolve(&list)
            .unwrap();
        assert_eq!(named.device.id, SYSTEM_AUDIO_ID);
    }

    #[test]
    fn contains_checks_direction() {
        let list = windows_like();
        assert!(list.contains("out-spk", Direction::Output));
        assert!(!list.contains("out-spk", Direction::Input));
    }
}
