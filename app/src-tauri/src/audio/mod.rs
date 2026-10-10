//! Windows audio integration, via Core Audio.
//!
//! This is a genuinely separate subsystem from Bose device control, and the UI
//! keeps them visually distinct. Windows system volume for an endpoint is not
//! the same thing as whatever volume the headphones keep internally, and
//! conflating the two would be exactly the kind of comfortable lie this project
//! avoids.
//!
//! Everything here works against any audio endpoint, Bose or otherwise, through
//! documented Win32 APIs. Nothing here is vendor-specific and nothing here
//! required reverse engineering.

#[cfg(windows)]
pub mod playback;
#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use win::*;

use serde::{Deserialize, Serialize};

/// An active output endpoint that speech can be played through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub id: String,
    pub name: String,
}

/// How a playback ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Played {
    /// False if it was stopped before the end.
    pub completed: bool,
    /// The highest level Windows' own meter for the endpoint showed while
    /// this was playing, 0.0-1.0. Evidence that audio reached that endpoint's
    /// stream — not that anyone heard it.
    pub endpoint_peak: f32,
}

/// An audio output endpoint as Windows sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioEndpoint {
    pub id: String,
    pub name: String,
    /// 0-100.
    pub volume_percent: u8,
    pub muted: bool,
    pub is_default_render: bool,
    pub is_default_communications: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate_hz: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u16>,
    /// True when the endpoint name suggests it is a Bluetooth device. A hint
    /// for matching an endpoint to the connected headphones, never proof.
    pub likely_bluetooth: bool,
}

/// Converts a Core Audio scalar (0.0-1.0) to a percentage.
///
/// Rounds to nearest rather than truncating, so a scalar of 0.499 does not
/// display as 49% when the user set 50%.
pub fn scalar_to_percent(scalar: f32) -> u8 {
    // NaN has no meaningful position on the scale, so it floors. Infinity does
    // have one — it is above the range — so it clamps to 100 like any other
    // over-range value. Treating both as "not finite" would silently turn a
    // maxed-out reading into a muted one.
    if scalar.is_nan() || scalar <= 0.0 {
        return 0;
    }
    if scalar >= 1.0 {
        return 100;
    }
    (scalar * 100.0).round() as u8
}

pub fn percent_to_scalar(percent: u8) -> f32 {
    (percent.min(100) as f32) / 100.0
}

/// Whether an endpoint name suggests a Bluetooth device.
pub fn looks_like_bluetooth_endpoint(name: &str) -> bool {
    let lowered = name.to_lowercase();
    ["bluetooth", "hands-free", "handsfree", "stereo)", "a2dp"]
        .iter()
        .any(|hint| lowered.contains(hint))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_conversion_rounds_to_nearest() {
        assert_eq!(scalar_to_percent(0.0), 0);
        assert_eq!(scalar_to_percent(1.0), 100);
        assert_eq!(scalar_to_percent(0.5), 50);
        // Truncation would give 49 here, which reads as a bug to the user.
        assert_eq!(scalar_to_percent(0.499), 50);
        assert_eq!(scalar_to_percent(0.494), 49);
    }

    #[test]
    fn scalar_conversion_clamps_out_of_range_input() {
        assert_eq!(scalar_to_percent(-0.5), 0);
        assert_eq!(scalar_to_percent(2.0), 100);
        assert_eq!(scalar_to_percent(f32::NAN), 0);
        assert_eq!(scalar_to_percent(f32::INFINITY), 100);
    }

    #[test]
    fn percent_round_trips() {
        for p in [0u8, 1, 25, 50, 75, 99, 100] {
            assert_eq!(scalar_to_percent(percent_to_scalar(p)), p);
        }
    }

    #[test]
    fn percent_above_100_is_clamped_not_wrapped() {
        assert_eq!(percent_to_scalar(200), 1.0);
    }

    #[test]
    fn bluetooth_endpoint_hints() {
        assert!(looks_like_bluetooth_endpoint(
            "Headphones (Bose QC Stereo)"
        ));
        assert!(looks_like_bluetooth_endpoint("Headset (Bose Hands-Free AG Audio)"));
        assert!(!looks_like_bluetooth_endpoint("Speakers (Realtek(R) Audio)"));
    }
}
