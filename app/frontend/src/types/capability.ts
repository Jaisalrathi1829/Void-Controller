/**
 * The capability model is the core honesty mechanism of this application.
 *
 * Every hardware-facing feature carries a status describing *how much we
 * actually know* about it. The UI renders itself from this model. A control is
 * never shown as functional merely because a component exists to draw it.
 *
 * The rules for transitions are deliberately strict and are enforced in
 * `capabilityTransition()` below, and mirrored in Rust
 * (`app/src-tauri/src/device/capability.rs`) so neither side can drift.
 */

export const CAPABILITY_STATUSES = [
  'unknown',
  'supported',
  'verified',
  'ready',
  'unsupported',
] as const;

export type CapabilityStatus = (typeof CAPABILITY_STATUSES)[number];

/**
 * Precise meanings. These are contractual — do not loosen them.
 *
 * - `unknown`      We have not established whether the feature is accessible.
 * - `supported`    A technically valid API/protocol/interface appears to expose
 *                  the feature, but it has NOT been confirmed on the user's
 *                  physical device.
 * - `verified`     The actual physical device was tested and the feature was
 *                  confirmed to work. Only ever set by a hardware verification
 *                  run — never by a code path that merely sent a command.
 * - `ready`        The device itself answers for the feature and reports its
 *                  state, and every change is checked by reading it back.
 *                  No change has been confirmed yet since the application
 *                  started; the first one that is makes it `verified`.
 * - `unsupported`  The required functionality cannot currently be accessed
 *                  safely through available interfaces.
 */
export const CAPABILITY_MEANING: Record<CapabilityStatus, string> = {
  unknown: 'Not yet established whether this is accessible on this device.',
  supported:
    'A valid interface appears to expose this, but it has not been confirmed on your physical headphones.',
  verified: 'Confirmed working against your physical headphones.',
  ready:
    'Your headphones report this, and every change is checked by reading it back from them.',
  unsupported: 'Cannot currently be accessed safely through available interfaces.',
};

/** Short label, for where there is no room for more. */
export const CAPABILITY_LABEL: Record<CapabilityStatus, string> = {
  unknown: 'UNKNOWN',
  supported: 'SUPPORTED',
  verified: 'VERIFIED',
  ready: 'READY',
  unsupported: 'UNSUPPORTED',
};

/**
 * The set of features tracked. Keep in sync with the Rust `CapabilityKey`.
 */
export const CAPABILITY_KEYS = [
  'battery',
  'volume',
  'playback',
  'noiseControl',
  'awareMode',
  'customNoiseControl',
  'equalizer',
  'multipoint',
  'deviceSettings',
  'firmwareVersion',
  'autoOff',
  'voicePrompts',
  'sidetone',
  'deviceRename',
  'pairedDevices',
] as const;

export type CapabilityKey = (typeof CAPABILITY_KEYS)[number];

/** How a capability is (or would be) reached. */
export type CapabilityMechanism =
  | 'windows-audio'
  | 'android-audio'
  | 'windows-bluetooth'
  | 'windows-pnp'
  | 'windows-media-session'
  | 'ble-gatt-standard'
  | 'ble-gatt-vendor'
  | 'rfcomm-vendor'
  | 'software-dsp'
  | 'none';

export interface CapabilityRecord {
  key: CapabilityKey;
  status: CapabilityStatus;
  /** The interface through which this is (or would be) reached. */
  mechanism: CapabilityMechanism;
  /**
   * Whether this was confirmed against physical hardware. This is tracked
   * separately from `status` so that it is impossible to imply hardware
   * verification through status alone.
   */
  hardwareVerified: boolean;
  /** Human-readable justification for the current status. Always populated. */
  evidence: string;
  /** ISO-8601 timestamp of the last status change, if any. */
  lastEvaluated?: string;
}

export type DeviceCapabilities = Record<CapabilityKey, CapabilityRecord>;

/**
 * A capability is only actionable in the UI when we have real evidence it
 * works. `supported` is deliberately NOT actionable-without-warning: it means
 * "the interface exists" not "it works on your device".
 */
export function isActionable(record: CapabilityRecord): boolean {
  return record.status === 'verified';
}

/** Whether the UI should render a control at all (possibly disabled). */
export function isPresentable(record: CapabilityRecord): boolean {
  return record.status !== 'unsupported';
}

/**
 * Whether a control should carry an "unverified" caveat when used.
 * These states permit interaction but must never claim success blindly.
 */
export function requiresUnverifiedWarning(record: CapabilityRecord): boolean {
  return record.status === 'supported' || record.status === 'ready';
}

export class CapabilityTransitionError extends Error {
  constructor(
    readonly from: CapabilityStatus,
    readonly to: CapabilityStatus,
    reason: string,
  ) {
    super(`Illegal capability transition ${from} -> ${to}: ${reason}`);
    this.name = 'CapabilityTransitionError';
  }
}

export interface TransitionOptions {
  /**
   * Must be true to reach `verified`. Set ONLY by a hardware verification run
   * that observed the device actually behaving as expected.
   */
  hardwareEvidence?: boolean;
}

/**
 * Enforces the transition rules from the project's hardware truth rule:
 *
 *   - `unknown` never silently becomes `supported`; it requires evidence text.
 *   - Nothing becomes `verified` without explicit hardware evidence.
 *   - `unsupported` is terminal for a session; re-evaluating requires a fresh
 *     discovery run, which constructs records from scratch rather than
 *     transitioning them.
 *
 * Returns the new record. Throws `CapabilityTransitionError` on an illegal move.
 */
export function capabilityTransition(
  current: CapabilityRecord,
  to: CapabilityStatus,
  evidence: string,
  options: TransitionOptions = {},
): CapabilityRecord {
  if (!evidence || !evidence.trim()) {
    throw new CapabilityTransitionError(
      current.status,
      to,
      'evidence is required for every transition',
    );
  }

  if (to === 'verified' && options.hardwareEvidence !== true) {
    throw new CapabilityTransitionError(
      current.status,
      to,
      'verified requires hardwareEvidence=true from a physical device test',
    );
  }

  if (current.status === 'unsupported' && to !== 'unsupported') {
    throw new CapabilityTransitionError(
      current.status,
      to,
      'unsupported is terminal within a session; run a fresh discovery instead',
    );
  }

  return {
    ...current,
    status: to,
    hardwareVerified: to === 'verified' ? true : current.hardwareVerified,
    evidence,
    lastEvaluated: new Date().toISOString(),
  };
}

/** Builds a capability set where everything is honestly unknown. */
export function createUnknownCapabilities(
  reason = 'No device has been interrogated yet.',
): DeviceCapabilities {
  const out = {} as DeviceCapabilities;
  for (const key of CAPABILITY_KEYS) {
    out[key] = {
      key,
      status: 'unknown',
      mechanism: 'none',
      hardwareVerified: false,
      evidence: reason,
    };
  }
  return out;
}
