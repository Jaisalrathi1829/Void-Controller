//! Calls into the Kotlin half of the application.
//!
//! One Tauri plugin, registered by this crate and implemented by
//! `AuroraPlugin.kt`. It is invoked from here and from nowhere else: the web
//! view is granted no permission to call it, so everything it can do is
//! reachable only through this crate's own typed commands.
//!
//! Every call blocks until Kotlin answers. None may be made from the thread
//! that runs the event loop.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[cfg(target_os = "android")]
mod host {
    use serde::de::DeserializeOwned;
    use serde::Serialize;
    use std::sync::OnceLock;
    use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
    use tauri::Wry;

    const PLUGIN: &str = "aurora";
    const PACKAGE: &str = "local.bosecontrolcenter.app";
    const CLASS: &str = "AuroraPlugin";

    static HANDLE: OnceLock<PluginHandle<Wry>> = OnceLock::new();

    /// The plugin to register with the application.
    pub fn init() -> TauriPlugin<Wry> {
        Builder::new(PLUGIN)
            .setup(|_app, api| {
                let handle = api.register_android_plugin(PACKAGE, CLASS)?;
                let _ = HANDLE.set(handle);
                Ok(())
            })
            .build()
    }

    pub fn call<R: DeserializeOwned>(command: &str, payload: impl Serialize) -> Result<R, String> {
        let handle = HANDLE
            .get()
            .ok_or_else(|| "the Android side of the application has not started".to_string())?;
        handle
            .run_mobile_plugin::<R>(command, payload)
            .map_err(|e| e.to_string())
    }
}

#[cfg(not(target_os = "android"))]
mod host {
    use serde::de::DeserializeOwned;
    use serde::Serialize;

    pub fn call<R: DeserializeOwned>(command: &str, _payload: impl Serialize) -> Result<R, String> {
        Err(format!("{command}: not running on Android"))
    }
}

#[cfg(target_os = "android")]
pub use host::init;

/// Runs one Kotlin command and waits for its answer.
pub fn call<R: DeserializeOwned>(command: &str, payload: impl Serialize) -> Result<R, String> {
    host::call(command, payload)
}

/// A command whose answer carries nothing.
pub fn run(command: &str, payload: impl Serialize) -> Result<(), String> {
    call::<serde_json::Value>(command, payload).map(|_| ())
}

/// Bytes as hexadecimal text, the form they cross the bridge in.
pub fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[usize::from(b >> 4)] as char);
        out.push(DIGITS[usize::from(b & 0x0F)] as char);
    }
    out
}

pub fn from_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedDevice {
    /// `AA:BB:CC:DD:EE:FF`. Never leaves the native layer.
    pub address: String,
    pub name: String,
    pub connected: bool,
    #[serde(default)]
    pub battery: Option<u8>,
    #[serde(default)]
    pub uuids: Vec<String>,
    #[serde(default)]
    pub low_energy_only: bool,
}

/// What Android says about Bluetooth on this phone. Asking sends nothing.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedReport {
    pub radio_present: bool,
    pub radio_enabled: bool,
    /// Whether the user has let this application see Bluetooth devices.
    pub permission: bool,
    #[serde(default)]
    pub devices: Vec<PairedDevice>,
}

pub fn paired() -> Result<PairedReport, String> {
    call("paired", ())
}

pub fn ask_permissions() -> Result<(), String> {
    run("askPermissions", ())
}

pub fn open_bluetooth_settings() -> Result<(), String> {
    run("openBluetoothSettings", ())
}

/// Writes one line to Android's log, for whoever is watching it with `adb
/// logcat -s Aurora`. For the few events worth a line — a model identified,
/// a connection refused — and never for anything that happens many times a
/// second. Addresses are not written.
pub fn log(message: &str) {
    let _ = run("log", serde_json::json!({ "message": message }));
}

/// Starts or stops the foreground service that keeps the process running
/// while the application is not on screen.
pub fn keep_alive(on: bool) -> Result<(), String> {
    run("keepAlive", serde_json::json!({ "on": on }))
}

/// Where the voice model shipped in the package has been unpacked to, if
/// this package carries one. Unpacks it on first use, which takes a moment.
pub fn bundled_model_root() -> Result<Option<String>, String> {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        path: Option<String>,
    }
    call::<Reply>("modelRoot", ()).map(|reply| reply.path)
}

/// What this phone calls itself: its Bluetooth name where that can be read,
/// otherwise the device name from its settings. `None` if it gives neither.
pub fn host_name() -> Result<Option<String>, String> {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        name: Option<String>,
    }
    call::<Reply>("hostName", ()).map(|reply| reply.name)
}

/// Tells the Kotlin side whether the model is in memory, which is the one
/// thing it cannot see for itself and needs for its own log lines.
pub fn note_memory(what: &str) {
    let _ = run("noteMemory", serde_json::json!({ "what": what }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_survive_the_trip_through_hexadecimal() {
        let frame = [0x1F, 0x03, 0x01, 0x00, 0xFF, 0x80, 0x00];
        assert_eq!(to_hex(&frame), "1f030100ff8000");
        assert_eq!(from_hex(&to_hex(&frame)).unwrap(), frame);
        assert_eq!(from_hex("").unwrap(), Vec::<u8>::new());
        assert_eq!(from_hex("1F03").unwrap(), vec![0x1F, 0x03]);
    }

    #[test]
    fn text_that_is_not_hexadecimal_is_refused() {
        assert_eq!(from_hex("abc"), None, "odd length");
        assert_eq!(from_hex("zz"), None);
        assert_eq!(from_hex("0x"), None);
        assert_eq!(from_hex("é0"), None);
    }

    /// What Kotlin sends, as this side reads it.
    #[test]
    fn the_paired_report_reads_as_kotlin_writes_it() {
        let report: PairedReport = serde_json::from_str(
            r#"{"radioPresent":true,"radioEnabled":true,"permission":true,"devices":[
                {"address":"E4:58:BC:F9:F0:2E","name":"Aurora","connected":true,"battery":70,
                 "uuids":["00000000-deca-fade-deca-deafdecacaff"],"lowEnergyOnly":false},
                {"address":"11:22:33:44:55:66","name":"Car","connected":false,"uuids":[]}]}"#,
        )
        .unwrap();
        assert!(report.permission && report.radio_enabled);
        assert_eq!(report.devices[0].battery, Some(70));
        assert_eq!(report.devices[1].battery, None, "no level is not zero");
        assert!(!report.devices[1].low_energy_only);
    }

    #[test]
    fn without_permission_there_are_simply_no_devices() {
        let report: PairedReport =
            serde_json::from_str(r#"{"radioPresent":true,"radioEnabled":false,"permission":false}"#)
                .unwrap();
        assert!(!report.permission);
        assert!(report.devices.is_empty());
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn off_android_the_bridge_says_so_rather_than_pretending() {
        assert!(paired().unwrap_err().contains("not running on Android"));
    }
}
