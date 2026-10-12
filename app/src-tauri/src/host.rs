//! What the phone or computer this application runs on is called.
//!
//! The headphones are connected *to* something, and when that something is
//! this machine, its name is what there is to say: "Aurora connected to
//! Nemesis". It is the name the system gives itself — on Android the
//! Bluetooth name, which is what the headphones themselves would call it.
//!
//! Only this machine is named. Whatever else the headphones may be connected
//! to at the same time is not something this application has asked them.

use parking_lot::Mutex;
use std::time::{Duration, Instant};

/// How long an answer is kept. The name hardly ever changes, but on Android
/// the better answer only becomes available once Bluetooth access has been
/// granted, so it is asked for again now and then.
const KEEP: Duration = Duration::from_secs(300);

/// How soon it is asked for again after the system gave no name. That can be
/// a matter of timing — on Android, asking before the Kotlin side is up — and
/// should not stand for five minutes.
const RETRY: Duration = Duration::from_secs(15);

static KNOWN: Mutex<Option<(Instant, Option<String>)>> = Mutex::new(None);

/// This machine's name, or `None` if the system will not say.
///
/// On Android this asks the Kotlin side and so must not be called from the
/// thread that runs the event loop.
pub fn name() -> Option<String> {
    let mut known = KNOWN.lock();
    if let Some((when, name)) = known.as_ref() {
        let good_for = if name.is_some() { KEEP } else { RETRY };
        if when.elapsed() < good_for {
            return name.clone();
        }
    }
    let name = lookup().map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
    *known = Some((Instant::now(), name.clone()));
    name
}

#[cfg(windows)]
fn lookup() -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::System::SystemInformation::{
        ComputerNamePhysicalDnsHostname, GetComputerNameExW,
    };

    // The name as its owner typed it, not the upper-cased NetBIOS form.
    let mut len = 0u32;
    // The first call says how much room the name needs, and fails doing it.
    let _ = unsafe { GetComputerNameExW(ComputerNamePhysicalDnsHostname, PWSTR::null(), &mut len) };
    if len == 0 || len > 1024 {
        return std::env::var("COMPUTERNAME").ok();
    }
    let mut buffer = vec![0u16; len as usize];
    let read = unsafe {
        GetComputerNameExW(
            ComputerNamePhysicalDnsHostname,
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
    };
    match read {
        Ok(()) => Some(String::from_utf16_lossy(&buffer[..len as usize])),
        Err(_) => std::env::var("COMPUTERNAME").ok(),
    }
}

#[cfg(target_os = "android")]
fn lookup() -> Option<String> {
    crate::android::bridge::host_name().ok().flatten()
}

#[cfg(not(any(windows, target_os = "android")))]
fn lookup() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real question to the real system. Whatever it answers, it is the
    /// same answer the second time, and never an empty one.
    #[test]
    fn the_name_is_stable_and_never_empty() {
        let first = name();
        assert_eq!(name(), first);
        if let Some(name) = &first {
            assert!(!name.trim().is_empty());
        }
        #[cfg(windows)]
        assert!(first.is_some(), "Windows always has a computer name");
    }
}
