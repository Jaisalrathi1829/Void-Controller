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
    loop {
        if stop.load(Ordering::SeqCst) {
            let _ = bridge::run("playStop", ());
            return Ok(stopped(false));
        }
        if bridge::call::<Poll>("playPoll", ())?.done {
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
