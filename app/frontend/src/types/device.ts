import type { DeviceCapabilities } from './capability';

/**
 * Which implementation is backing the current session.
 *
 * This is surfaced prominently in the UI. When `mock`, every value shown is
 * fabricated for development purposes and must be labelled SIMULATED.
 */
export type DeviceSource = 'mock' | 'real';

export type ConnectionState =
  | 'disconnected'
  | 'discovering'
  | 'connecting'
  | 'connected'
  | 'reconnecting'
  | 'error';

/** How we are talking to the device, if at all. */
export type Transport =
  | 'none'
  | 'bluetooth-classic'
  | 'bluetooth-le'
  | 'windows-audio-endpoint'
  | 'simulated';

export interface DeviceIdentity {
  /** Friendly name as reported by Windows. */
  name: string;
  /**
   * Stable opaque identifier. This is a hash of the Bluetooth address rather
   * than the address itself, so that diagnostics reports can be shared without
   * leaking a permanent hardware identifier. The raw address stays local.
   */
  id: string;
  /** Windows PnP instance id, present only for real devices. */
  instanceId?: string;
  manufacturer?: string;
  modelNumber?: string;
  firmwareVersion?: string;
  serialNumber?: string;
}

/**
 * A battery reading, with explicit provenance.
 *
 * `source` matters: a level read from the Windows PnP property is a different
 * claim from one parsed out of a vendor protocol frame, and the UI says which.
 */
export interface BatteryReading {
  /** 0-100. */
  percent: number;
  source:
    | 'windows-pnp'
    | 'android-bluetooth'
    | 'ble-battery-service'
    | 'vendor-protocol'
    | 'simulated';
  /** Some devices report per-earcup or case levels. Absent unless observed. */
  charging?: boolean;
  readAt: string;
}

export type NoiseControlMode = 'quiet' | 'aware' | 'custom' | 'off';

export interface NoiseControlState {
  mode: NoiseControlMode;
  /**
   * Noise-cancellation strength of the active mode, 0-10 where 10 is the
   * most. Absent unless the device reported one.
   */
  level?: number;
  /** The device's own name for the active mode — a custom mode's name. */
  label?: string;
  source: 'vendor-protocol' | 'simulated';
  readAt: string;
}

export interface EqSettings {
  bass: number;
  mid: number;
  treble: number;
}

export interface EqState extends EqSettings {
  source: 'vendor-protocol' | 'software-dsp' | 'simulated';
  readAt: string;
}

/** Whether the headphones will stay connected to two devices at once. */
export interface MultipointState {
  enabled: boolean;
  source: 'vendor-protocol' | 'simulated';
  readAt: string;
}

/**
 * A device the headphones are paired with: a phone, a computer. By name only
 * — the address the headphones know it by never leaves the native layer.
 */
export interface PairedDevice {
  /**
   * Stands for the device when it is to be connected or disconnected. Not its
   * address: the native layer matches it against the headphones' own list.
   */
  id: string;
  /** What the headphones call it. Empty if they have no name for it. */
  name: string;
  connected: boolean;
  /** The phone or computer this application is running on. */
  thisDevice: boolean;
}

/** The longest name the headphones are given, in bytes of UTF-8. */
export const MAX_DEVICE_NAME_BYTES = 31;

/**
 * A name as it may be given to the headphones: what is left after trimming,
 * or why not. The native layer applies the same rule, and its answer is the
 * one that counts; this is so the reason can be shown while typing.
 */
export function checkDeviceName(
  name: string,
): { ok: true; name: string } | { ok: false; reason: string } {
  const trimmed = name.trim();
  if (trimmed.length === 0) return { ok: false, reason: 'A name cannot be empty.' };
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f-\u009f]/.test(trimmed)) {
    return { ok: false, reason: 'A name cannot contain line breaks or control characters.' };
  }
  const bytes = new TextEncoder().encode(trimmed).length;
  if (bytes > MAX_DEVICE_NAME_BYTES) {
    return {
      ok: false,
      reason: `That name is too long: ${bytes} bytes, and the most is ${MAX_DEVICE_NAME_BYTES}.`,
    };
  }
  return { ok: true, name: trimmed };
}

/**
 * The host system's own volume — a Windows audio endpoint, or Android's media
 * volume. Distinct from any Bose-internal volume.
 */
export interface SystemAudioState {
  /** The Windows endpoint, or on Android what media is playing through. */
  endpointName: string;
  endpointId: string;
  /** 0-100. On Android, the media volume as a share of its steps. */
  volumePercent: number;
  muted: boolean;
  isDefaultRender: boolean;
  isDefaultCommunications: boolean;
  /** Present when the endpoint exposes a mix format. */
  sampleRateHz?: number;
  channels?: number;
}

export interface DeviceSnapshot {
  source: DeviceSource;
  connection: ConnectionState;
  transport: Transport;
  identity: DeviceIdentity | null;
  capabilities: DeviceCapabilities;

  /**
   * All of the following are null unless the corresponding capability has
   * produced a real reading. They are never defaulted to plausible-looking
   * values.
   */
  battery: BatteryReading | null;
  noiseControl: NoiseControlState | null;
  equalizer: EqState | null;
  /** Whether multipoint is on, as the headphones last said. */
  multipoint?: MultipointState | null;
  /** What the headphones are paired with, and which of those are connected. */
  pairedDevices?: PairedDevice[] | null;
  systemAudio: SystemAudioState | null;
  /**
   * What the phone or computer running this application is called — the
   * thing the headphones are connected to when they are connected here.
   */
  hostName?: string | null;

  /** Populated when `connection === 'error'`. */
  lastError: string | null;
  updatedAt: string;
}

/**
 * The result of issuing a command to a device.
 *
 * `applied` means we observed the device's state actually change to the
 * requested value. `sent-unverified` means the command was transmitted but the
 * device gave us no evidence of the outcome — the UI must say so rather than
 * claiming success.
 */
export type CommandOutcome =
  | { kind: 'applied'; verifiedAt: string }
  | { kind: 'sent-unverified'; reason: string }
  | { kind: 'rejected'; reason: string }
  | { kind: 'unsupported'; reason: string };

export function isSuccessfullyApplied(outcome: CommandOutcome): boolean {
  return outcome.kind === 'applied';
}

/** Human-readable text for a command outcome. Never overstates the result. */
export function describeOutcome(outcome: CommandOutcome): string {
  switch (outcome.kind) {
    case 'applied':
      return 'Applied and confirmed by the device.';
    case 'sent-unverified':
      return `Command sent. State could not be verified. ${outcome.reason}`.trim();
    case 'rejected':
      return `Device rejected the command. ${outcome.reason}`.trim();
    case 'unsupported':
      return `Not supported on this device. ${outcome.reason}`.trim();
  }
}
