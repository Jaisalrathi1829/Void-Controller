/**
 * Browser-preview simulation.
 *
 * Used only when the page is opened outside the Tauri shell, so the UI can be
 * developed and tested in a plain browser. It mirrors the Rust mock's most
 * important property: it can never produce a `verified` capability, because a
 * simulation is evidence about the software, not about anyone's headphones.
 *
 * It offers three made-up devices to choose from — a pair of QuietComfort
 * Headphones, a pair of QC Ultra Headphones and a pair of QC Ultra Earbuds —
 * each with the modes and features its model has, so that choosing a device
 * and seeing only its features can be tried without one. Every one of them
 * is labelled simulated.
 */

import {
  checkDeviceName,
  type BatteryPart,
  type CommandOutcome,
  type DeviceModel,
  type DeviceSnapshot,
  type EqSettings,
  type NoiseControlMode,
  type NoiseModeOption,
  type PairedBose,
} from '@/types/device';
import {
  createUnknownCapabilities,
  type CapabilityKey,
  type DeviceCapabilities,
} from '@/types/capability';
import type { DeviceCommand } from '@/services/ipc';

const SIMULATION_NOTE =
  'Simulated in the browser preview. This is evidence about the application, not about any physical device.';

/** A made-up device: what it is called, which model, what it has. */
interface SimulatedProfile {
  id: string;
  name: string;
  model: DeviceModel;
  /** Its modes, in its own order. A mode that is not a preset is custom. */
  modes: { index: number; name: string; preset: boolean }[];
  /** Per-part batteries, for earbuds. */
  parts?: BatteryPart[];
  /** Features this model does not have, and are not shown. */
  lacks: CapabilityKey[];
  /** Noise cancellation as levels (the QuietComfort 35 generation): the
   * indices are the levels' values, and none is Quiet or Aware. */
  levels?: boolean;
}

const PROFILES: [SimulatedProfile, ...SimulatedProfile[]] = [
  {
    id: '00000000000000000a01',
    name: 'Bose QuietComfort',
    model: { name: 'QuietComfort Headphones', category: 'headphones', support: 'tested' },
    modes: [
      { index: 0, name: 'Quiet', preset: true },
      { index: 1, name: 'Aware', preset: true },
      { index: 2, name: 'Simulated custom', preset: false },
    ],
    lacks: [],
  },
  {
    id: '00000000000000000a02',
    name: 'Bose QC Ultra',
    model: {
      name: 'QuietComfort Ultra Headphones (2nd Gen)',
      category: 'headphones',
      support: 'documented',
    },
    modes: [
      { index: 0, name: 'Quiet', preset: true },
      { index: 1, name: 'Aware', preset: true },
      { index: 2, name: 'Immersion', preset: true },
      { index: 3, name: 'Cinema', preset: true },
      { index: 4, name: 'Commute', preset: false },
    ],
    lacks: [],
  },
  {
    id: '00000000000000000a03',
    name: 'Bose QC Ultra Earbuds',
    model: {
      name: 'QuietComfort Ultra Earbuds (2nd Gen)',
      category: 'earbuds',
      support: 'documented',
    },
    modes: [
      { index: 0, name: 'Quiet', preset: true },
      { index: 1, name: 'Aware', preset: true },
      { index: 2, name: 'Immersion', preset: true },
      { index: 3, name: 'Cinema', preset: true },
    ],
    parts: [
      { name: 'Right', percent: 81 },
      { name: 'Left', percent: 76 },
      { name: 'Case', percent: 54 },
    ],
    // No custom mode is set up on them, and they list no paired devices.
    lacks: ['customNoiseControl', 'pairedDevices'],
  },
  {
    id: '00000000000000000a04',
    name: 'Bose QC35 II',
    model: { name: 'QuietComfort 35 II', category: 'headphones', support: 'documented' },
    // Its levels, by the values the headphones use: high 1, low 3, off 0.
    modes: [
      { index: 1, name: 'High', preset: true },
      { index: 3, name: 'Low', preset: true },
      { index: 0, name: 'Off', preset: true },
    ],
    lacks: ['equalizer', 'customNoiseControl', 'awareMode', 'multipoint'],
    levels: true,
  },
];

function simulatedCapabilities(lacks: CapabilityKey[]): DeviceCapabilities {
  const caps = createUnknownCapabilities('Browser preview: no real hardware interrogated.');
  for (const key of [
    'battery',
    'noiseControl',
    'awareMode',
    'customNoiseControl',
    'equalizer',
    'firmwareVersion',
    'multipoint',
    'deviceRename',
    'pairedDevices',
  ] as const) {
    const absent = lacks.includes(key);
    caps[key] = {
      ...caps[key],
      // Deliberately `ready`, never `verified`: nothing here is hardware.
      status: absent ? 'unsupported' : 'ready',
      mechanism: 'none',
      hardwareVerified: false,
      evidence: absent ? 'The simulated model does not have this.' : SIMULATION_NOTE,
      lastEvaluated: new Date().toISOString(),
    };
  }
  return caps;
}

/** The ids the two made-up paired devices go by. */
const SIMULATED_THIS = '00000000000000000001';
const SIMULATED_OTHER = '00000000000000000002';

export class BrowserSimulatedDevice {
  private connected = false;
  private profile: SimulatedProfile = PROFILES[0];
  private batteryPercent = 78;
  private modeIndex = 0;
  private level = 6;
  private eq: EqSettings = { bass: 0, mid: 0, treble: 0 };
  private multipoint = true;
  private otherConnected = true;
  private name = PROFILES[0].name;
  private mutations = 0;

  /** The made-up devices, as a paired list would show them. */
  async paired(): Promise<PairedBose[]> {
    return PROFILES.map((p) => ({
      id: p.id,
      name: `${p.id === this.profile.id ? this.name : p.name} (simulated)`,
      model: p.model,
      connected: true,
      current: this.connected && p.id === this.profile.id,
    }));
  }

  /** Becomes the made-up device listed under `id`, connected. */
  async choose(id: string): Promise<DeviceSnapshot> {
    const profile = PROFILES.find((p) => p.id === id);
    if (!profile) {
      throw { kind: 'not-connected', message: 'That simulated device is not listed.' };
    }
    if (profile.id !== this.profile.id) {
      this.profile = profile;
      this.name = profile.name;
      this.modeIndex = profile.modes[0]?.index ?? 0;
    }
    this.connected = true;
    return this.snapshot();
  }

  private get mode(): NoiseControlMode {
    // A level is named, like a custom mode, and is neither Quiet nor Aware.
    if (this.profile.levels) return 'custom';
    if (this.modeIndex === 0) return 'quiet';
    if (this.modeIndex === 1) return 'aware';
    return 'custom';
  }

  private get activeMode() {
    return this.profile.modes.find((m) => m.index === this.modeIndex);
  }

  private noiseModes(): NoiseModeOption[] {
    return this.profile.modes.map((m) => ({
      index: m.index,
      name: m.name,
      preset: m.preset,
      adjustable: !m.preset && !this.profile.lacks.includes('customNoiseControl'),
      active: m.index === this.modeIndex,
    }));
  }

  async snapshot(): Promise<DeviceSnapshot> {
    const now = new Date().toISOString();
    const active = this.activeMode;
    const adjustable = !!active && !active.preset;
    return {
      source: 'mock',
      connection: this.connected ? 'connected' : 'disconnected',
      transport: this.connected ? 'simulated' : 'none',
      identity: this.connected
        ? {
            name: `${this.name} (SIMULATED)`,
            id: 'browser-preview-0000',
            manufacturer: 'Simulated',
            modelNumber: 'MOCK-QC',
            firmwareVersion: '0.0.0-simulated',
          }
        : null,
      capabilities: this.connected
        ? simulatedCapabilities(this.profile.lacks)
        : createUnknownCapabilities('Simulated device is disconnected.'),
      battery: this.connected
        ? {
            percent: this.batteryPercent,
            source: 'simulated',
            charging: false,
            readAt: now,
            ...(this.profile.parts ? { parts: this.profile.parts } : {}),
          }
        : null,
      noiseControl: this.connected
        ? {
            mode: this.mode,
            // Like the real headphones: only a custom mode has a level, and
            // every mode but Quiet and Aware goes by its own name.
            ...(this.mode === 'custom' ? { label: active?.name ?? 'Custom' } : {}),
            ...(adjustable ? { level: this.level } : {}),
            source: 'simulated',
            readAt: now,
          }
        : null,
      equalizer: this.connected ? { ...this.eq, source: 'simulated', readAt: now } : null,
      model: this.connected ? this.profile.model : null,
      noiseModes: this.connected ? this.noiseModes() : null,
      multipoint: this.connected
        ? { enabled: this.multipoint, source: 'simulated', readAt: now }
        : null,
      pairedDevices:
        this.connected && !this.profile.lacks.includes('pairedDevices')
          ? [
              {
                id: SIMULATED_THIS,
                name: 'This device (simulated)',
                connected: true,
                thisDevice: true,
              },
              {
                id: SIMULATED_OTHER,
                name: 'Another device (simulated)',
                // With multipoint off there is only the one connection.
                connected: this.multipoint && this.otherConnected,
                thisDevice: false,
              },
            ]
          : null,
      systemAudio: null,
      lastError: null,
      updatedAt: now,
    };
  }

  /** Switches mode, now and then without confirming it. */
  private switchTo(index: number): CommandOutcome {
    this.mutations += 1;
    this.modeIndex = index;
    // Exercise the "could not verify" path periodically, so that UI state
    // is not written assuming every command confirms.
    if (this.mutations % 7 === 0) {
      return {
        kind: 'sent-unverified',
        reason: 'Simulated device did not echo the new mode.',
      };
    }
    return { kind: 'applied', verifiedAt: new Date().toISOString() };
  }

  async execute(command: DeviceCommand): Promise<CommandOutcome> {
    switch (command.kind) {
      case 'connect':
        await delay(400);
        this.connected = true;
        return { kind: 'applied', verifiedAt: new Date().toISOString() };

      case 'disconnect':
        this.connected = false;
        return { kind: 'applied', verifiedAt: new Date().toISOString() };

      case 'reconnect':
        this.connected = false;
        await delay(300);
        this.connected = true;
        return { kind: 'applied', verifiedAt: new Date().toISOString() };

      case 'setNoiseControl': {
        if (!this.connected) return { kind: 'rejected', reason: 'No device is connected.' };
        if (this.profile.levels) {
          return {
            kind: 'unsupported',
            reason: 'The simulated QC35 II has noise-cancellation levels, not Quiet and Aware.',
          };
        }
        const index =
          command.mode === 'quiet'
            ? 0
            : command.mode === 'aware'
              ? 1
              : this.profile.modes.find((m) => !m.preset)?.index;
        if (index === undefined) {
          return {
            kind: 'unsupported',
            reason: 'No custom mode is set up on the simulated device.',
          };
        }
        return this.switchTo(index);
      }

      case 'selectNoiseMode': {
        if (!this.connected) return { kind: 'rejected', reason: 'No device is connected.' };
        if (!this.profile.modes.some((m) => m.index === command.index)) {
          return { kind: 'rejected', reason: 'That is not one of the simulated device’s modes.' };
        }
        return this.switchTo(command.index);
      }

      case 'setEqualizer': {
        if (!this.connected) return { kind: 'rejected', reason: 'No device is connected.' };
        const { bass, mid, treble } = command.settings;
        if ([bass, mid, treble].some((v) => v < -10 || v > 10)) {
          return { kind: 'rejected', reason: 'EQ gains must be within -10..10 dB.' };
        }
        this.eq = command.settings;
        return { kind: 'applied', verifiedAt: new Date().toISOString() };
      }

      case 'setMultipoint': {
        if (!this.connected) return { kind: 'rejected', reason: 'No device is connected.' };
        this.multipoint = command.enabled;
        return { kind: 'applied', verifiedAt: new Date().toISOString() };
      }

      case 'setPairedDeviceConnection': {
        if (!this.connected) return { kind: 'rejected', reason: 'No device is connected.' };
        if (command.device === SIMULATED_OTHER) {
          if (command.connected && !this.multipoint) {
            return {
              kind: 'unsupported',
              reason: 'The simulated headphones hold one connection while multipoint is off.',
            };
          }
          this.otherConnected = command.connected;
          return { kind: 'applied', verifiedAt: new Date().toISOString() };
        }
        if (command.device === SIMULATED_THIS && command.connected) {
          return { kind: 'applied', verifiedAt: new Date().toISOString() };
        }
        return {
          kind: 'unsupported',
          reason: 'The simulated headphones are not disconnected from the device simulating them.',
        };
      }

      case 'setDeviceName': {
        if (!this.connected) return { kind: 'rejected', reason: 'No device is connected.' };
        const checked = checkDeviceName(command.name);
        if (!checked.ok) return { kind: 'rejected', reason: checked.reason };
        this.name = checked.name;
        return { kind: 'applied', verifiedAt: new Date().toISOString() };
      }

      case 'setSystemVolume':
      case 'setSystemMute':
      case 'mediaPlayPause':
      case 'mediaNext':
      case 'mediaPrevious':
        return {
          kind: 'unsupported',
          reason: 'Windows audio is not reachable from the browser preview.',
        };

      case 'setNoiseControlLevel': {
        if (!this.connected) return { kind: 'rejected', reason: 'No device is connected.' };
        if (!Number.isInteger(command.level) || command.level < 0 || command.level > 10) {
          return { kind: 'rejected', reason: 'Level must be a whole number from 0 to 10.' };
        }
        if (this.activeMode?.preset !== false) {
          return {
            kind: 'unsupported',
            reason: 'The simulated device only has an adjustable level in a custom mode.',
          };
        }
        this.level = command.level;
        return { kind: 'applied', verifiedAt: new Date().toISOString() };
      }

      default:
        return { kind: 'applied', verifiedAt: new Date().toISOString() };
    }
  }
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
