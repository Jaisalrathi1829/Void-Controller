//! "Find my buds": when and where the earbuds were last connected.
//!
//! The earbuds cannot say where they are; the phone can say where it is. So
//! while the earbuds are connected this notes the time, and — on a phone,
//! with the location permission — where the phone was, as Android last knew
//! it. When they disconnect it notes that moment the same way. The place is
//! then the last place they were connected to the phone.
//!
//! Only with the switch on (it starts off), only for earbuds, and only kept in
//! the settings file on this device. Nothing is sent anywhere: the location
//! is the one the phone already has, and showing it on a map is handed to the
//! phone's own maps application. On a computer only the time is noted.

use crate::bose::bmap::models::Category;
use crate::bose::real::Sighting;
use crate::settings::{LastSeen, Place};
use crate::watcher::Step;
use crate::AppState;
use parking_lot::Mutex;
use std::time::{Duration, Instant};

/// How often the note is brought up to date while the earbuds stay connected.
const WHILE_CONNECTED: Duration = Duration::from_secs(120);

static LAST_NOTED: Mutex<Option<Instant>> = Mutex::new(None);

/// Brings the note up to date from one look at the earbuds.
pub fn observe(state: &AppState, seen: Option<&Sighting>, step: &Step) {
    let settings = state.settings().get();
    if !settings.remember_last_seen {
        return;
    }
    if state.current().reported_category() != Some(Category::Earbuds) {
        return;
    }
    let Some(seen) = seen else { return };

    let due = {
        let mut last = LAST_NOTED.lock();
        let due = step.lost
            || (seen.connected && last.map_or(true, |at| at.elapsed() >= WHILE_CONNECTED));
        if due {
            *last = Some(Instant::now());
        }
        due
    };
    if !due {
        return;
    }

    let previous = settings.last_seen.and_then(|l| l.place);
    let place = locate().or(previous);
    let note = LastSeen {
        device_name: seen.name.clone(),
        at: crate::device::now_rfc3339(),
        place,
    };
    let _ = state.settings().update(|s| s.last_seen = Some(note));
}

/// Where the phone is, as Android last knew it. `None` on a computer, without
/// the permission, or with no location to give.
fn locate() -> Option<Place> {
    #[cfg(target_os = "android")]
    {
        let fix = crate::android::bridge::last_location().ok()?;
        if !fix.permission {
            return None;
        }
        let (latitude, longitude) = (fix.latitude?, fix.longitude?);
        if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
            return None;
        }
        let at = fix
            .time
            .and_then(|ms| {
                time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()
            })
            .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok())
            .unwrap_or_else(crate::device::now_rfc3339);
        Some(Place {
            latitude_e6: (latitude * 1e6).round() as i32,
            longitude_e6: (longitude * 1e6).round() as i32,
            accuracy_m: fix.accuracy.map(|a| a.round().clamp(0.0, 1e6) as u32),
            at,
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        None
    }
}
