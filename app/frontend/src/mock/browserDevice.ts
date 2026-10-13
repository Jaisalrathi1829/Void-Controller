/**
 * Browser-preview simulation.
 *
 * Used only when the page is opened outside the Tauri shell, so the UI can be
 * developed and tested in a plain browser. It mirrors the Rust mock's most
 * important property: it can never produce a `verified` capability, because a
 * simulation is evidence about the software, not about anyone's headphones.
 */

import {
  checkDeviceName,
  type CommandOutcome,
  type DeviceSnapshot,
  type EqSettings,
  type NoiseControlMode,
} from '@/types/device';
import { createUnknownCapabilities, type DeviceCapabilities } from '@/types/capability';
import type { DeviceCommand } from '@/services/ipc';

const SIMULATION_NOTE =
  'Simulated in the browser preview. This is evidence about the application, not about any physical device.';

function simulatedCapabilities(): DeviceCapabilities {
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
    caps[key] = {
      ...caps[key],
      // Deliberately `ready`, never `verified`: nothing here is hardware.
      status: 'ready',
      mechanism: 'none',
      hardwareVerified: false,
      evidence: SIMULATION_NOTE,
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
  private batteryPercent = 78;
  private mode: NoiseControlMode = 'quiet';
  private level = 6;
  private eq: EqSettings = { bass: 0, mid: 0, treble: 0 };
  private multipoint = true;
  private otherConnected = true;
  private name = 'Bose QuietComfort';
  private mutations = 0;

  async snapshot(): Promise<DeviceSnapshot> {
    const now = new Date().toISOString();
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
        ? simulatedCapabilities()
        : createUnknownCapabilities('Simulated device is disconnected.'),
      battery: this.connected
        ? { percent: this.batteryPercent, source: 'simulated', charging: false, readAt: now }
        : null,
      noiseControl: this.connected
        ? {
            mode: this.mode,
            // Like the real headphones: only the custom mode has a level.
            ...(this.mode === 'custom' ? { level: this.level, label: 'Simulated custom' } : {}),
            source: 'simulated',
            readAt: now,
          }
        : null,
      equalizer: this.connected ? { ...this.eq, source: 'simulated', readAt: now } : null,
      multipoint: this.connected
        ? { enabled: this.multipoint, source: 'simulated', readAt: now }
        : null,
      pairedDevices: this.connected
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
        this.mutations += 1;
        this.mode = command.mode;
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
        if (this.mode !== 'custom') {
          return {
            kind: 'unsupported',
            reason: 'The simulated device only has an adjustable level in Custom mode.',
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
