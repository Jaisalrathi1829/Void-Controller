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
  source: 'windows-pnp' | 'ble-battery-service' | 'vendor-protocol' | 'simulated';
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

/** Windows audio endpoint state — distinct from any Bose-internal volume. */
export interface WindowsAudioState {
  endpointName: string;
  endpointId: string;
  /** 0-100, Windows system volume for this endpoint. */
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
  windowsAudio: WindowsAudioState | null;

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
