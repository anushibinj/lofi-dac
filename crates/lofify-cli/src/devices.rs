//! `lofify devices`: list audio endpoints and detect a virtual cable.

use anyhow::Result;
use clap::Args;
use lofify_engine::sim::SimWorld;
use lofify_engine::types::{DeviceInfo, DeviceList};

#[derive(Args, Debug)]
pub struct DevicesArgs {
    /// List a simulated Windows machine instead of real hardware (works on any OS).
    #[arg(long)]
    pub simulate: bool,
}

fn line(d: &DeviceInfo) -> String {
    let mut extras = Vec::new();
    if d.is_default {
        extras.push("default".to_string());
    }
    if d.is_virtual_cable() {
        extras.push("VIRTUAL CABLE".to_string());
    }
    if let (Some(rate), Some(ch)) = (d.sample_rate, d.channels) {
        extras.push(format!("{rate} Hz, {ch} ch"));
    }
    let extras = if extras.is_empty() {
        String::new()
    } else {
        format!("  [{}]", extras.join(", "))
    };
    format!("  {}{extras}", d.name)
}

/// Renders the device list as text. Pure so it can be tested.
pub fn render(list: &DeviceList) -> String {
    let mut out = String::new();
    out.push_str("Capture sources (recording devices):\n");
    if list.inputs.is_empty() {
        out.push_str("  (none)\n");
    }
    for d in &list.inputs {
        out.push_str(&line(d));
        out.push('\n');
    }
    out.push_str("\nOutput devices (playback devices):\n");
    if list.outputs.is_empty() {
        out.push_str("  (none)\n");
    }
    for d in &list.outputs {
        out.push_str(&line(d));
        out.push('\n');
    }
    out.push('\n');
    match list.find_virtual_cable_input() {
        Some(cable) => {
            out.push_str(&format!("Virtual cable detected: \"{}\"\n", cable.name));
            match list.default_output() {
                Some(d) if d.is_virtual_cable() => {
                    out.push_str("Windows is playing through the cable. Pick your real speakers as Lofify's output.\n");
                }
                _ => out.push_str(
                    "Set the Windows default playback device to the cable's \"CABLE Input\" so Lofify can hear your audio.\n",
                ),
            }
        }
        None => out.push_str(
            "No virtual cable found. Install VB-Cable (https://vb-audio.com/Cable/) and see docs/SETUP_WINDOWS.md.\n",
        ),
    }
    out
}

pub fn run(args: &DevicesArgs) -> Result<()> {
    let list = if args.simulate {
        lofify_engine::sim::SimWorld::windows_like().backend_list()?
    } else {
        lofify_engine::platform::list_devices()?
    };
    print!("{}", render(&list));
    Ok(())
}

trait BackendList {
    fn backend_list(&self) -> Result<DeviceList>;
}

impl BackendList for SimWorld {
    fn backend_list(&self) -> Result<DeviceList> {
        use lofify_engine::backend::AudioBackend;
        Ok(self.backend().list_devices()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_devices_and_detects_the_cable() {
        let list = SimWorld::windows_like().backend_list().unwrap();
        let text = render(&list);
        assert!(text.contains("Speakers (Realtek Audio)"));
        assert!(text.contains("CABLE Output (VB-Audio Virtual Cable)"));
        assert!(text.contains("VIRTUAL CABLE"));
        assert!(text.contains("Virtual cable detected"));
        assert!(text.contains("Windows is playing through the cable"));
        assert!(text.contains("48000 Hz, 2 ch"));
    }

    #[test]
    fn missing_cable_shows_install_instructions() {
        let world = SimWorld::new();
        world.add_output("Speakers", 48_000, 2, true);
        let text = render(&world.backend_list().unwrap());
        assert!(text.contains("No virtual cable found") && text.contains("VB-Cable"));
        assert!(text.contains("(none)"), "no capture devices");
    }

    #[test]
    fn cable_present_but_windows_not_using_it_gives_the_setup_hint() {
        let world = SimWorld::new();
        world.add_input("CABLE Output (VB-Audio Virtual Cable)", 48_000, 2);
        world.add_output("Speakers", 48_000, 2, true);
        let text = render(&world.backend_list().unwrap());
        assert!(text.contains("Set the Windows default playback device"));
    }
}
