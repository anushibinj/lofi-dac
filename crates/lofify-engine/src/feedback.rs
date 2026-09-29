//! Feedback protection: refuse configurations that would play Lofify's own output back into
//! its capture source.

use crate::error::{EngineError, Result};
use crate::types::{CaptureTarget, DeviceInfo};

/// Checks a resolved capture target and output device pair.
///
/// Refuses when
/// 1. the output is a virtual cable endpoint (name contains "CABLE"), because audio sent there
///    comes straight back out of the cable's recording side, or
/// 2. the output is the very same endpoint that is being captured (loopback of the output).
pub fn check_feedback(capture: &CaptureTarget, output: &DeviceInfo) -> Result<()> {
    if output.is_virtual_cable() {
        return Err(EngineError::Feedback(format!(
            "the output device \"{}\" is a virtual audio cable. Pick your real speakers or \
             headphones as the output; the cable's playback side must stay the Windows default",
            output.name
        )));
    }
    if capture.device.id == output.id && capture.device.direction == output.direction {
        return Err(EngineError::Feedback(format!(
            "the capture source and the output are the same device (\"{}\")",
            output.name
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Direction;

    fn out(id: &str, name: &str) -> DeviceInfo {
        DeviceInfo::new(id, name, Direction::Output)
    }

    fn cable_capture() -> CaptureTarget {
        CaptureTarget {
            device: DeviceInfo::new(
                "in-cable",
                "CABLE Output (VB-Audio Virtual Cable)",
                Direction::Input,
            ),
            loopback: false,
        }
    }

    #[test]
    fn real_output_is_allowed() {
        assert!(check_feedback(&cable_capture(), &out("spk", "Speakers (Realtek Audio)")).is_ok());
    }

    #[test]
    fn cable_output_is_refused() {
        let err = check_feedback(
            &cable_capture(),
            &out("c", "CABLE Input (VB-Audio Virtual Cable)"),
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::Feedback(m) if m.contains("virtual audio cable")));
        // any case
        assert!(check_feedback(&cable_capture(), &out("c", "my cable-b input")).is_err());
    }

    #[test]
    fn loopback_of_the_output_device_is_refused() {
        let spk = out("spk", "Speakers (Realtek Audio)");
        let capture = CaptureTarget {
            device: spk.clone(),
            loopback: true,
        };
        let err = check_feedback(&capture, &spk).unwrap_err();
        assert!(matches!(err, EngineError::Feedback(m) if m.contains("same device")));
    }

    #[test]
    fn loopback_of_a_different_device_is_fine() {
        let capture = CaptureTarget {
            device: out("hdmi", "HDMI Monitor"),
            loopback: true,
        };
        assert!(check_feedback(&capture, &out("spk", "Speakers")).is_ok());
    }

    #[test]
    fn same_id_in_different_directions_is_not_the_same_endpoint() {
        // A microphone and a speaker on one card may share a backend id string.
        let capture = CaptureTarget {
            device: DeviceInfo::new("card0", "Mic", Direction::Input),
            loopback: false,
        };
        assert!(check_feedback(&capture, &out("card0", "Speakers")).is_ok());
    }
}
