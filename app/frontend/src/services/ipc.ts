/**
 * The only channel between the UI and the native layer.
 *
 * Note what this module does *not* export: there is no `write(uuid, bytes)`,
 * no characteristic handle, no raw payload. The native side exposes a fixed
 * set of typed commands and nothing else, so the UI is structurally incapable
 * of issuing an arbitrary Bluetooth write.
 *
 * When the app is opened in a plain browser (`npm run dev` without Tauri), the
 * native layer is absent. Rather than showing a broken page, we fall back to a
 * browser-side simulation — which is labelled SIMULATED exactly like the
 * native mock, because it is one.
 */

import type {
  DeviceSnapshot,
  DeviceSource,
  CommandOutcome,
  NoiseControlState,
} from '@/types/device';
import type { DiscoveredDevice, BluetoothAvailability } from '@/types/bluetooth';
import { BrowserSimulatedDevice } from '@/mock/browserDevice';

export type DeviceCommand =
  | { kind: 'refreshSnapshot' }
  | { kind: 'readBattery' }
  | { kind: 'readNoiseControl' }
  | { kind: 'readEqualizer' }
  | { kind: 'readDeviceInfo' }
  | { kind: 'connect' }
  | { kind: 'disconnect' }
  | { kind: 'reconnect' }
  | { kind: 'setSystemVolume'; percent: number }
  | { kind: 'setSystemMute'; muted: boolean }
  | { kind: 'mediaPlayPause' }
  | { kind: 'mediaNext' }
  | { kind: 'mediaPrevious' }
  | { kind: 'setNoiseControl'; mode: 'quiet' | 'aware' | 'custom' | 'off' }
  | { kind: 'setNoiseControlLevel'; level: number }
  | { kind: 'setEqualizer'; settings: { bass: number; mid: number; treble: number } };

export interface IpcError {
  message: string;
  kind: string;
}

/** True when running inside the Tauri shell with a real native layer. */
export function hasNativeLayer(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/**
 * Where the current session's data comes from. Surfaced in the UI so the
 * distinction between "native mock", "browser preview" and "real hardware" is
 * never ambiguous.
 */
export type RuntimeMode = 'native' | 'browser-preview';

export function runtimeMode(): RuntimeMode {
  return hasNativeLayer() ? 'native' : 'browser-preview';
}

/**
 * The kind of system the application is running on, as far as the interface
 * needs to know: it changes some wording and which settings make sense, and
 * nothing about what any control does.
 */
export type HostPlatform = 'android' | 'desktop';

export function hostPlatform(): HostPlatform {
  return typeof navigator !== 'undefined' && /Android/i.test(navigator.userAgent)
    ? 'android'
    : 'desktop';
}

const browserFallback = new BrowserSimulatedDevice();

async function invokeNative<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import('@tauri-apps/api/core');
  return invoke<T>(cmd, args);
}

export async function getSnapshot(): Promise<DeviceSnapshot> {
  if (!hasNativeLayer()) return browserFallback.snapshot();
  return invokeNative<DeviceSnapshot>('get_snapshot');
}

export async function getDeviceSource(): Promise<DeviceSource> {
  if (!hasNativeLayer()) return 'mock';
  return invokeNative<DeviceSource>('get_device_source');
}

export async function setDeviceSource(source: DeviceSource): Promise<DeviceSnapshot> {
  if (!hasNativeLayer()) {
    // A browser preview cannot reach hardware. Say so rather than pretending
    // the switch worked.
    if (source === 'real') {
      throw {
        kind: 'unsupported',
        message:
          'Real hardware is only reachable from the desktop application, not the browser preview.',
      } satisfies IpcError;
    }
    return browserFallback.snapshot();
  }
  return invokeNative<DeviceSnapshot>('set_device_source', { source });
}

export async function executeCommand(command: DeviceCommand): Promise<CommandOutcome> {
  if (!hasNativeLayer()) return browserFallback.execute(command);
  return invokeNative<CommandOutcome>('execute_command', { command });
}

/**
 * The noise-control modes that can be requested by name.
 *
 * Narrower than `NoiseControlMode` on purpose: that type has to be able to
 * describe whatever a device reports. This one lists only what has an
 * implementation behind it.
 *
 * `custom` names no slot. Which of the headphones' custom modes it means is
 * decided natively, from the list the headphones themselves report.
 */
export const REQUESTABLE_NOISE_MODES = ['quiet', 'aware', 'custom'] as const;
export type RequestableNoiseMode = (typeof REQUESTABLE_NOISE_MODES)[number];

export function isRequestableNoiseMode(mode: unknown): mode is RequestableNoiseMode {
  return (
    typeof mode === 'string' && (REQUESTABLE_NOISE_MODES as readonly string[]).includes(mode)
  );
}

/**
 * Reads the noise-control mode from the device now.
 *
 * `null` means there is no reading to give — no device, not connected, or not
 * a model that can be asked. It is never a default.
 */
export async function getCurrentMode(): Promise<NoiseControlState | null> {
  if (!hasNativeLayer()) return (await browserFallback.snapshot()).noiseControl;
  return invokeNative<NoiseControlState | null>('get_current_mode');
}

/**
 * Asks the device to switch to Quiet, Aware or its custom mode.
 *
 * The outcome is `applied` only when the device then reported that mode. A
 * request that went out without that confirmation comes back as
 * `sent-unverified`, and callers must not treat it as a success.
 *
 * This takes a mode name and nothing else: there is no way to pass a mode
 * index, a slot number, a frame or bytes through it. The check below repeats
 * at runtime what the type says at compile time, and the native side refuses
 * anything else again on its own.
 */
export async function setNoiseControlMode(mode: RequestableNoiseMode): Promise<CommandOutcome> {
  if (!isRequestableNoiseMode(mode)) {
    throw {
      kind: 'invalid-input',
      message: 'Noise control can only be set to Quiet, Aware or Custom.',
    } satisfies IpcError;
  }
  if (!hasNativeLayer()) return browserFallback.execute({ kind: 'setNoiseControl', mode });
  return invokeNative<CommandOutcome>('set_noise_control_mode', { mode });
}

/**
 * Sets the noise-cancellation strength of the custom mode the device is in:
 * a whole number from 0 to 10, where 10 is the most cancellation.
 *
 * `applied` only when the device then reported that level. On real headphones
 * this changes the level saved in that custom mode.
 */
export async function setNoiseCancellationLevel(level: number): Promise<CommandOutcome> {
  if (!Number.isInteger(level) || level < 0 || level > 10) {
    throw {
      kind: 'invalid-input',
      message: 'The noise-cancellation level must be a whole number from 0 to 10.',
    } satisfies IpcError;
  }
  return executeCommand({ kind: 'setNoiseControlLevel', level });
}

/**
 * The switches on the Settings page. A closed set: the native side accepts
 * these names and nothing else.
 */
export const SETTING_KEYS = [
  'startWithWindows',
  'startMinimized',
  'minimizeToTray',
  'autoConnect',
  'autoReconnect',
  'notifyConnected',
  'notifyDisconnected',
  'notifyLowBattery',
  'voiceEnabled',
  'announceModeChanges',
  'announceConnection',
  'announceBattery',
  'voiceUseDefaultOutput',
] as const;
export type SettingKey = (typeof SETTING_KEYS)[number];

export function isSettingKey(key: unknown): key is SettingKey {
  return typeof key === 'string' && (SETTING_KEYS as readonly string[]).includes(key);
}

export type AppSettings = Record<SettingKey, boolean> & {
  /** Name of the headphones the app attaches to. Never an identifier. */
  preferredDeviceName: string | null;
  /** The voice announcements are spoken in, by the model's name for it. */
  voiceId: string;
  /** Speaking rate; 100 is the model's natural pace. */
  voiceSpeedPercent: number;
  /** Loudness of speech only. Windows' volume is never changed. */
  voiceVolumePercent: number;
  /** Switches that cannot be used in this build, each with the reason. */
  unavailable: Partial<Record<SettingKey, string>>;
};

/** Every switch off and unusable, for the stated reason. */
export function unavailableSettings(reason: string): AppSettings {
  const flags = Object.fromEntries(SETTING_KEYS.map((k) => [k, false])) as Record<
    SettingKey,
    boolean
  >;
  const unavailable = Object.fromEntries(SETTING_KEYS.map((k) => [k, reason])) as Record<
    SettingKey,
    string
  >;
  return {
    ...flags,
    preferredDeviceName: null,
    voiceId: '',
    voiceSpeedPercent: VOICE_SPEED.natural,
    voiceVolumePercent: 0,
    unavailable,
  };
}

// Start-up, the tray and notifications belong to the desktop application. A
// browser tab has none of them, so the switches say so instead of pretending.
const BROWSER_PREVIEW_SETTINGS = 'Not available in the browser preview.';

export async function getSettings(): Promise<AppSettings> {
  if (!hasNativeLayer()) return unavailableSettings(BROWSER_PREVIEW_SETTINGS);
  return invokeNative<AppSettings>('get_settings');
}

/**
 * Flips one switch and returns the settings as they then stand — which is
 * what the native side actually did, not what was asked for.
 */
export async function setSetting(key: SettingKey, value: boolean): Promise<AppSettings> {
  if (!isSettingKey(key) || typeof value !== 'boolean') {
    throw {
      kind: 'invalid-input',
      message: 'Unknown setting.',
    } satisfies IpcError;
  }
  if (!hasNativeLayer()) {
    throw { kind: 'unsupported', message: BROWSER_PREVIEW_SETTINGS } satisfies IpcError;
  }
  return invokeNative<AppSettings>('set_setting', { key, value });
}

/* ---- voice feedback ---- */

/** The speaking rates the native layer accepts, as percentages. */
export const VOICE_SPEED = { min: 50, max: 200, natural: 100 } as const;

export type VoiceState =
  | 'not-installed'
  | 'ready'
  | 'initializing'
  | 'synthesizing'
  | 'playing'
  | 'unavailable'
  | 'error';

export interface VoiceInfo {
  id: string;
  label: string;
}

/** Where voice feedback stands, as the native layer reports it. */
export interface VoiceStatus {
  state: VoiceState;
  /** One sentence saying what the state means right now. */
  detail: string;
  model: string;
  /** Where the voice model is, or is expected to be. */
  assetsDir: string;
  /** The installed model's voices. Empty until it has been found intact. */
  voices: VoiceInfo[];
  /** The sentence being synthesized or spoken, if any. */
  speaking: string | null;
  /** Whether that sentence is a Test Voice request, not a real event. */
  isTest: boolean;
  /** The audio output last spoken through. */
  output: string | null;
  modelLoaded: boolean;
}

// Speech is synthesized and played by the desktop application. A browser tab
// has neither the model nor a way to choose the headphones' output, and it
// does not stand in for them with the browser's own speech.
const BROWSER_PREVIEW_VOICE =
  'Voice feedback runs in the desktop application, not the browser preview.';

/** A status saying voice feedback cannot be used, and why. */
export function voiceUnavailable(detail: string): VoiceStatus {
  return {
    state: 'unavailable',
    detail,
    model: '',
    assetsDir: '',
    voices: [],
    speaking: null,
    isTest: false,
    output: null,
    modelLoaded: false,
  };
}

export async function getVoiceStatus(): Promise<VoiceStatus> {
  if (!hasNativeLayer()) return voiceUnavailable(BROWSER_PREVIEW_VOICE);
  return invokeNative<VoiceStatus>('get_voice_status');
}

/**
 * The voice settings that are not on/off switches: which voice, how fast,
 * how loud. Each is a named choice with a bounded value.
 */
export type VoiceOption =
  | { kind: 'voice'; id: string }
  | { kind: 'speed'; percent: number }
  | { kind: 'volume'; percent: number };

function isVoiceOption(option: VoiceOption): boolean {
  switch (option?.kind) {
    case 'voice':
      return typeof option.id === 'string' && option.id.length > 0;
    case 'speed':
      return (
        Number.isInteger(option.percent) &&
        option.percent >= VOICE_SPEED.min &&
        option.percent <= VOICE_SPEED.max
      );
    case 'volume':
      return Number.isInteger(option.percent) && option.percent >= 0 && option.percent <= 100;
    default:
      return false;
  }
}

/** Changes one voice setting and returns the settings as they then stand. */
export async function setVoiceOption(option: VoiceOption): Promise<AppSettings> {
  if (!isVoiceOption(option)) {
    throw { kind: 'invalid-input', message: 'Not a valid voice setting.' } satisfies IpcError;
  }
  if (!hasNativeLayer()) {
    throw { kind: 'unsupported', message: BROWSER_PREVIEW_VOICE } satisfies IpcError;
  }
  return invokeNative<AppSettings>('set_voice_option', { option });
}

/**
 * Speaks the sample sentence in the chosen voice.
 *
 * Takes no text. What is said is fixed natively, so the UI cannot use this to
 * have arbitrary words spoken.
 */
export async function testVoice(): Promise<void> {
  if (!hasNativeLayer()) {
    throw { kind: 'unsupported', message: BROWSER_PREVIEW_VOICE } satisfies IpcError;
  }
  return invokeNative<void>('test_voice');
}

/** Stops whatever is being said and drops anything waiting to be said. */
export async function stopSpeaking(): Promise<void> {
  if (!hasNativeLayer()) return;
  return invokeNative<void>('stop_speaking');
}

/** Looks again for the voice model, after installing it. */
export async function recheckVoiceModel(): Promise<VoiceStatus> {
  if (!hasNativeLayer()) return voiceUnavailable(BROWSER_PREVIEW_VOICE);
  return invokeNative<VoiceStatus>('recheck_voice_model');
}

/**
 * Calls `handler` whenever the voice status changes. Returns a function that
 * stops listening. A no-op in the browser preview.
 */
export async function onVoiceStatus(handler: (status: VoiceStatus) => void): Promise<() => void> {
  if (!hasNativeLayer()) return () => {};
  const { listen } = await import('@tauri-apps/api/event');
  return listen<VoiceStatus>('voice-status', (event) => handler(event.payload));
}

/**
 * Calls `handler` whenever the native layer sees the headphones connect or
 * disconnect. Returns a function that stops listening. A no-op in the browser
 * preview, where nothing is watching.
 */
export async function onDeviceChanged(handler: () => void): Promise<() => void> {
  if (!hasNativeLayer()) return () => {};
  const { listen } = await import('@tauri-apps/api/event');
  return listen('device-changed', () => handler());
}

export async function getBluetoothAvailability(): Promise<BluetoothAvailability> {
  if (!hasNativeLayer()) {
    return {
      radioPresent: false,
      radioEnabled: false,
      detail: 'Browser preview: no access to Bluetooth hardware.',
    };
  }
  return invokeNative<BluetoothAvailability>('get_bluetooth_availability');
}

export async function listBluetoothDevices(): Promise<DiscoveredDevice[]> {
  if (!hasNativeLayer()) return [];
  return invokeNative<DiscoveredDevice[]>('list_bluetooth_devices');
}

/**
 * Opens the Windows Bluetooth settings page.
 *
 * The URI lives in native code and this call takes no argument, so the UI
 * cannot use it to open anything else.
 */
export async function openBluetoothSettings(): Promise<void> {
  if (!hasNativeLayer()) {
    throw {
      kind: 'unsupported',
      message: 'Windows settings can only be opened from the desktop application.',
    } satisfies IpcError;
  }
  return invokeNative<void>('open_bluetooth_settings');
}

/**
 * Asks Android, again, to let the application see Bluetooth devices.
 *
 * Takes nothing. Whether it worked shows in the next snapshot. Anywhere but
 * Android there is no such permission, and the native layer says so.
 */
export async function requestBluetoothPermission(): Promise<void> {
  if (!hasNativeLayer()) {
    throw {
      kind: 'unsupported',
      message: 'There is no Bluetooth permission to ask for in the browser preview.',
    } satisfies IpcError;
  }
  return invokeNative<void>('request_bluetooth_permission');
}

/** Normalises anything thrown by the IPC layer into a displayable error. */
export function toIpcError(e: unknown): IpcError {
  if (e && typeof e === 'object' && 'message' in e && 'kind' in e) {
    return e as IpcError;
  }
  return {
    kind: 'unknown',
    message: e instanceof Error ? e.message : String(e),
  };
}
