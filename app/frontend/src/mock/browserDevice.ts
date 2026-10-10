/**
 * Browser-preview simulation.
 *
 * Used only when the page is opened outside the Tauri shell, so the UI can be
 * developed and tested in a plain browser. It mirrors the Rust mock's most
 * important property: it can never produce a `verified` capability, because a
 * simulation is evidence about the software, not about anyone's headphones.
 */

import type { CommandOutcome, DeviceSnapshot, EqSettings, NoiseControlMode } from '@/types/device';
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
  ] as const) {
    caps[key] = {
      ...caps[key],
      // Deliberately `experimental`, never `verified`.
      status: 'experimental',
      mechanism: 'none',
      hardwareVerified: false,
      evidence: SIMULATION_NOTE,
      lastEvaluated: new Date().toISOString(),
    };
  }
  return caps;
}

export class BrowserSimulatedDevice {
  private connected = false;
  private batteryPercent = 78;
  private mode: NoiseControlMode = 'quiet';
  private level = 6;
  private eq: EqSettings = { bass: 0, mid: 0, treble: 0 };
  private mutations = 0;

  async snapshot(): Promise<DeviceSnapshot> {
    const now = new Date().toISOString();
    return {
      source: 'mock',
      connection: this.connected ? 'connected' : 'disconnected',
      transport: this.connected ? 'simulated' : 'none',
      identity: this.connected
        ? {
            name: 'Bose QuietComfort (SIMULATED)',
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
      windowsAudio: null,
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
