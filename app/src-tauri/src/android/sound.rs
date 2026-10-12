//! Playing synthesized speech through Android's audio.
//!
//! The counterpart of `audio::playback`. The same two promises hold: speech
//! goes to the headphones' own output unless the user has allowed another,
//! and no volume but the speech's own is touched — the gain is applied to
//! the samples before they are handed over.

use super::bridge;
use crate::audio::{Output, Played};
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The id of "wherever Android is sending audio at the moment".
const CURRENT: &str = "current";

/// How long the last of a sentence takes to reach Bluetooth headphones after
/// Android reports it played.
const TAIL: Duration = Duration::from_millis(250);

#[derive(Deserialize)]
struct Device {
    id: i32,
    name: String,
}

#[derive(Deserialize)]
struct Outputs {
    #[serde(default)]
    headphones: Option<Device>,
    #[serde(default)]
    speaker: Option<Device>,
}

fn output(device: Device) -> Output {
    Output {
        id: device.id.to_string(),
        name: device.name,
    }
}

fn outputs(address: Option<&str>) -> Result<Outputs, String> {
    bridge::call("outputs", serde_json::json!({ "address": address }))
}

/// The audio output that belongs to the headphones at `address`.
pub fn output_for(address: &str) -> Result<Option<Output>, String> {
    Ok(outputs(Some(address))?.headphones.map(output))
}

/// The phone's own speaker: somewhere to say the headphones have gone.
pub fn speaker() -> Result<Option<Output>, String> {
    Ok(outputs(None)?.speaker.map(output))
}

/// Whatever Android would route media to right now. Used only when the user
/// has allowed speech to go somewhere other than the headphones.
pub fn current_output() -> Output {
    Output {
        id: CURRENT.to_string(),
        name: "the phone's current audio output".to_string(),
    }
}

/// Android's media volume, as Android holds it.
///
/// This is the phone's volume for music and video, which is what the
/// headphones play at while they are the phone's media output. It is not a
/// value read from the headphones.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaVolume {
    /// The step it is on, `0..=steps`.
    pub level: u32,
    /// How many steps there are above zero. Fifteen on most phones.
    pub steps: u32,
    pub muted: bool,
    /// What media is playing through at the moment, as Android names it.
    #[serde(default)]
    pub output: Option<String>,
}

impl MediaVolume {
    pub fn percent(&self) -> u8 {
        percent_of(self.level, self.steps)
    }
}

/// A step as a percentage of the way up.
pub fn percent_of(level: u32, steps: u32) -> u8 {
    if steps == 0 {
        return 0;
    }
    let level = level.min(steps);
    ((level * 100 + steps / 2) / steps) as u8
}

/// The step nearest a percentage. Android's volume moves in steps, so most
/// percentages cannot be set exactly; this is the one that will be.
pub fn level_for(percent: u8, steps: u32) -> u32 {
    (u32::from(percent.min(100)) * steps + 50) / 100
}

pub fn media_volume() -> Result<MediaVolume, String> {
    bridge::call("volumeGet", ())
}

/// Sets the media volume to the step nearest `percent` and returns that step
/// with what Android then reports. Whether it took is for the caller to say,
/// by comparing the two.
pub fn set_media_volume(percent: u8) -> Result<(u32, MediaVolume), String> {
    let before = media_volume()?;
    let level = level_for(percent, before.steps);
    let after: MediaVolume = bridge::call("volumeSet", serde_json::json!({ "level": level }))?;
    Ok((level, after))
}

/// Mutes or unmutes media and returns what Android then reports.
pub fn set_media_mute(muted: bool) -> Result<MediaVolume, String> {
    bridge::call("muteSet", serde_json::json!({ "muted": muted }))
}

/// Plays mono samples through one output, until done or told to stop.
pub fn play(
    output_id: &str,
    samples: &[f32],
    sample_rate: u32,
    gain: f32,
    stop: &AtomicBool,
) -> Result<Played, String> {
    #[derive(Deserialize)]
    struct Poll {
        done: bool,
    }

    let mut pcm = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        let scaled = (sample * gain).clamp(-1.0, 1.0);
        pcm.extend_from_slice(&((scaled * f32::from(i16::MAX)) as i16).to_le_bytes());
    }
    let device_id = if output_id == CURRENT {
        None
    } else {
        Some(
            output_id
                .parse::<i32>()
                .map_err(|_| "not an audio output this application offered".to_string())?,
        )
    };
    bridge::run(
        "playStart",
        serde_json::json!({
            "pcm": bridge::to_hex(&pcm),
            "sampleRate": sample_rate,
            "deviceId": device_id,
        }),
    )?;

    let stopped = |completed| Played {
        completed,
        // Android offers no meter for an output. Nothing is claimed.
        endpoint_peak: 0.0,
    };
    // Android is asked whether it has finished only once it could have:
    // until then there is nothing to learn, and every question is a trip
    // through the application's main thread. Being told to stop is noticed
    // here, without asking anyone.
    let started = std::time::Instant::now();
    let length = Duration::from_secs_f64(samples.len() as f64 / f64::from(sample_rate.max(1)));
    loop {
        if stop.load(Ordering::SeqCst) {
            let _ = bridge::run("playStop", ());
            return Ok(stopped(false));
        }
        if started.elapsed() >= length && bridge::call::<Poll>("playPoll", ())?.done {
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    // Still stoppable while the tail plays out.
    let tail_ends = std::time::Instant::now() + TAIL;
    while std::time::Instant::now() < tail_ends {
        if stop.load(Ordering::SeqCst) {
            let _ = bridge::run("playStop", ());
            return Ok(stopped(false));
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    Ok(stopped(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_is_shown_as_the_nearest_percentage() {
        assert_eq!(percent_of(0, 15), 0);
        assert_eq!(percent_of(15, 15), 100);
        assert_eq!(percent_of(8, 15), 53);
        assert_eq!(percent_of(1, 15), 7);
        // Phones with finer control have more steps.
        assert_eq!(percent_of(75, 150), 50);
    }

    #[test]
    fn a_percentage_is_set_as_the_nearest_step() {
        assert_eq!(level_for(0, 15), 0);
        assert_eq!(level_for(100, 15), 15);
        assert_eq!(level_for(50, 15), 8);
        assert_eq!(level_for(3, 15), 0, "nearer silence than the first step");
        assert_eq!(level_for(4, 15), 1);
        assert_eq!(level_for(250, 15), 15, "more than everything is everything");
    }

    /// What the slider shows after a change is the step that was set, so a
    /// step's own percentage must lead back to that step.
    #[test]
    fn every_step_survives_the_trip_through_a_percentage() {
        for steps in [1, 7, 15, 25, 30, 100] {
            for level in 0..=steps {
                assert_eq!(level_for(percent_of(level, steps), steps), level, "{level}/{steps}");
            }
        }
    }

    #[test]
    fn a_phone_reporting_no_steps_is_not_divided_by() {
        assert_eq!(percent_of(3, 0), 0);
        assert_eq!(level_for(50, 0), 0);
        assert_eq!(percent_of(20, 15), 100, "a step past the top is the top");
    }

    #[test]
    fn the_volume_reads_as_kotlin_writes_it() {
        let volume: MediaVolume =
            serde_json::from_str(r#"{"level":8,"steps":15,"muted":false,"output":"Aurora"}"#).unwrap();
        assert_eq!(volume.percent(), 53);
        assert_eq!(volume.output.as_deref(), Some("Aurora"));
        let bare: MediaVolume = serde_json::from_str(r#"{"level":0,"steps":15,"muted":true}"#).unwrap();
        assert!(bare.muted && bare.output.is_none());
    }
}
