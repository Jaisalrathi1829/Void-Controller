//! Bose QC Control Center — native layer.
//!
//! Architecture note: the frontend has no Bluetooth surface. It calls the
//! `#[tauri::command]` handlers below, each of which takes typed parameters
//! that are validated before any backend sees them. There is deliberately no
//! command that accepts a UUID, a characteristic handle, or a byte array.

pub mod audio;
pub mod autostart;
pub mod bluetooth;
pub mod bose;
pub mod device;
pub mod settings;
pub mod tray;
pub mod util;
pub mod voice;
pub mod watcher;

use bose::{MockBoseDevice, RealBoseDevice};
use device::capability::{CapabilityKey, HardwareProof, Mechanism};
use device::command::{CommandOutcome, DeviceCommand};
use device::state::{
    DeviceSnapshot, DeviceSource, NoiseControlMode, NoiseControlState, WindowsAudioState,
};
use device::traits::BoseDevice;
use parking_lot::RwLock;
use settings::{SettingKey, SettingsStore, StoredSettings};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{Emitter, Manager};
use voice::{VoiceFeedback, VoiceStatus};

/// Which backend the session is using, and the settings that shape it.
pub struct AppState {
    device: RwLock<Arc<dyn BoseDevice>>,
    settings: Arc<SettingsStore>,
    /// Whether the application is attached to the real headphones. Shared
    /// with the real backend, which reads it on every request.
    attached: Arc<AtomicBool>,
    voice: VoiceFeedback,
}

impl AppState {
    /// Starts attached or not according to the auto-connect setting.
    pub fn new(source: DeviceSource, settings: Arc<SettingsStore>, voice: VoiceFeedback) -> Self {
        let attached = Arc::new(AtomicBool::new(settings.get().auto_connect));
        let state = Self {
            device: RwLock::new(Arc::new(MockBoseDevice::default())),
            settings,
            attached,
            voice,
        };
        state.switch_to(source);
        state
    }

    pub fn settings(&self) -> &SettingsStore {
        &self.settings
    }

    pub fn voice(&self) -> &VoiceFeedback {
        &self.voice
    }

    /// Attaches to the real headphones: an explicit "use them" from the user.
    pub fn attach(&self) {
        self.attached.store(true, Ordering::SeqCst);
    }

    /// Lets go of the real headphones until the user asks for them again.
    pub fn detach(&self) {
        self.attached.store(false, Ordering::SeqCst);
    }

    /// Records which headphones this application is about, the first time
    /// they are seen, when the remembered ones are no longer paired, or when
    /// they have been renamed. Only the name is ever shown.
    pub fn remember_device(&self, seen: &bose::real::Sighting) {
        let stored = self.settings.get();
        let same_device = stored.preferred_device_id.as_deref() == Some(seen.instance_id.as_str());
        let same_name = stored.preferred_device_name.as_deref() == Some(seen.name.as_str());
        if same_device && same_name {
            return;
        }
        let _ = self.settings.update(|s| {
            s.preferred_device_id = Some(seen.instance_id.clone());
            s.preferred_device_name = Some(seen.name.clone());
        });
    }

    /// The remembered headphones' instance id, if they are still paired.
    fn preferred_device(&self) -> Option<String> {
        let stored = self.settings.get().preferred_device_id?;
        bose::real::sight_device(Some(&stored)).map(|s| s.instance_id)
    }

    /// The backend to start on.
    ///
    /// Real hardware, if Windows already knows a Bose device — the headphones
    /// are paired with this PC, and looking at them is what the application
    /// is for. The simulated device otherwise, so there is something to
    /// explore, labelled as simulated. Deciding this only enumerates what
    /// Windows has; nothing is sent to any device.
    pub fn initial_source() -> DeviceSource {
        #[cfg(windows)]
        {
            let paired = bluetooth::pnp::enumerate_bluetooth_devices()
                .into_iter()
                .filter(|d| d.is_top_level())
                .any(|d| {
                    bose::is_bose_device(
                        d.vendor_id,
                        d.friendly_name.as_deref().unwrap_or_default(),
                    )
                });
            if paired {
                return DeviceSource::Real;
            }
        }
        DeviceSource::Mock
    }

    fn backend_for(&self, source: DeviceSource) -> Arc<dyn BoseDevice> {
        match source {
            DeviceSource::Mock => Arc::new(MockBoseDevice::default()),
            DeviceSource::Real => Arc::new(RealBoseDevice::with_attachment(
                self.preferred_device(),
                Arc::clone(&self.attached),
            )),
        }
    }

    pub fn current(&self) -> Arc<dyn BoseDevice> {
        self.device.read().clone()
    }

    pub fn switch_to(&self, source: DeviceSource) {
        let backend = self.backend_for(source);
        *self.device.write() = backend;
    }
}

/// The settings as the UI sees them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    /// Read from Windows' startup list, not from the settings file.
    pub start_with_windows: bool,
    pub start_minimized: bool,
    pub minimize_to_tray: bool,
    pub auto_connect: bool,
    pub auto_reconnect: bool,
    pub notify_connected: bool,
    pub notify_disconnected: bool,
    pub notify_low_battery: bool,
    /// The remembered headphones' name. Their identifier is never included.
    pub preferred_device_name: Option<String>,
    pub voice_enabled: bool,
    pub voice_id: String,
    pub voice_speed_percent: u16,
    pub voice_volume_percent: u8,
    pub announce_mode_changes: bool,
    pub announce_connection: bool,
    pub announce_battery: bool,
    pub voice_use_default_output: bool,
    /// Switches that cannot be used in this build, each with the reason.
    pub unavailable: BTreeMap<SettingKey, String>,
}

impl SettingsView {
    fn build(stored: &StoredSettings) -> Self {
        let mut unavailable = BTreeMap::new();
        let autostart_blocked = autostart::unavailable_reason();
        if let Some(why) = autostart_blocked {
            unavailable.insert(SettingKey::StartWithWindows, why.to_string());
        }
        let start_with_windows = autostart_blocked.is_none()
            && autostart::Autostart::for_this_app().is_ok_and(|entry| entry.is_enabled());

        Self {
            start_with_windows,
            start_minimized: stored.start_minimized,
            minimize_to_tray: stored.minimize_to_tray,
            auto_connect: stored.auto_connect,
            auto_reconnect: stored.auto_reconnect,
            notify_connected: stored.notify_connected,
            notify_disconnected: stored.notify_disconnected,
            notify_low_battery: stored.notify_low_battery,
            preferred_device_name: stored.preferred_device_name.clone(),
            voice_enabled: stored.voice_enabled,
            voice_id: stored.voice_id.clone(),
            voice_speed_percent: stored.voice_speed_percent,
            voice_volume_percent: stored.voice_volume_percent,
            announce_mode_changes: stored.announce_mode_changes,
            announce_connection: stored.announce_connection,
            announce_battery: stored.announce_battery,
            voice_use_default_output: stored.voice_use_default_output,
            unavailable,
        }
    }
}

/// Serializable error returned to the frontend.
#[derive(Debug, serde::Serialize)]
pub struct UiError {
    pub message: String,
    pub kind: String,
}

impl From<device::DeviceError> for UiError {
    fn from(e: device::DeviceError) -> Self {
        let kind = match &e {
            device::DeviceError::NotConnected => "not-connected",
            device::DeviceError::BluetoothUnavailable => "bluetooth-unavailable",
            device::DeviceError::Unreachable => "unreachable",
            device::DeviceError::Timeout(_) => "timeout",
            device::DeviceError::Unsupported { .. } => "unsupported",
            device::DeviceError::InvalidResponse(_) => "invalid-response",
            device::DeviceError::Rejected(_) => "rejected",
            device::DeviceError::ControlLink(_) => "control-link",
            device::DeviceError::Platform(_) => "platform",
            device::DeviceError::InvalidInput(_) => "invalid-input",
        };
        Self {
            message: e.to_string(),
            kind: kind.to_string(),
        }
    }
}

type UiResult<T> = Result<T, UiError>;

// --- Commands ---------------------------------------------------------------

/// Attaches Windows audio state to a device snapshot.
///
/// Audio is a system concern rather than a device-backend one: the endpoint
/// exists whether we are talking to a mock or to real headphones. Keeping it
/// out of the backends means the mock cannot fabricate a Windows volume, which
/// would be a particularly misleading thing to simulate.
///
/// Reading an endpoint successfully is genuine hardware evidence that the
/// volume path works, so the capability is promoted accordingly.
fn attach_windows_audio(mut snapshot: DeviceSnapshot) -> DeviceSnapshot {
    #[cfg(windows)]
    {
        match audio::default_output_endpoint() {
            Ok(Some(ep)) => {
                let proof = HardwareProof::observed_passively(
                    CapabilityKey::Volume,
                    format!("read Windows volume from endpoint {:?}", ep.name),
                );
                let _ = snapshot
                    .capabilities
                    .get_mut(CapabilityKey::Volume)
                    .verify_with_hardware(&proof, Mechanism::WindowsAudio);

                snapshot.windows_audio = Some(WindowsAudioState {
                    endpoint_name: ep.name,
                    endpoint_id: util::stable_id(&ep.id),
                    volume_percent: ep.volume_percent,
                    muted: ep.muted,
                    is_default_render: ep.is_default_render,
                    is_default_communications: ep.is_default_communications,
                    sample_rate_hz: ep.sample_rate_hz,
                    channels: ep.channels,
                });
            }
            Ok(None) => {
                let _ = snapshot
                    .capabilities
                    .get_mut(CapabilityKey::Volume)
                    .mark_supported(
                        Mechanism::WindowsAudio,
                        "Core Audio is available but no active output endpoint is present.",
                    );
            }
            Err(e) => {
                let _ = snapshot
                    .capabilities
                    .get_mut(CapabilityKey::Volume)
                    .mark_unsupported(format!("Core Audio unavailable: {e}"));
            }
        }
    }
    snapshot
}

/// Resolves the endpoint id to act on. `None` means the default endpoint.
///
/// The UI only ever sees salted hashes, so it cannot name a raw endpoint id —
/// which is deliberate. Volume commands therefore act on the default endpoint.
fn target_endpoint() -> Option<&'static str> {
    None
}

#[tauri::command]
async fn get_snapshot(state: tauri::State<'_, AppState>) -> UiResult<DeviceSnapshot> {
    let dev = state.current();
    let snapshot = dev.snapshot().await?;
    // The one place a mode announcement can come from: what the device was
    // just found to be reporting. A request to change mode never reaches the
    // voice service; only the state that results from it does, here.
    state.voice().observe_snapshot(&snapshot);
    Ok(attach_windows_audio(snapshot))
}

#[tauri::command]
fn list_audio_endpoints() -> UiResult<Vec<audio::AudioEndpoint>> {
    #[cfg(windows)]
    {
        Ok(audio::list_output_endpoints()?)
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

#[tauri::command]
async fn get_device_source(state: tauri::State<'_, AppState>) -> UiResult<DeviceSource> {
    Ok(state.current().source())
}

#[tauri::command]
async fn set_device_source(
    state: tauri::State<'_, AppState>,
    source: DeviceSource,
) -> UiResult<DeviceSnapshot> {
    // Choosing real hardware by hand is asking to be attached to it.
    if source == DeviceSource::Real {
        state.attach();
    }
    state.switch_to(source);
    let dev = state.current();
    let snapshot = dev.snapshot().await?;
    state.voice().observe_snapshot(&snapshot);
    Ok(attach_windows_audio(snapshot))
}

fn platform_error(message: String) -> UiError {
    UiError {
        message,
        kind: "platform".to_string(),
    }
}

#[tauri::command]
fn get_settings(state: tauri::State<'_, AppState>) -> SettingsView {
    SettingsView::build(&state.settings().get())
}

/// Flips one switch and returns the settings as they then stand.
///
/// The key is one of a closed set and the value a boolean; there is nothing
/// else to pass. Switches with an immediate effect take it here — the startup
/// list is edited, the tray icon appears or goes — and a switch whose effect
/// could not be carried out is not recorded as on.
#[tauri::command]
fn set_setting(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    key: SettingKey,
    value: bool,
) -> UiResult<SettingsView> {
    let before = state.settings().get();
    match key {
        SettingKey::StartWithWindows => {
            if let Some(why) = autostart::unavailable_reason() {
                return Err(UiError {
                    message: why.to_string(),
                    kind: "unsupported".to_string(),
                });
            }
            let entry = autostart::Autostart::for_this_app().map_err(platform_error)?;
            if value {
                entry.enable().map_err(platform_error)?;
            } else {
                entry.disable().map_err(platform_error)?;
            }
        }
        SettingKey::MinimizeToTray => {
            // The icon first: a window that hides with nothing to bring it
            // back would be worse than the switch failing.
            tray::sync(&app, value).map_err(platform_error)?;
            state
                .settings()
                .update(|s| s.set(key, value))
                .map_err(platform_error)?;
        }
        _ => {
            state
                .settings()
                .update(|s| s.set(key, value))
                .map_err(platform_error)?;
        }
    }
    let after = state.settings().get();
    // So that switching voice feedback, or one kind of announcement, off
    // takes effect on what is being said right now and not just from the
    // next event.
    state.voice().settings_changed(&before, &after);
    Ok(SettingsView::build(&after))
}

/// The voice settings that are not on/off switches.
///
/// Each is a named choice with a bounded value; nothing free-form.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum VoiceOption {
    /// One of the installed model's voices, by its id.
    Voice { id: String },
    /// Speaking rate as a percentage of the model's natural pace.
    Speed { percent: u16 },
    /// Loudness of speech, 0-100. Windows' own volume is never touched.
    Volume { percent: u8 },
}

impl VoiceOption {
    fn validate(&self) -> Result<(), UiError> {
        let invalid = |message: String| UiError {
            message,
            kind: "invalid-input".to_string(),
        };
        match self {
            Self::Voice { id } if voice::assets::voice(id).is_none() => {
                Err(invalid(format!("There is no voice called {id:?}.")))
            }
            Self::Speed { percent } if !settings::VOICE_SPEED_RANGE.contains(percent) => {
                Err(invalid(format!(
                    "Speech speed must be {}-{} percent, got {percent}.",
                    settings::VOICE_SPEED_RANGE.start(),
                    settings::VOICE_SPEED_RANGE.end()
                )))
            }
            Self::Volume { percent } if *percent > 100 => Err(invalid(format!(
                "Speech volume must be 0-100 percent, got {percent}."
            ))),
            _ => Ok(()),
        }
    }
}

#[tauri::command]
fn set_voice_option(
    state: tauri::State<'_, AppState>,
    option: VoiceOption,
) -> UiResult<SettingsView> {
    option.validate()?;
    let before = state.settings().get();
    let after = state
        .settings()
        .update(|s| match &option {
            VoiceOption::Voice { id } => s.voice_id = id.clone(),
            VoiceOption::Speed { percent } => s.voice_speed_percent = *percent,
            VoiceOption::Volume { percent } => s.voice_volume_percent = *percent,
        })
        .map_err(platform_error)?;
    state.voice().settings_changed(&before, &after);
    Ok(SettingsView::build(&after))
}

#[tauri::command]
fn get_voice_status(state: tauri::State<'_, AppState>) -> VoiceStatus {
    state.voice().status()
}

/// Speaks the sample sentence. Takes no text: the frontend cannot use this to
/// have arbitrary words spoken.
#[tauri::command]
fn test_voice(state: tauri::State<'_, AppState>) -> UiResult<()> {
    state.voice().test().map_err(|message| UiError {
        message,
        kind: "voice-unavailable".to_string(),
    })
}

#[tauri::command]
fn stop_speaking(state: tauri::State<'_, AppState>) {
    state.voice().stop();
}

/// Looks again for the voice model, after installing it. Blocking work, so it
/// runs off the command thread.
#[tauri::command]
async fn recheck_voice_model(state: tauri::State<'_, AppState>) -> UiResult<VoiceStatus> {
    let voice = state.voice().clone();
    tokio::task::spawn_blocking(move || voice.recheck())
        .await
        .map_err(|e| platform_error(format!("the check did not finish: {e}")))
}

/// The single entry point for every device operation.
///
/// Validation happens here, once, for both mock and real backends — so the
/// mock exercises exactly the same checks the hardware path does.
#[tauri::command]
async fn execute_command(
    state: tauri::State<'_, AppState>,
    command: DeviceCommand,
) -> UiResult<CommandOutcome> {
    command.validate()?;

    let dev = state.current();
    let outcome = match command {
        DeviceCommand::Connect => dev.connect().await?,
        DeviceCommand::Disconnect => dev.disconnect().await?,
        DeviceCommand::Reconnect => {
            let _ = dev.disconnect().await;
            dev.connect().await?
        }
        DeviceCommand::SetNoiseControl { mode } => dev.set_noise_control(mode).await?,
        DeviceCommand::SetEqualizer { settings } => dev.set_equalizer(settings).await?,

        DeviceCommand::RefreshSnapshot
        | DeviceCommand::ReadBattery
        | DeviceCommand::ReadNoiseControl
        | DeviceCommand::ReadEqualizer
        | DeviceCommand::ReadDeviceInfo => CommandOutcome::applied(),

        DeviceCommand::SetNoiseControlLevel { level } => {
            dev.set_noise_control_level(level).await?
        }

        // Windows system volume. Set, then re-read the endpoint and only
        // report success if it actually reports the requested value.
        DeviceCommand::SetSystemVolume { percent } => {
            #[cfg(windows)]
            {
                let actual = audio::set_volume(target_endpoint(), percent)?;
                if actual == percent {
                    CommandOutcome::applied()
                } else {
                    CommandOutcome::sent_unverified(format!(
                        "Endpoint reports {actual}% after requesting {percent}%."
                    ))
                }
            }
            #[cfg(not(windows))]
            {
                CommandOutcome::unsupported("Core Audio is only available on Windows.")
            }
        }

        DeviceCommand::SetSystemMute { muted } => {
            #[cfg(windows)]
            {
                let actual = audio::set_mute(target_endpoint(), muted)?;
                if actual == muted {
                    CommandOutcome::applied()
                } else {
                    CommandOutcome::sent_unverified(
                        "Endpoint did not report the requested mute state.",
                    )
                }
            }
            #[cfg(not(windows))]
            {
                CommandOutcome::unsupported("Core Audio is only available on Windows.")
            }
        }

        // Media transport goes through the Windows session model, which is a
        // separate integration and not implemented yet.
        DeviceCommand::MediaPlayPause
        | DeviceCommand::MediaNext
        | DeviceCommand::MediaPrevious => {
            CommandOutcome::unsupported("Media transport control is not implemented yet.")
        }
    };

    Ok(outcome)
}

/// The noise-control modes the UI may ask for by name.
///
/// Narrower than [`NoiseControlMode`] on purpose. The state type has to be
/// able to describe "off" if a device ever reports it; the *request* type only
/// contains what has an implementation, so a request for anything else does
/// not deserialize, let alone reach a device.
///
/// `Custom` names no slot. Which of the headphones' custom modes it means is
/// decided natively, from the list the headphones themselves report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoiseModeRequest {
    Quiet,
    Aware,
    Custom,
}

impl From<NoiseModeRequest> for NoiseControlMode {
    fn from(request: NoiseModeRequest) -> Self {
        match request {
            NoiseModeRequest::Quiet => Self::Quiet,
            NoiseModeRequest::Aware => Self::Aware,
            NoiseModeRequest::Custom => Self::Custom,
        }
    }
}

/// Reads the noise-control mode from the device now.
///
/// `None` means there is no reading to give. It is never a default.
#[tauri::command]
async fn get_current_mode(
    state: tauri::State<'_, AppState>,
) -> UiResult<Option<NoiseControlState>> {
    Ok(state.current().noise_control().await?)
}

/// Asks the device to switch to Quiet, Aware or its custom mode.
///
/// The outcome is `applied` only if the device then reported that mode.
#[tauri::command]
async fn set_noise_control_mode(
    state: tauri::State<'_, AppState>,
    mode: NoiseModeRequest,
) -> UiResult<CommandOutcome> {
    Ok(state.current().set_noise_control(mode.into()).await?)
}

/// Opens the Windows Bluetooth settings page.
///
/// The target URI is fixed in native code and takes no parameter, so the
/// frontend cannot use this to open an arbitrary URL or launch a program.
/// That is why this exists as a dedicated command rather than granting the
/// opener plugin's general `open-url` permission to the UI.
#[tauri::command]
fn open_bluetooth_settings(app: tauri::AppHandle) -> UiResult<()> {
    #[cfg(windows)]
    {
        use tauri_plugin_opener::OpenerExt;
        app.opener()
            .open_url("ms-settings:bluetooth", None::<&str>)
            .map_err(|e| UiError {
                message: format!("Could not open Windows Bluetooth settings: {e}"),
                kind: "platform".to_string(),
            })?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Err(UiError {
            message: "Bluetooth settings are only available on Windows.".to_string(),
            kind: "unsupported".to_string(),
        })
    }
}

#[tauri::command]
fn get_bluetooth_availability() -> bluetooth::BluetoothAvailability {
    #[cfg(windows)]
    {
        bluetooth::radio::availability()
    }
    #[cfg(not(windows))]
    {
        bluetooth::BluetoothAvailability::unavailable("Not running on Windows.")
    }
}

/// Read-only enumeration of Bluetooth devices Windows knows about.
#[tauri::command]
fn list_bluetooth_devices() -> Vec<bluetooth::DiscoveredDevice> {
    #[cfg(windows)]
    {
        use bluetooth::{DiscoveredDevice, DiscoveredTransport};
        bluetooth::pnp::enumerate_bluetooth_devices()
            .into_iter()
            .filter(|d| d.is_top_level())
            .map(|d| {
                let name = d
                    .friendly_name
                    .clone()
                    .unwrap_or_else(|| "Unnamed device".to_string());
                DiscoveredDevice {
                    id: util::stable_id(&d.instance_id),
                    looks_like_bose: bose::looks_like_bose(&name),
                    name,
                    transport: if d.instance_id.starts_with("BTHLE") {
                        DiscoveredTransport::LowEnergy
                    } else {
                        DiscoveredTransport::Classic
                    },
                    connected: d.is_connected,
                    battery_percent: d.battery_percent,
                }
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    util::set_salt(util::generate_salt());

    tauri::Builder::default()
        // One instance only: two would fight over the headphones' single
        // control connection. Starting the application again brings the
        // running one forward — including out of the tray. Registered first,
        // as the plugin requires.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let handle = app.handle().clone();

            let store = Arc::new(match app.path().app_config_dir() {
                Ok(dir) => SettingsStore::load(dir.join("settings.json")),
                // Nowhere to keep them: defaults, for this run only.
                Err(_) => SettingsStore::in_memory(StoredSettings::default()),
            });
            let settings = store.get();

            // Voice feedback. The model lives under the application's local
            // data folder; nothing is fetched, and nothing is loaded until
            // there is something to say.
            let voice_root = app
                .path()
                .app_local_data_dir()
                .map(|dir| dir.join("voice"))
                .unwrap_or_else(|_| std::path::PathBuf::from("voice"));
            let status_target = handle.clone();
            let voice = VoiceFeedback::start(
                Arc::clone(&store),
                Box::new(
                    voice::backend::KokoroBackend::new(voice_root, Arc::clone(&store))
                        // An installer that carries the model puts it here.
                        .with_bundled(app.path().resource_dir().ok().map(|dir| dir.join("voice"))),
                ),
                voice::Timing::standard(),
                move |status| {
                    let _ = status_target.emit(voice::VOICE_STATUS_EVENT, status);
                },
            );

            // Start on real hardware when the headphones are already paired
            // with this PC, and on the labelled simulation otherwise.
            app.manage(AppState::new(AppState::initial_source(), store, voice));

            if settings.minimize_to_tray {
                if let Err(e) = tray::sync(&handle, true) {
                    eprintln!("tray icon unavailable: {e}");
                }
            }

            // The window is created hidden so that "start minimized" never
            // flashes it on screen. It stays hidden only if there is a tray
            // icon to bring it back; otherwise it is shown, minimized or not.
            let stay_hidden = settings.start_minimized && tray::is_present(&handle);
            if !stay_hidden {
                if let Some(window) = app.get_webview_window(tray::MAIN_WINDOW) {
                    let _ = window.show();
                    if settings.start_minimized {
                        let _ = window.minimize();
                    }
                }
            }

            watcher::spawn(handle);
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != tray::MAIN_WINDOW {
                return;
            }
            let app = window.app_handle();
            let Some(state) = app.try_state::<AppState>() else {
                return;
            };
            // Only while the setting is on *and* the icon exists.
            if !state.settings().get().minimize_to_tray || !tray::is_present(app) {
                return;
            }
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    let _ = window.hide();
                }
                tauri::WindowEvent::Resized(_) => {
                    if window.is_minimized().unwrap_or(false) {
                        let _ = window.hide();
                    }
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            get_settings,
            set_setting,
            set_voice_option,
            get_voice_status,
            test_voice,
            stop_speaking,
            recheck_voice_model,
            get_device_source,
            set_device_source,
            execute_command,
            get_current_mode,
            set_noise_control_mode,
            get_bluetooth_availability,
            list_bluetooth_devices,
            list_audio_endpoints,
            open_bluetooth_settings,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Bose QC Control Center")
        .run(|app, event| {
            // Stop speaking and release the model before the process goes.
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app.try_state::<AppState>() {
                    state.voice().shutdown();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An application state whose voice service has no model to find.
    fn state(source: DeviceSource, store: Arc<SettingsStore>) -> AppState {
        let nowhere = std::env::temp_dir().join("bose-qc-no-voice-model-here");
        let voice = VoiceFeedback::start(
            Arc::clone(&store),
            Box::new(voice::backend::KokoroBackend::new(nowhere, Arc::clone(&store))),
            voice::Timing::standard(),
            |_| {},
        );
        AppState::new(source, store, voice)
    }

    #[test]
    fn the_typed_noise_request_accepts_exactly_three_names() {
        let parse = |json: &str| serde_json::from_str::<NoiseModeRequest>(json);
        assert_eq!(parse("\"quiet\"").unwrap(), NoiseModeRequest::Quiet);
        assert_eq!(parse("\"aware\"").unwrap(), NoiseModeRequest::Aware);
        assert_eq!(parse("\"custom\"").unwrap(), NoiseModeRequest::Custom);
        // A valid *state*, but not something this operation can be asked for.
        assert!(parse("\"off\"").is_err());
        assert!(parse("\"Quiet\"").is_err());
    }

    /// The request cannot smuggle in an index, a slot, a frame or bytes.
    /// "Custom" in particular carries no slot number: which mode it means is
    /// decided natively.
    #[test]
    fn the_typed_noise_request_has_no_raw_form() {
        let parse = |json: &str| serde_json::from_str::<NoiseModeRequest>(json);
        assert!(parse("1").is_err());
        assert!(parse("2").is_err());
        assert!(parse("[31, 3, 5, 2, 1, 0]").is_err());
        assert!(parse("\"1F 03 05 02 01 00\"").is_err());
        assert!(parse("{\"index\": 1}").is_err());
        assert!(parse("{\"custom\": 3}").is_err());
        assert!(parse("\"custom:3\"").is_err());
    }

    #[test]
    fn auto_connect_decides_whether_the_application_starts_attached() {
        for auto_connect in [true, false] {
            let store = Arc::new(SettingsStore::in_memory(StoredSettings {
                auto_connect,
                ..StoredSettings::default()
            }));
            let state = state(DeviceSource::Mock, store);
            assert_eq!(state.attached.load(Ordering::SeqCst), auto_connect);
        }
    }

    #[test]
    fn attaching_and_detaching_are_explicit() {
        let store = Arc::new(SettingsStore::in_memory(StoredSettings {
            auto_connect: false,
            ..StoredSettings::default()
        }));
        let state = state(DeviceSource::Mock, store);
        state.attach();
        assert!(state.attached.load(Ordering::SeqCst));
        state.detach();
        assert!(!state.attached.load(Ordering::SeqCst));
    }

    /// The headphones are remembered by an identifier that stays native; the
    /// UI's view of the settings carries their name and nothing else.
    #[test]
    fn the_settings_view_names_the_headphones_without_identifying_them() {
        let store = Arc::new(SettingsStore::in_memory(StoredSettings::default()));
        let state = state(DeviceSource::Mock, store);
        state.remember_device(&bose::real::Sighting {
            instance_id: r"BTHENUM\DEV_E458BCF9F02E\7&78167D1&0&BLUETOOTHDEVICE_E458BCF9F02E".into(),
            name: "Aurora".into(),
            connected: true,
            battery: Some(80),
        });

        let view = SettingsView::build(&state.settings().get());
        assert_eq!(view.preferred_device_name.as_deref(), Some("Aurora"));
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("E458BCF9F02E"), "{json}");
        assert!(!json.contains("BTHENUM"), "{json}");
        assert!(!json.contains("preferredDeviceId"), "{json}");
    }

    #[test]
    fn the_settings_view_uses_the_names_the_frontend_expects() {
        let view = SettingsView::build(&StoredSettings {
            minimize_to_tray: true,
            ..StoredSettings::default()
        });
        let json = serde_json::to_value(&view).unwrap();
        for key in [
            "startWithWindows",
            "startMinimized",
            "minimizeToTray",
            "autoConnect",
            "autoReconnect",
            "notifyConnected",
            "notifyDisconnected",
            "notifyLowBattery",
            "voiceEnabled",
            "announceModeChanges",
            "announceConnection",
            "announceBattery",
            "voiceUseDefaultOutput",
        ] {
            assert!(json[key].is_boolean(), "{key}");
        }
        assert!(json["voiceId"].is_string());
        assert!(json["voiceSpeedPercent"].is_u64());
        assert!(json["voiceVolumePercent"].is_u64());
        assert_eq!(json["minimizeToTray"], true);
        assert!(json["unavailable"].is_object());
        // Tests run in a development build, where the startup switch must be
        // off and must say why.
        if cfg!(debug_assertions) {
            assert_eq!(json["startWithWindows"], false);
            assert!(json["unavailable"]["startWithWindows"].is_string());
        }
    }

    #[test]
    fn voice_options_are_named_and_bounded() {
        let parse = |json: &str| serde_json::from_str::<VoiceOption>(json);
        let ok = |json: &str| parse(json).unwrap().validate().is_ok();

        assert!(ok(r#"{"kind":"voice","id":"af_bella"}"#));
        assert!(ok(r#"{"kind":"speed","percent":50}"#));
        assert!(ok(r#"{"kind":"speed","percent":200}"#));
        assert!(ok(r#"{"kind":"volume","percent":0}"#));
        assert!(ok(r#"{"kind":"volume","percent":100}"#));

        // A voice the installed model does not have.
        assert!(!ok(r#"{"kind":"voice","id":"af_heart"}"#));
        assert!(!ok(r#"{"kind":"voice","id":""}"#));
        assert!(!ok(r#"{"kind":"speed","percent":49}"#));
        assert!(!ok(r#"{"kind":"speed","percent":201}"#));
        assert!(!ok(r#"{"kind":"volume","percent":101}"#));

        // And nothing that is not one of the three.
        assert!(parse(r#"{"kind":"text","value":"say this"}"#).is_err());
        assert!(parse(r#"{"kind":"speed","percent":-5}"#).is_err());
        assert!(parse(r#"{"kind":"volume","percent":300}"#).is_err());
    }

    /// The level command carries one bounded number and nothing else.
    #[test]
    fn the_level_command_is_bounded_before_it_reaches_a_backend() {
        assert!(DeviceCommand::SetNoiseControlLevel { level: 0 }.validate().is_ok());
        assert!(DeviceCommand::SetNoiseControlLevel { level: 10 }.validate().is_ok());
        assert!(DeviceCommand::SetNoiseControlLevel { level: 11 }.validate().is_err());
        assert!(DeviceCommand::SetNoiseControlLevel { level: 255 }.validate().is_err());

        let json = serde_json::to_string(&DeviceCommand::SetNoiseControlLevel { level: 4 }).unwrap();
        assert_eq!(json, r#"{"kind":"setNoiseControlLevel","level":4}"#);
    }

    /// The frontend switches on these exact strings to decide what to tell the
    /// user. A name that does not match falls through silently — which, for
    /// "sent but not verified", would mean saying nothing at all.
    #[test]
    fn command_outcomes_use_the_names_the_frontend_switches_on() {
        let kind = |o: &CommandOutcome| {
            serde_json::to_value(o).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(kind(&CommandOutcome::applied()), "applied");
        assert_eq!(kind(&CommandOutcome::sent_unverified("x")), "sent-unverified");
        assert_eq!(kind(&CommandOutcome::unsupported("x")), "unsupported");
        assert_eq!(
            kind(&CommandOutcome::Rejected { reason: "x".into() }),
            "rejected"
        );

        let applied = serde_json::to_value(CommandOutcome::applied()).unwrap();
        assert!(applied["verifiedAt"].is_string());
    }
}
