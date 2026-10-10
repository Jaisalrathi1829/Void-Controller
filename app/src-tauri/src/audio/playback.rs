//! Playing synthesized speech through a chosen Windows output endpoint.
//!
//! Plain shared-mode WASAPI: the audio goes into Windows' mixer for one
//! specific endpoint, exactly like any other application's sound. Nothing
//! here goes near the headphones' Bluetooth control channel.
//!
//! Two things this deliberately does not do:
//!
//! * **It never changes a system or endpoint volume.** Speech volume is a
//!   gain applied to the samples before they are handed over, so it is
//!   independent of — and cannot disturb — whatever the user has Windows set
//!   to.
//! * **It never picks an endpoint by itself.** The caller says which one, by
//!   id. Choosing is a policy decision and lives with the caller.

#![cfg(windows)]

use super::{Output, Played};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioClient, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;

/// `WAVE_FORMAT_IEEE_FLOAT`.
const FORMAT_IEEE_FLOAT: u16 = 3;

/// How much audio Windows is asked to buffer, in 100-nanosecond units.
const BUFFER_100NS: i64 = 2_000_000; // 200 ms

/// How often the buffer is topped up, and so how quickly a stop takes effect.
const TICK: Duration = Duration::from_millis(10);

/// `PKEY_Device_FriendlyName`.
const PKEY_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

/// `PKEY_Device_ContainerId`: the same for every node of one physical device.
const PKEY_CONTAINER_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x8c7ed206_3f8a_4827_b3ab_ae9e1faefc6c),
    pid: 2,
};

/// `PKEY_AudioEndpoint_FormFactor`.
const PKEY_FORM_FACTOR: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x1da5d803_d492_4edd_8c23_e0c0ffee7f0e),
    pid: 0,
};

/// `EndpointFormFactor::Headset`: the hands-free (call) endpoint of a
/// Bluetooth headset, as opposed to its stereo one.
const FORM_FACTOR_HEADSET: u32 = 5;

fn failed(what: &str, e: windows::core::Error) -> String {
    format!("{what}: {e}")
}

fn ensure_com() {
    unsafe {
        // S_FALSE when already initialised on this thread, which is fine.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn enumerator() -> Result<IMMDeviceEnumerator, String> {
    ensure_com();
    unsafe {
        CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(|e| failed("Windows audio is unavailable", e))
    }
}

fn id_of(device: &IMMDevice) -> Option<String> {
    unsafe {
        let pwstr = device.GetId().ok()?;
        let id = pwstr.to_string().ok();
        CoTaskMemFree(Some(pwstr.0 as *const _));
        id
    }
}

fn text_property(device: &IMMDevice, key: &PROPERTYKEY) -> Option<String> {
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ).ok()?;
        let value = store.GetValue(key).ok()?;
        Some(value.to_string()).filter(|s| !s.is_empty())
    }
}

fn number_property(device: &IMMDevice, key: &PROPERTYKEY) -> Option<u32> {
    unsafe {
        let store = device.OpenPropertyStore(STGM_READ).ok()?;
        let value = store.GetValue(key).ok()?;
        u32::try_from(&value).ok()
    }
}

/// The hex digits of a GUID in whatever punctuation it arrived in, so two
/// spellings of the same id compare equal.
pub fn normalize_guid(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_hexdigit)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

fn output_from(device: &IMMDevice) -> Option<Output> {
    Some(Output {
        id: id_of(device)?,
        name: text_property(device, &PKEY_FRIENDLY_NAME)
            .unwrap_or_else(|| "Unnamed output".to_string()),
    })
}

/// The stereo output endpoint belonging to a physical device, if it is active.
///
/// `container_id` identifies the physical device: Windows gives every node of
/// one device — its Bluetooth entries and its audio endpoints alike — the
/// same container id, so this match does not depend on what anything is
/// named. A Bluetooth headset has two outputs; the hands-free one is skipped,
/// because playing through it switches the headset into call mode and drops
/// whatever music was playing to telephone quality.
pub fn output_for_device(container_id: &str) -> Result<Option<Output>, String> {
    let wanted = normalize_guid(container_id);
    if wanted.len() != 32 {
        return Ok(None);
    }
    let enumerator = enumerator()?;
    unsafe {
        let endpoints = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(|e| failed("could not list audio outputs", e))?;
        let count = endpoints
            .GetCount()
            .map_err(|e| failed("could not list audio outputs", e))?;
        for i in 0..count {
            let Ok(device) = endpoints.Item(i) else {
                continue;
            };
            let same_device = text_property(&device, &PKEY_CONTAINER_ID)
                .is_some_and(|c| normalize_guid(&c) == wanted);
            if !same_device {
                continue;
            }
            if number_property(&device, &PKEY_FORM_FACTOR) == Some(FORM_FACTOR_HEADSET) {
                continue;
            }
            if let Some(output) = output_from(&device) {
                return Ok(Some(output));
            }
        }
    }
    Ok(None)
}

/// Windows' current default output, if there is one.
pub fn default_output() -> Result<Option<Output>, String> {
    default_output_avoiding(None)
}

/// Windows' current default output, unless it belongs to the given physical
/// device.
///
/// For saying that headphones have disconnected: for a moment afterwards
/// Windows may still name their endpoint as the default, and speaking into it
/// would be speaking to nobody.
pub fn default_output_avoiding(container_id: Option<&str>) -> Result<Option<Output>, String> {
    let avoid = container_id.map(normalize_guid).filter(|c| c.len() == 32);
    let enumerator = enumerator()?;
    unsafe {
        let Ok(device) = enumerator.GetDefaultAudioEndpoint(eRender, eConsole) else {
            return Ok(None);
        };
        if let Some(avoid) = &avoid {
            let belongs = text_property(&device, &PKEY_CONTAINER_ID)
                .is_some_and(|c| &normalize_guid(&c) == avoid);
            if belongs {
                return Ok(None);
            }
        }
        Ok(output_from(&device))
    }
}

fn device_by_id(id: &str) -> Result<IMMDevice, String> {
    let enumerator = enumerator()?;
    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        enumerator
            .GetDevice(PCWSTR(wide.as_ptr()))
            .map_err(|e| failed("the audio output is no longer available", e))
    }
}

/// Plays mono float samples through one endpoint and returns when the sound
/// has finished, been stopped, or failed.
///
/// `gain` scales the samples (1.0 = as synthesized). Setting `stop` ends
/// playback within a few milliseconds. If the endpoint disappears mid-way —
/// headphones switched off — that is an error, and nothing is re-routed
/// anywhere else.
pub fn play(
    endpoint_id: &str,
    samples: &[f32],
    sample_rate: u32,
    gain: f32,
    stop: &AtomicBool,
) -> Result<Played, String> {
    if samples.is_empty() {
        return Ok(Played {
            completed: true,
            endpoint_peak: 0.0,
        });
    }
    let gain = if gain.is_finite() { gain.clamp(0.0, 1.0) } else { 0.0 };
    let device = device_by_id(endpoint_id)?;

    unsafe {
        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| failed("could not open the audio output", e))?;

        // Mono float at the model's own rate. Windows converts to whatever
        // the endpoint actually runs at; the samples are not resampled here.
        let format = WAVEFORMATEX {
            wFormatTag: FORMAT_IEEE_FLOAT,
            nChannels: 1,
            nSamplesPerSec: sample_rate,
            nAvgBytesPerSec: sample_rate * 4,
            nBlockAlign: 4,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                BUFFER_100NS,
                0,
                &format,
                None,
            )
            .map_err(|e| failed("the audio output did not accept the speech format", e))?;

        let capacity = client
            .GetBufferSize()
            .map_err(|e| failed("could not size the audio buffer", e))?;
        let render: IAudioRenderClient = client
            .GetService()
            .map_err(|e| failed("could not open the audio stream", e))?;
        // The endpoint's own level meter. Optional: playback does not depend
        // on it.
        let meter: Option<IAudioMeterInformation> = device.Activate(CLSCTX_ALL, None).ok();

        let mut written = 0usize;
        let mut started = false;
        let mut peak = 0.0f32;
        let sample_peak = |peak: &mut f32| {
            if let Some(level) = meter.as_ref().and_then(|m| m.GetPeakValue().ok()) {
                *peak = peak.max(level);
            }
        };

        let lost = |e| failed("the audio output stopped during playback", e);

        // Feed the buffer until every sample has been handed over.
        while written < samples.len() {
            if stop.load(Ordering::SeqCst) {
                let _ = client.Stop();
                return Ok(Played {
                    completed: false,
                    endpoint_peak: peak,
                });
            }

            let queued = client.GetCurrentPadding().map_err(lost)?;
            let room = (capacity - queued) as usize;
            let chunk = room.min(samples.len() - written);
            if chunk > 0 {
                let buffer = render.GetBuffer(chunk as u32).map_err(lost)?;
                let out = std::slice::from_raw_parts_mut(buffer as *mut f32, chunk);
                for (dst, src) in out.iter_mut().zip(&samples[written..written + chunk]) {
                    *dst = (src * gain).clamp(-1.0, 1.0);
                }
                render.ReleaseBuffer(chunk as u32, 0).map_err(lost)?;
                written += chunk;
            }
            if !started {
                client.Start().map_err(lost)?;
                started = true;
            }
            sample_peak(&mut peak);
            std::thread::sleep(TICK);
        }

        // Everything is queued; wait for it to actually be played. Bounded,
        // so a stalled endpoint cannot hold the speech thread forever.
        let seconds = samples.len() as f32 / sample_rate.max(1) as f32;
        let deadline = Instant::now() + Duration::from_secs_f32(seconds + 2.0);
        loop {
            if stop.load(Ordering::SeqCst) {
                let _ = client.Stop();
                return Ok(Played {
                    completed: false,
                    endpoint_peak: peak,
                });
            }
            if client.GetCurrentPadding().map_err(lost)? == 0 || Instant::now() >= deadline {
                break;
            }
            sample_peak(&mut peak);
            std::thread::sleep(TICK);
        }
        let _ = client.Stop();

        Ok(Played {
            completed: true,
            endpoint_peak: peak,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_compare_equal_however_they_are_written() {
        let a = normalize_guid("{8c7ed206-3f8a-4827-b3ab-ae9e1faefc6c}");
        let b = normalize_guid("8C7ED206-3F8A-4827-B3AB-AE9E1FAEFC6C");
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
    }

    /// Something that is not a device id matches no endpoint, rather than
    /// matching every endpoint whose id happens to be unreadable too.
    #[test]
    fn a_malformed_container_id_matches_nothing() {
        assert_eq!(output_for_device(""), Ok(None));
        assert_eq!(output_for_device("not-a-guid"), Ok(None));
    }

    #[test]
    fn nothing_to_play_is_not_an_error() {
        let stop = AtomicBool::new(false);
        let played = play("ignored", &[], 24_000, 1.0, &stop).unwrap();
        assert!(played.completed);
    }

    #[test]
    fn an_endpoint_that_does_not_exist_is_an_error() {
        let stop = AtomicBool::new(false);
        let result = play("{0.0.0.00000000}.{no-such-endpoint}", &[0.0; 240], 24_000, 1.0, &stop);
        assert!(result.is_err(), "{result:?}");
    }
}
