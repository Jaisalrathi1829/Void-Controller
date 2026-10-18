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
  /**
   * Each battery's own level on a device with several — earbuds and their
   * case — as the device reported them. Absent otherwise.
   */
  parts?: BatteryPart[];
}

/** One battery of a device that has several: an earbud, or the case. */
export interface BatteryPart {
  /** What the model's description calls it: "Left", "Right", "Case". */
  name: string;
  percent: number;
}

/** An action a touch shortcut can be set to. */
export interface ShortcutOption {
  action: number;
  name: string;
}

/** An earbud's touch shortcut, as the earbuds last reported it. */
export interface ShortcutState {
  /** Which control: "Shortcut", "Left earbud", ... */
  control: string;
  /** "Press and hold", ... */
  gesture: string;
  action: number;
  actionName: string;
  /** The actions the earbuds list for it. Empty: shown, not changed. */
  options: ShortcutOption[];
  readAt: string;
}

/** What earbuds do when taken out or put in. Null where they have no such setting. */
export interface WearState {
  autoPause: boolean | null;
  autoAnswer: boolean | null;
}

/** "Find my buds": when, and on a phone where, the earbuds were last connected. */
export interface LastSeen {
  deviceName: string;
  at: string;
  place: {
    latitudeE6: number;
    longitudeE6: number;
    accuracyM: number | null;
    at: string;
  } | null;
}

/** What kind of thing a Bose model is. */
export type DeviceCategory = 'headphones' | 'earbuds';

/**
 * How much is known about a model's controls.
 *
 * - `tested`      Run against the physical model by this project.
 * - `documented`  Its protocol is described by people who have one; never
 *                 run here.
 * - `recognized`  Known to be a Bose model of its kind, with nothing written
 *                 down about its controls. Only what it answers is offered,
 *                 and nothing is written into its modes.
 */
export type ModelSupport = 'tested' | 'documented' | 'recognized';

/** Which model a device is, from Bose's catalogue. */
export interface DeviceModel {
  /** Bose's name for the model. */
  name: string;
  category: DeviceCategory;
  support: ModelSupport;
}

/** One of the noise modes the device offers, in its own order. */
export interface NoiseModeOption {
  /** The slot the device lists it under: what switching to it asks for. */
  index: number;
  /** The device's own name for it. */
  name: string;
  /** A fixed mode, rather than one the user set up. */
  preset: boolean;
  /** Its noise-cancellation level can be changed from here. */
  adjustable: boolean;
  /** The mode the device last said it is in. */
  active: boolean;
}

/**
 * A Bose device paired with this phone or computer, to choose from. By name
 * and model only — no address.
 */
export interface PairedBose {
  /** What choosing it asks for. A salted hash, never the address. */
  id: string;
  /** The name its owner gave it. */
  name: string;
  /** Null for a Bose device the catalogue does not have. */
  model: DeviceModel | null;
  connected: boolean;
  /** The one this application is looking at now. */
  current: boolean;
}

/** What Void says about how far a model has been tried. */
export function supportNote(support: ModelSupport): string {
  switch (support) {
    case 'tested':
      return 'Tested with Void.';
    case 'documented':
      return 'Not yet tested with Void. Its controls follow the published description of this model, and every change is checked by reading it back.';
    case 'recognized':
      return 'Not yet tested with Void. Only what the device answers about itself is shown, and nothing is written into its modes.';
  }
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
  /** Which model the device is. Null or absent when it cannot be told. */
  model?: DeviceModel | null;
  /**
   * The noise modes the device offers, with the one it is in marked. Null or
   * absent until it has said which mode it is in.
   */
  noiseModes?: NoiseModeOption[] | null;
  /** Whether multipoint is on, as the headphones last said. */
  multipoint?: MultipointState | null;
  /** The touch shortcut, on earbuds. */
  shortcut?: ShortcutState | null;
  /** In-ear detection settings, on earbuds. */
  wear?: WearState | null;
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
