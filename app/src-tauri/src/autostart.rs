//! "Start with Windows", through the current user's startup list.
//!
//! Windows runs every command listed under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` when the user signs
//! in. This module adds, removes and reads this application's one entry
//! there. It is per-user, needs no elevation, and touches nothing else.
//!
//! The entry is the source of truth. Nothing remembers separately whether the
//! switch is "on": it is on exactly when the entry exists and points at the
//! executable that is running. An entry left behind by a copy of the
//! application in another folder therefore reads as off, rather than as on
//! for something that would start a different program.

use std::path::Path;

/// Passed to the application when Windows starts it at sign-in.
pub const AUTOSTART_FLAG: &str = "--autostart";

#[cfg(windows)]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const VALUE_NAME: &str = "BoseQcControlCenter";

/// The command line registered for an executable.
pub fn command_for(exe: &Path) -> String {
    format!("\"{}\" {AUTOSTART_FLAG}", exe.display())
}

/// Why the switch cannot be used in this build, if it cannot.
///
/// A development build loads its interface from a dev server. Started by
/// Windows at sign-in, with no dev server running, it would open an empty
/// window — so registering it would be offering something that does not work.
pub fn unavailable_reason() -> Option<&'static str> {
    if cfg!(not(windows)) {
        Some("Only available on Windows.")
    } else if cfg!(debug_assertions) {
        Some(
            "Only available in an installed build. A development build cannot start on its \
             own, because it needs its dev server running.",
        )
    } else {
        None
    }
}

/// One entry in a startup list.
pub struct Autostart {
    #[cfg_attr(not(windows), allow(dead_code))]
    subkey: String,
    #[cfg_attr(not(windows), allow(dead_code))]
    value: String,
    command: String,
}

impl Autostart {
    /// This application's entry in the user's startup list.
    #[cfg(windows)]
    pub fn for_this_app() -> Result<Self, String> {
        let exe = std::env::current_exe()
            .map_err(|e| format!("could not find this application's own path: {e}"))?;
        Ok(Self {
            subkey: RUN_KEY.to_string(),
            value: VALUE_NAME.to_string(),
            command: command_for(&exe),
        })
    }

    #[cfg(not(windows))]
    pub fn for_this_app() -> Result<Self, String> {
        Err("Starting with the system is only implemented for Windows.".to_string())
    }

    /// The command that would be registered.
    pub fn command(&self) -> &str {
        &self.command
    }
}

#[cfg(windows)]
mod imp {
    use super::Autostart;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyW, RegDeleteValueW, RegGetValueW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Opens (creating if need be) a key under HKCU, runs `f`, closes it.
    fn with_key<R>(subkey: &str, f: impl FnOnce(HKEY) -> Result<R, String>) -> Result<R, String> {
        let name = wide(subkey);
        let mut key = HKEY::default();
        let rc = unsafe { RegCreateKeyW(HKEY_CURRENT_USER, PCWSTR(name.as_ptr()), &mut key) };
        if rc != ERROR_SUCCESS {
            return Err(format!("could not open the startup list (error {})", rc.0));
        }
        let result = f(key);
        unsafe {
            let _ = RegCloseKey(key);
        }
        result
    }

    impl Autostart {
        /// The command currently registered under this entry's name, if any.
        pub fn registered_command(&self) -> Option<String> {
            let subkey = wide(&self.subkey);
            let value = wide(&self.value);
            let mut bytes: u32 = 0;
            unsafe {
                // First call sizes the buffer.
                let rc = RegGetValueW(
                    HKEY_CURRENT_USER,
                    PCWSTR(subkey.as_ptr()),
                    PCWSTR(value.as_ptr()),
                    RRF_RT_REG_SZ,
                    None,
                    None,
                    Some(&mut bytes),
                );
                if rc != ERROR_SUCCESS || bytes == 0 {
                    return None;
                }
                let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
                let rc = RegGetValueW(
                    HKEY_CURRENT_USER,
                    PCWSTR(subkey.as_ptr()),
                    PCWSTR(value.as_ptr()),
                    RRF_RT_REG_SZ,
                    None,
                    Some(buf.as_mut_ptr().cast()),
                    Some(&mut bytes),
                );
                if rc != ERROR_SUCCESS {
                    return None;
                }
                let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                Some(String::from_utf16_lossy(&buf[..end]))
            }
        }

        /// On exactly when the entry exists and starts this executable.
        pub fn is_enabled(&self) -> bool {
            self.registered_command().as_deref() == Some(self.command.as_str())
        }

        pub fn enable(&self) -> Result<(), String> {
            let value = wide(&self.value);
            let data = wide(&self.command);
            let bytes: Vec<u8> = data.iter().flat_map(|c| c.to_le_bytes()).collect();
            with_key(&self.subkey, |key| {
                let rc =
                    unsafe { RegSetValueExW(key, PCWSTR(value.as_ptr()), 0, REG_SZ, Some(&bytes)) };
                if rc == ERROR_SUCCESS {
                    Ok(())
                } else {
                    Err(format!("could not add to the startup list (error {})", rc.0))
                }
            })
        }

        /// Removes the entry. Already absent counts as done.
        pub fn disable(&self) -> Result<(), String> {
            let value = wide(&self.value);
            with_key(&self.subkey, |key| {
                let rc = unsafe { RegDeleteValueW(key, PCWSTR(value.as_ptr())) };
                if rc == ERROR_SUCCESS || rc == ERROR_FILE_NOT_FOUND {
                    Ok(())
                } else {
                    Err(format!(
                        "could not remove from the startup list (error {})",
                        rc.0
                    ))
                }
            })
        }
    }

    #[cfg(test)]
    pub(super) fn delete_key(subkey: &str) {
        use windows::Win32::System::Registry::RegDeleteKeyW;
        let name = wide(subkey);
        unsafe {
            let _ = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(name.as_ptr()));
        }
    }
}

#[cfg(not(windows))]
impl Autostart {
    pub fn registered_command(&self) -> Option<String> {
        None
    }
    pub fn is_enabled(&self) -> bool {
        false
    }
    pub fn enable(&self) -> Result<(), String> {
        Err("Starting with the system is only implemented for Windows.".to_string())
    }
    pub fn disable(&self) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_quotes_the_path_and_marks_the_launch() {
        let cmd = command_for(Path::new(r"C:\Program Files\Bose QC\app.exe"));
        assert_eq!(cmd, r#""C:\Program Files\Bose QC\app.exe" --autostart"#);
    }

    /// Tests run in a debug build, where the switch must say why it is off.
    #[test]
    fn a_development_build_explains_itself() {
        if cfg!(debug_assertions) {
            assert!(unavailable_reason().unwrap().contains("development build"));
        }
    }

    /// The real round trip, against a scratch key of this test's own rather
    /// than the user's actual startup list — which no test should ever touch.
    #[cfg(windows)]
    #[test]
    fn an_entry_can_be_added_read_and_removed() {
        let subkey = format!(r"Software\BoseQcControlCenter-test-{}", std::process::id());
        let entry = Autostart {
            subkey: subkey.clone(),
            value: "Entry".into(),
            command: command_for(Path::new(r"C:\somewhere\app.exe")),
        };

        assert!(!entry.is_enabled(), "nothing registered yet");
        assert!(entry.registered_command().is_none());

        entry.enable().unwrap();
        assert!(entry.is_enabled());
        assert_eq!(entry.registered_command().as_deref(), Some(entry.command()));

        // An entry pointing at a different executable is not "on" for this one.
        let moved = Autostart {
            subkey: subkey.clone(),
            value: "Entry".into(),
            command: command_for(Path::new(r"C:\elsewhere\app.exe")),
        };
        assert!(!moved.is_enabled());

        entry.disable().unwrap();
        assert!(!entry.is_enabled());
        entry.disable().unwrap(); // removing what is not there is not an error

        imp::delete_key(&subkey);
    }
}
