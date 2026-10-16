//! Void — native layer.
//!
//! Architecture note: the frontend has no Bluetooth surface. It calls the
//! `#[tauri::command]` handlers below, each of which takes typed parameters
//! that are validated before any backend sees them. There is deliberately no
//! command that accepts a UUID, a characteristic handle, or a byte array.

pub mod android;
pub mod audio;
pub mod autostart;
pub mod bluetooth;
pub mod bose;
pub mod device;
pub mod host;
pub mod settings;
#[cfg(desktop)]
pub mod tray;
pub mod util;
pub mod voice;
pub mod watcher;
#[cfg(desktop)]
pub mod widget;

use bose::{MockBoseDevice, RealBoseDevice};
#[cfg(any(windows, target_os = "android"))]
use device::capability::{CapabilityKey, HardwareProof, Mechanism};
use device::command::{CommandOutcome, DeviceCommand};
#[cfg(any(windows, target_os = "android"))]
use device::state::SystemAudioState;
use device::state::{DeviceSnapshot, DeviceSource, NoiseControlMode, NoiseControlState};
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
            // A different pair of headphones has not told this application
            // what it is called, whatever the last pair said.
            if !same_device {
                s.reported_device_name = None;
            }
            s.preferred_device_id = Some(seen.instance_id.clone());
            s.preferred_device_name = Some(seen.name.clone());
        });
    }

    /// Has the headphones just seen go by the name they gave themselves, if
    /// this application has read one from them, rather than by the system's.
    ///
    /// The system's is the one it learned when they were paired or last
    /// connected. After a rename it can stay the old one for some time —
    /// Windows keeps it until they are paired again — and an application
    /// that had just renamed them would go on announcing the old name.
    pub fn go_by_own_name(&self, seen: &mut bose::real::Sighting) {
        let stored = self.settings.get();
        if stored.preferred_device_id.as_deref() != Some(seen.instance_id.as_str()) {
            return;
        }
        if let Some(own) = stored.reported_device_name.filter(|name| !name.trim().is_empty()) {
            seen.name = own;
        }
    }

    /// Remembers what the remembered headphones call themselves, as just
    /// read from them. `None` — not read, or not connected — changes nothing.
    /// Returns whether the name is new.
    pub fn remember_own_name(&self, own: Option<String>) -> bool {
        let Some(own) = own.filter(|name| !name.trim().is_empty()) else {
            return false;
        };
        let stored = self.settings.get();
        if stored.preferred_device_id.is_none()
            || stored.reported_device_name.as_deref() == Some(own.as_str())
        {
            return false;
        }
        self.settings
            .update(|s| {
                s.reported_device_name = Some(own.clone());
                s.preferred_device_name = Some(own.clone());
            })
            .is_ok()
    }

    /// The remembered headphones' instance id, if they are still paired.
    #[cfg(not(target_os = "android"))]
    fn preferred_device(&self) -> Option<String> {
        let stored = self.settings.get().preferred_device_id?;
        bose::real::sight_device(Some(&stored)).map(|s| s.instance_id)
    }

    /// The remembered headphones' instance id. Whether they are still paired
    /// is settled when the backend first looks — this runs while the
    /// application is still starting, before Android can be asked anything.
    #[cfg(target_os = "android")]
    fn preferred_device(&self) -> Option<String> {
        self.settings.get().preferred_device_id
    }

    /// The backend to start on.
    ///
    /// Real hardware, if Windows already knows a Bose device — the headphones
    /// are paired with this PC, and looking at them is what the application
    /// is for. The simulated device otherwise, so there is something to
    /// explore, labelled as simulated. Deciding this only enumerates what
    /// Windows has; nothing is sent to any device.
    pub fn initial_source() -> DeviceSource {
        // A phone application about the headphones starts on the headphones.
        // With none paired it says so; the simulation is a choice, not what
        // someone holding real headphones should be met with.
        #[cfg(target_os = "android")]
        return DeviceSource::Real;

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
        #[cfg(not(target_os = "android"))]
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
    pub desktop_widget: bool,
    pub widget_on_top: bool,
    /// Switches that cannot be used in this build, each with the reason.
    pub unavailable: BTreeMap<SettingKey, String>,
}

/// Emitted to every window when the settings change, with the settings as
/// they then stand. There is more than one window now, and a switch flipped
/// in one has to show in the other.
pub const SETTINGS_CHANGED_EVENT: &str = "settings-changed";

impl SettingsView {
    fn build(stored: &StoredSettings) -> Self {
        let mut unavailable = BTreeMap::new();
        let autostart_blocked = autostart::unavailable_reason();
        if let Some(why) = autostart_blocked {
            unavailable.insert(SettingKey::StartWithWindows, why.to_string());
        }
        let start_with_windows = autostart_blocked.is_none()
            && autostart::Autostart::for_this_app().is_ok_and(|entry| entry.is_enabled());
        // A phone has no minimized state to start in, and no desktop to
        // put a widget on.
        #[cfg(mobile)]
        for key in [
            SettingKey::StartMinimized,
            SettingKey::DesktopWidget,
            SettingKey::WidgetOnTop,
        ] {
            unavailable.insert(key, "Not available on Android.".to_string());
        }

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
            desktop_widget: stored.desktop_widget,
            widget_on_top: stored.widget_on_top,
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
            device::DeviceError::NotPermitted(_) => "not-permitted",
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

/// Attaches what the host system knows to a device snapshot: its volume,
/// and its own name.
///
/// Audio is a system concern rather than a device-backend one: the endpoint
/// exists whether we are talking to a mock or to real headphones. Keeping it
/// out of the backends means the mock cannot fabricate a Windows volume, which
/// would be a particularly misleading thing to simulate.
///
/// Reading an endpoint successfully is genuine hardware evidence that the
/// volume path works, so the capability is promoted accordingly.
fn attach_system(mut snapshot: DeviceSnapshot) -> DeviceSnapshot {
    snapshot.host_name = host::name();

    // Android has one volume that matters here: the one media plays at,
    // which is the headphones' while they are the media output.
    #[cfg(target_os = "android")]
    {
        match android::sound::media_volume() {
            Ok(volume) if volume.steps > 0 => {
                let output = volume
                    .output
                    .clone()
                    .unwrap_or_else(|| "Media".to_string());
                let proof = HardwareProof::observed_passively(
                    CapabilityKey::Volume,
                    format!("read Android media volume, playing through {output:?}"),
                );
                let _ = snapshot
                    .capabilities
                    .get_mut(CapabilityKey::Volume)
                    .verify_with_hardware(&proof, Mechanism::AndroidAudio);

                snapshot.system_audio = Some(SystemAudioState {
                    endpoint_id: util::stable_id("android-media-volume"),
                    endpoint_name: output,
                    volume_percent: volume.percent(),
                    muted: volume.muted,
                    is_default_render: true,
                    is_default_communications: false,
                    sample_rate_hz: None,
                    channels: None,
                });
            }
            Ok(_) => {
                let _ = snapshot
                    .capabilities
                    .get_mut(CapabilityKey::Volume)
                    .mark_unsupported("Android reports a media volume with no steps.");
            }
            Err(e) => {
                let _ = snapshot
                    .capabilities
                    .get_mut(CapabilityKey::Volume)
                    .mark_unsupported(format!("Android's media volume could not be read: {e}"));
            }
        }
    }

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

                snapshot.system_audio = Some(SystemAudioState {
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
#[cfg(windows)]
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
    // The custom modes the headphones listed, so that saying one's name does
    // not have to wait for it to be synthesized the first time.
    state.voice().expect_modes(&dev.known_custom_modes());
    // What the headphones call themselves, kept for the times the name is
    // needed and they have not yet been asked: a connection being announced.
    if dev.source() == DeviceSource::Real {
        state.remember_own_name(dev.reported_name());
    }
    let snapshot = attach_system(snapshot);
    // And what this machine is called, for announcing a connection to it.
    if let Some(host) = &snapshot.host_name {
        state.voice().set_host(Some(host.clone()));
    }
    Ok(snapshot)
}

/// Hands the voice service the mode the headphones themselves last reported.
///
/// Called straight after a change was asked for. What it passes on is the
/// headphones' own read-back, never the request: if they did not change, or
/// did not answer, there is nothing new here and nothing is said. Without
/// this the announcement waits for the next full refresh, which can be most
/// of a second behind.
pub(crate) fn observe_reported_mode(state: &AppState, device: &dyn BoseDevice) {
    if device.source() != DeviceSource::Real {
        return;
    }
    if let Some(reported) = device.reported_noise_control() {
        state
            .voice()
            .observe_mode(voice::policy::SpokenMode::from_state(&reported));
    }
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
    Ok(attach_system(snapshot))
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
#[cfg_attr(desktop, tauri::command)]
#[cfg_attr(mobile, tauri::command(async))]
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
            #[cfg(desktop)]
            tray::sync(&app, value).map_err(platform_error)?;
            // On Android the same switch means "keep running in the
            // background": the service that keeps the process alive while the
            // application is not on screen. That first, for the same reason.
            #[cfg(target_os = "android")]
            android::bridge::keep_alive(value).map_err(platform_error)?;
            let _ = &app;
            state
                .settings()
                .update(|s| s.set(key, value))
                .map_err(platform_error)?;
        }
        SettingKey::DesktopWidget | SettingKey::WidgetOnTop => {
            #[cfg(mobile)]
            return Err(UiError {
                message: "There is no desktop widget on Android.".to_string(),
                kind: "unsupported".to_string(),
            });
            #[cfg(desktop)]
            {
                state
                    .settings()
                    .update(|s| s.set(key, value))
                    .map_err(platform_error)?;
                // The window follows the switch.
                match key {
                    SettingKey::DesktopWidget => widget::sync(&app, value),
                    _ => widget::set_on_top(&app, value),
                }
            }
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
    let view = SettingsView::build(&after);
    let _ = app.emit(SETTINGS_CHANGED_EVENT, &view);
    Ok(view)
}

/// Brings the main window forward. For the widget, whose whole point is that
/// the main window is usually not open. Takes nothing: it can show this
/// application's own window and nothing else.
#[tauri::command]
fn show_main_window(app: tauri::AppHandle) {
    #[cfg(desktop)]
    tray::show_main(&app);
    let _ = &app;
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
    app: tauri::AppHandle,
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
    let view = SettingsView::build(&after);
    let _ = app.emit(SETTINGS_CHANGED_EVENT, &view);
    Ok(view)
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
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    command: DeviceCommand,
) -> UiResult<CommandOutcome> {
    command.validate()?;

    // Whatever comes of it, the device may now be in another state, and a
    // second window — the widget, or the main window behind it — is showing
    // the old one. The window that asked re-reads by itself.
    let changes_something = !matches!(
        command,
        DeviceCommand::RefreshSnapshot
            | DeviceCommand::ReadBattery
            | DeviceCommand::ReadNoiseControl
            | DeviceCommand::ReadEqualizer
            | DeviceCommand::ReadDeviceInfo
    );
    let outcome = run_command(&state, command).await;
    if changes_something {
        let _ = app.emit(watcher::DEVICE_CHANGED_EVENT, ());
        // The home-screen widget shows the mode too.
        #[cfg(target_os = "android")]
        android::widget::mode_changed(&app);
    }
    outcome
}

async fn run_command(state: &AppState, command: DeviceCommand) -> UiResult<CommandOutcome> {
    let dev = state.current();
    let outcome = match command {
        DeviceCommand::Connect => dev.connect().await?,
        DeviceCommand::Disconnect => dev.disconnect().await?,
        DeviceCommand::Reconnect => {
            let _ = dev.disconnect().await;
            dev.connect().await?
        }
        DeviceCommand::SetNoiseControl { mode } => {
            let outcome = dev.set_noise_control(mode).await;
            observe_reported_mode(state, dev.as_ref());
            outcome?
        }
        DeviceCommand::SetEqualizer { settings } => dev.set_equalizer(settings).await?,
        DeviceCommand::SelectNoiseMode { index } => {
            let outcome = dev.select_noise_mode(index).await;
            observe_reported_mode(state, dev.as_ref());
            outcome?
        }
        DeviceCommand::SetMultipoint { enabled } => dev.set_multipoint(enabled).await?,
        DeviceCommand::SetPairedDeviceConnection { device, connected } => {
            dev.set_paired_connection(&device, connected).await?
        }
        DeviceCommand::SetDeviceName { name } => {
            let outcome = dev.set_device_name(&name).await;
            // Whatever they now call themselves is what they go by here,
            // from this moment: the system will take longer to catch up.
            if dev.source() == DeviceSource::Real {
                state.remember_own_name(dev.reported_name());
            }
            outcome?
        }

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
            // Android's media volume moves in steps, so the nearest step is
            // what is set, and success is Android then reporting that step.
            #[cfg(target_os = "android")]
            {
                let (asked, after) =
                    android::sound::set_media_volume(percent).map_err(platform_error)?;
                if after.level == asked {
                    CommandOutcome::applied()
                } else {
                    CommandOutcome::sent_unverified(format!(
                        "The phone reports {}% after requesting {percent}%.",
                        after.percent()
                    ))
                }
            }
            #[cfg(not(any(windows, target_os = "android")))]
            {
                let _ = percent;
                CommandOutcome::unsupported(
                    "System volume is only implemented for Windows and Android.",
                )
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
            #[cfg(target_os = "android")]
            {
                let after = android::sound::set_media_mute(muted).map_err(platform_error)?;
                if after.muted == muted {
                    CommandOutcome::applied()
                } else {
                    CommandOutcome::sent_unverified(
                        "The phone did not report the requested mute state.",
                    )
                }
            }
            #[cfg(not(any(windows, target_os = "android")))]
            {
                let _ = muted;
                CommandOutcome::unsupported(
                    "System volume is only implemented for Windows and Android.",
                )
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
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    mode: NoiseModeRequest,
) -> UiResult<CommandOutcome> {
    let device = state.current();
    let outcome = device.set_noise_control(mode.into()).await;
    // Whatever came of it, what the headphones now report is what is said.
    observe_reported_mode(&state, device.as_ref());
    // And what any other window should now show.
    let _ = app.emit(watcher::DEVICE_CHANGED_EVENT, ());
    #[cfg(target_os = "android")]
    android::widget::mode_changed(&app);
    Ok(outcome?)
}

/// Opens the Windows Bluetooth settings page.
///
/// The target URI is fixed in native code and takes no parameter, so the
/// frontend cannot use this to open an arbitrary URL or launch a program.
/// That is why this exists as a dedicated command rather than granting the
/// opener plugin's general `open-url` permission to the UI.
#[cfg_attr(desktop, tauri::command)]
#[cfg_attr(mobile, tauri::command(async))]
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
    #[cfg(target_os = "android")]
    {
        let _ = app;
        android::bridge::open_bluetooth_settings().map_err(platform_error)
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        let _ = app;
        Err(UiError {
            message: "Bluetooth settings are only available on Windows and Android.".to_string(),
            kind: "unsupported".to_string(),
        })
    }
}

/// Asks Android for permission to see Bluetooth devices, again.
///
/// The application asks once when it starts. This is for someone who said no
/// and has thought better of it. Takes nothing and returns nothing: whether
/// it worked shows in the next snapshot.
#[cfg_attr(desktop, tauri::command)]
#[cfg_attr(mobile, tauri::command(async))]
fn request_bluetooth_permission() -> UiResult<()> {
    #[cfg(target_os = "android")]
    {
        android::bridge::ask_permissions().map_err(platform_error)
    }
    #[cfg(not(target_os = "android"))]
    {
        Err(UiError {
            message: "There is no Bluetooth permission to ask for on this system.".to_string(),
            kind: "unsupported".to_string(),
        })
    }
}

#[cfg_attr(desktop, tauri::command)]
#[cfg_attr(mobile, tauri::command(async))]
fn get_bluetooth_availability() -> bluetooth::BluetoothAvailability {
    #[cfg(windows)]
    {
        bluetooth::radio::availability()
    }
    #[cfg(target_os = "android")]
    {
        match android::directory::report() {
            Ok(report) => bluetooth::BluetoothAvailability {
                radio_present: report.radio_present,
                radio_enabled: report.radio_enabled,
                detail: if !report.permission {
                    "This application has not been allowed to use Bluetooth.".to_string()
                } else if report.radio_enabled {
                    "Bluetooth is on.".to_string()
                } else {
                    "Bluetooth is switched off.".to_string()
                },
            },
            Err(why) => bluetooth::BluetoothAvailability::unavailable(why),
        }
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        bluetooth::BluetoothAvailability::unavailable("Not running on Windows or Android.")
    }
}

/// The Bose headphones and earbuds paired with this machine, with the model
/// each is, to choose from. Only enumerates what the system knows; nothing is
/// sent to any device.
#[cfg_attr(desktop, tauri::command)]
#[cfg_attr(mobile, tauri::command(async))]
fn list_bose_devices(state: tauri::State<'_, AppState>) -> Vec<bose::choice::PairedBose> {
    let current = state.settings().get().preferred_device_id;
    let mut listed = bose::choice::choices(bose::choice::paired(), current.as_deref());
    // The current device goes by the name it gave itself, where it has.
    if let Some(own) = state.settings().get().reported_device_name {
        for device in listed.iter_mut().filter(|d| d.current && !own.trim().is_empty()) {
            device.name = own.clone();
        }
    }
    listed
}

/// Makes the paired device listed under `id` the one this application is
/// about, and attaches to it: an explicit choice by the user.
#[tauri::command]
async fn use_bose_device(state: tauri::State<'_, AppState>, id: String) -> UiResult<DeviceSnapshot> {
    if !device::state::is_paired_device_id(&id) {
        return Err(UiError {
            message: "That is not a device this application listed.".to_string(),
            kind: "invalid-input".to_string(),
        });
    }
    let seen = bose::choice::sighting_for(bose::choice::paired(), &id).ok_or_else(|| UiError {
        message: "That device is no longer paired with this computer.".to_string(),
        kind: "not-connected".to_string(),
    })?;
    state.remember_device(&seen);
    state.attach();
    state.switch_to(DeviceSource::Real);
    let dev = state.current();
    let snapshot = dev.snapshot().await?;
    state.voice().observe_snapshot(&snapshot);
    Ok(attach_system(snapshot))
}

/// Read-only enumeration of Bluetooth devices Windows knows about.
#[cfg_attr(desktop, tauri::command)]
#[cfg_attr(mobile, tauri::command(async))]
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
    #[cfg(target_os = "android")]
    {
        use bluetooth::{DiscoveredDevice, DiscoveredTransport};
        android::directory::report()
            .map(|report| report.devices)
            .unwrap_or_default()
            .into_iter()
            .map(|d| DiscoveredDevice {
                id: util::stable_id(&d.instance_id),
                looks_like_bose: d.advertises_bmap || bose::looks_like_bose(&d.name),
                name: d.name,
                transport: if d.low_energy_only {
                    DiscoveredTransport::LowEnergy
                } else {
                    DiscoveredTransport::Classic
                },
                connected: Some(d.connected),
                battery_percent: d.battery,
            })
            .collect()
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        Vec::new()
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    util::set_salt(util::generate_salt());

    let builder = tauri::Builder::default();

    // One instance only: two would fight over the headphones' single
    // control connection. Starting the application again brings the
    // running one forward — including out of the tray. Registered first,
    // as the plugin requires.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        tray::show_main(app);
    }));

    // Android's Bluetooth, audio and background service, for this crate's
    // own use. The web view is given no permission to call it.
    #[cfg(target_os = "android")]
    let builder = builder.plugin(android::bridge::init());

    builder
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
            let backend = voice::backend::KokoroBackend::new(voice_root, Arc::clone(&store));
            // An installer that carries the model puts it here.
            #[cfg(not(target_os = "android"))]
            let backend =
                backend.with_bundled(app.path().resource_dir().ok().map(|dir| dir.join("voice")));
            // An Android package carries it packed, and it is unpacked the
            // first time it is wanted.
            #[cfg(target_os = "android")]
            let backend = backend.with_bundled_lookup(|| {
                android::bridge::bundled_model_root()
                    .map(|root| root.map(std::path::PathBuf::from))
            });
            // Sentences already synthesized are kept, so that the model —
            // most of what this application weighs once loaded — is needed
            // only for a sentence that has not been said before. The cache
            // folder is the place for it: everything there can be made again.
            let spoken = app
                .path()
                .app_cache_dir()
                .or_else(|_| app.path().app_local_data_dir())
                .ok()
                .map(|dir| voice::store::SpeechStore::new(dir.join("spoken")));
            let voice = VoiceFeedback::start_with_store(
                Arc::clone(&store),
                Box::new(backend),
                voice::Timing::standard(),
                spoken,
                move |status| {
                    let _ = status_target.emit(voice::VOICE_STATUS_EVENT, status);
                },
            );
            // What this machine is called, for "connected to …". Asked off
            // the start-up path: on Android the answer comes from Kotlin.
            {
                let voice = voice.clone();
                std::thread::spawn(move || voice.set_host(host::name()));
            }

            // Start on real hardware when the headphones are already paired
            // with this PC, and on the labelled simulation otherwise.
            app.manage(AppState::new(AppState::initial_source(), store, voice));

            #[cfg(desktop)]
            {
                if settings.minimize_to_tray {
                    if let Err(e) = tray::sync(&handle, true) {
                        eprintln!("tray icon unavailable: {e}");
                    }
                }

                // The widget, if it was left on. It is a window of its own and
                // is put back where it was.
                if settings.desktop_widget {
                    widget::open(&handle);
                }

                // The window is created hidden so that "start minimized" never
                // flashes it on screen. It stays hidden only if there is
                // something to bring it back — the tray icon, or the widget —
                // and otherwise it is shown, minimized or not.
                let stay_hidden = settings.start_minimized
                    && (tray::is_present(&handle) || settings.desktop_widget);
                if !stay_hidden {
                    if let Some(window) = app.get_webview_window(tray::MAIN_WINDOW) {
                        let _ = window.show();
                        if settings.start_minimized {
                            let _ = window.minimize();
                        }
                    }
                }
            }

            // "Keep running in the background", if it was left on. Off the
            // start-up path: nothing here should wait on Android.
            #[cfg(target_os = "android")]
            if settings.minimize_to_tray {
                std::thread::spawn(|| {
                    let _ = android::bridge::keep_alive(true);
                });
            }
            #[cfg(mobile)]
            let _ = &settings;

            // The home-screen widget's requests, from here on.
            #[cfg(target_os = "android")]
            android::widget::spawn(handle.clone());

            watcher::spawn(handle);
            Ok(())
        })
        .on_window_event(|window, event| {
            // Hiding to the tray is a desktop notion.
            #[cfg(mobile)]
            let _ = (window, event);
            #[cfg(desktop)]
            {
            let app = window.app_handle();
            if window.label() == widget::WIDGET_WINDOW {
                match event {
                    tauri::WindowEvent::Moved(position) => {
                        widget::moved(app, position.x, position.y);
                    }
                    // Closed from the keyboard rather than by its own button:
                    // the same thing, and the switch has to say so.
                    tauri::WindowEvent::CloseRequested { .. } => widget::closed_by_user(app),
                    _ => {}
                }
                return;
            }
            if window.label() != tray::MAIN_WINDOW {
                return;
            }
            let Some(state) = app.try_state::<AppState>() else {
                return;
            };
            // Hiding to the tray: only while the setting is on *and* the
            // icon exists.
            let to_tray = state.settings().get().minimize_to_tray && tray::is_present(app);
            match event {
                // With the widget up, closing the main window leaves the
                // widget running — which is what the widget is for — and the
                // widget can bring the window back. With neither a tray icon
                // nor a widget, closing the window closes the application.
                tauri::WindowEvent::CloseRequested { api, .. }
                    if to_tray || widget::is_open(app) =>
                {
                    api.prevent_close();
                    let _ = window.hide();
                }
                tauri::WindowEvent::Resized(_) if to_tray => {
                    if window.is_minimized().unwrap_or(false) {
                        let _ = window.hide();
                    }
                }
                _ => {}
            }
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
            list_bose_devices,
            use_bose_device,
            list_audio_endpoints,
            open_bluetooth_settings,
            request_bluetooth_permission,
            show_main_window,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Void")
        .run(|app, event| {
            // Stop speaking and release the model before the process goes.
            if let tauri::RunEvent::Exit = event {
                #[cfg(target_os = "android")]
                android::widget::stop();
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

    fn sighting(id: &str, name: &str) -> bose::real::Sighting {
        bose::real::Sighting {
            instance_id: id.into(),
            name: name.into(),
            connected: true,
            battery: None,
        }
    }

    /// After a rename the system goes on calling the headphones what it
    /// called them. What they said themselves is the name that is used.
    #[test]
    fn renamed_headphones_go_by_the_name_they_gave_not_the_systems() {
        let store = Arc::new(SettingsStore::in_memory(StoredSettings::default()));
        let state = state(DeviceSource::Mock, store);
        state.remember_device(&sighting("DEV_A", "Aurora"));

        // Nothing read from them yet: the system's name stands.
        let mut seen = sighting("DEV_A", "Aurora");
        state.go_by_own_name(&mut seen);
        assert_eq!(seen.name, "Aurora");

        // They report a new name; the system has not caught up.
        assert!(state.remember_own_name(Some("Nova".into())));
        assert!(!state.remember_own_name(Some("Nova".into())), "nothing new the second time");
        let mut seen = sighting("DEV_A", "Aurora");
        state.go_by_own_name(&mut seen);
        assert_eq!(seen.name, "Nova");
        state.remember_device(&seen);
        assert_eq!(state.settings().get().preferred_device_name.as_deref(), Some("Nova"));
    }

    #[test]
    fn a_name_that_was_not_read_changes_nothing() {
        let store = Arc::new(SettingsStore::in_memory(StoredSettings::default()));
        let state = state(DeviceSource::Mock, store);
        // No headphones remembered yet: there is nobody for a name to belong to.
        assert!(!state.remember_own_name(Some("Nova".into())));
        state.remember_device(&sighting("DEV_A", "Aurora"));
        assert!(!state.remember_own_name(None));
        assert!(!state.remember_own_name(Some("   ".into())));
        assert_eq!(state.settings().get().reported_device_name, None);
    }

    /// One pair's name is not carried over to another pair.
    #[test]
    fn a_different_pair_of_headphones_does_not_inherit_the_name() {
        let store = Arc::new(SettingsStore::in_memory(StoredSettings::default()));
        let state = state(DeviceSource::Mock, store);
        state.remember_device(&sighting("DEV_A", "Aurora"));
        state.remember_own_name(Some("Nova".into()));

        let mut other = sighting("DEV_B", "Bose QC Ultra");
        state.go_by_own_name(&mut other);
        assert_eq!(other.name, "Bose QC Ultra");
        state.remember_device(&other);
        let stored = state.settings().get();
        assert_eq!(stored.reported_device_name, None);
        assert_eq!(stored.preferred_device_name.as_deref(), Some("Bose QC Ultra"));
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
