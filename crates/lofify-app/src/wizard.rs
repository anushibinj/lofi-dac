//! First-run setup wizard logic. Pure functions over the device list so every step's
//! "can I continue?" rule is testable without a window.

use lofify_engine::types::{DeviceInfo, DeviceList};

/// The wizard's four steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Detect the virtual cable (or explain how to install it).
    Cable,
    /// Make the cable the Windows default playback device.
    DefaultPlayback,
    /// Choose the real speakers or headphones.
    Output,
    /// Start and confirm that audio is detected.
    Confirm,
}

impl Step {
    pub const ALL: [Step; 4] = [
        Step::Cable,
        Step::DefaultPlayback,
        Step::Output,
        Step::Confirm,
    ];

    /// 1-based position for "Step 2 of 4".
    pub fn number(self) -> usize {
        Self::ALL
            .iter()
            .position(|s| *s == self)
            .map_or(1, |i| i + 1)
    }

    pub fn title(self) -> &'static str {
        match self {
            Step::Cable => "Find the virtual cable",
            Step::DefaultPlayback => "Play Windows sound through the cable",
            Step::Output => "Choose your real speakers or headphones",
            Step::Confirm => "Check that it works",
        }
    }

    pub fn next(self) -> Option<Step> {
        Self::ALL.get(self.number()).copied()
    }

    pub fn previous(self) -> Option<Step> {
        self.number()
            .checked_sub(2)
            .and_then(|i| Self::ALL.get(i))
            .copied()
    }
}

/// Whether a virtual cable is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CableStatus {
    Missing,
    Found(String),
}

pub fn cable_status(list: &DeviceList) -> CableStatus {
    match list.find_virtual_cable_input() {
        Some(d) => CableStatus::Found(d.name.clone()),
        None => CableStatus::Missing,
    }
}

/// What Windows currently plays through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaybackStatus {
    /// The default playback device is the virtual cable: correct.
    CableIsDefault,
    /// Something else is the default; audio is not reaching the cable.
    OtherDefault(String),
    NoDevice,
}

pub fn playback_status(list: &DeviceList) -> PlaybackStatus {
    match list.default_output() {
        Some(d) if d.is_virtual_cable() => PlaybackStatus::CableIsDefault,
        Some(d) => PlaybackStatus::OtherDefault(d.name.clone()),
        None => PlaybackStatus::NoDevice,
    }
}

/// Playback devices that may be used as Lofify's output (everything except virtual cables).
pub fn real_outputs(list: &DeviceList) -> Vec<&DeviceInfo> {
    list.outputs
        .iter()
        .filter(|d| !d.is_virtual_cable())
        .collect()
}

/// A sensible pre-selection for the output picker: keep the current choice if it is still valid,
/// otherwise the first real device.
pub fn suggested_output(list: &DeviceList, current: &str) -> Option<String> {
    let real = real_outputs(list);
    if !current.is_empty()
        && let Some(d) = real.iter().find(|d| d.name == current)
    {
        return Some(d.name.clone());
    }
    real.first().map(|d| d.name.clone())
}

/// Whether the wizard may move on from `step`.
pub fn can_advance(
    step: Step,
    list: &DeviceList,
    chosen_output: &str,
    audio_detected: bool,
) -> bool {
    match step {
        Step::Cable => matches!(cable_status(list), CableStatus::Found(_)),
        Step::DefaultPlayback => playback_status(list) == PlaybackStatus::CableIsDefault,
        Step::Output => real_outputs(list).iter().any(|d| d.name == chosen_output),
        Step::Confirm => audio_detected,
    }
}

pub const VB_CABLE_URL: &str = "https://vb-audio.com/Cable/";

/// Instructions shown when no cable is found.
pub const INSTALL_INSTRUCTIONS: &str = "1. Download VB-CABLE from vb-audio.com/Cable (free).\n\
2. Unzip it, right-click VBCABLE_Setup_x64.exe, choose Run as administrator, click Install Driver.\n\
3. Restart Windows.\n\
4. Come back here and press Re-check.\n\n\
Lofify never installs drivers for you.";

/// Instructions for making the cable the default playback device.
pub const DEFAULT_PLAYBACK_INSTRUCTIONS: &str = "Windows 11: Settings > System > Sound > Output > choose CABLE Input.\n\
Windows 10: press Win+R, run mmsys.cpl, right-click CABLE Input on the Playback tab, Set as Default Device.\n\n\
You will hear nothing until Lofify is running. If Lofify is ever closed, switch the default \
playback device back to your speakers to hear sound again.";

#[cfg(test)]
mod tests {
    use super::*;
    use lofify_engine::sim::SimWorld;

    fn list(world: &SimWorld) -> DeviceList {
        use lofify_engine::backend::AudioBackend;
        world.backend().list_devices().unwrap()
    }

    #[test]
    fn steps_are_numbered_and_linked() {
        assert_eq!(Step::Cable.number(), 1);
        assert_eq!(Step::Confirm.number(), 4);
        assert_eq!(Step::Cable.next(), Some(Step::DefaultPlayback));
        assert_eq!(Step::Confirm.next(), None);
        assert_eq!(Step::Cable.previous(), None);
        assert_eq!(Step::Output.previous(), Some(Step::DefaultPlayback));
        for s in Step::ALL {
            assert!(!s.title().is_empty());
        }
    }

    #[test]
    fn detects_missing_and_present_cable() {
        let world = SimWorld::new();
        world.add_output("Speakers", 48_000, 2, true);
        assert_eq!(cable_status(&list(&world)), CableStatus::Missing);
        assert!(!can_advance(Step::Cable, &list(&world), "", false));

        world.add_input("CABLE Output (VB-Audio Virtual Cable)", 48_000, 2);
        assert_eq!(
            cable_status(&list(&world)),
            CableStatus::Found("CABLE Output (VB-Audio Virtual Cable)".into())
        );
        assert!(can_advance(Step::Cable, &list(&world), "", false));
    }

    #[test]
    fn default_playback_step_needs_the_cable_as_default() {
        let world = SimWorld::windows_like();
        assert_eq!(
            playback_status(&list(&world)),
            PlaybackStatus::CableIsDefault
        );
        assert!(can_advance(Step::DefaultPlayback, &list(&world), "", false));

        world.set_default_output(&world.device_id("Speakers").unwrap());
        assert_eq!(
            playback_status(&list(&world)),
            PlaybackStatus::OtherDefault("Speakers (Realtek Audio)".into())
        );
        assert!(!can_advance(
            Step::DefaultPlayback,
            &list(&world),
            "",
            false
        ));
        assert_eq!(
            playback_status(&DeviceList::default()),
            PlaybackStatus::NoDevice
        );
    }

    #[test]
    fn output_choices_exclude_the_cable() {
        let l = list(&SimWorld::windows_like());
        let names: Vec<&str> = real_outputs(&l).iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Speakers (Realtek Audio)", "Headphones (USB DAC)"]);
        assert!(!can_advance(
            Step::Output,
            &l,
            "CABLE Input (VB-Audio Virtual Cable)",
            false
        ));
        assert!(!can_advance(Step::Output, &l, "", false));
        assert!(can_advance(Step::Output, &l, "Headphones (USB DAC)", false));
    }

    #[test]
    fn suggested_output_keeps_a_valid_choice() {
        let l = list(&SimWorld::windows_like());
        assert_eq!(
            suggested_output(&l, "Headphones (USB DAC)").as_deref(),
            Some("Headphones (USB DAC)")
        );
        assert_eq!(
            suggested_output(&l, "Gone").as_deref(),
            Some("Speakers (Realtek Audio)")
        );
        assert_eq!(
            suggested_output(&l, "").as_deref(),
            Some("Speakers (Realtek Audio)")
        );
        assert_eq!(suggested_output(&DeviceList::default(), "x"), None);
    }

    #[test]
    fn confirm_step_needs_audio() {
        let l = list(&SimWorld::windows_like());
        assert!(!can_advance(Step::Confirm, &l, "", false));
        assert!(can_advance(Step::Confirm, &l, "", true));
    }
}
