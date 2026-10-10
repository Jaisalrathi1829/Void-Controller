import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke }));

type Ipc = typeof import('./ipc');

/** A fresh module per test: the browser fallback device is module state. */
async function loadIpc(): Promise<Ipc> {
  vi.resetModules();
  return import('./ipc');
}

function enterNativeShell() {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
}

beforeEach(() => {
  invoke.mockReset();
});

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
});

describe('typed noise-control operations — native shell', () => {
  it('asks the native layer for the current mode and takes no arguments', async () => {
    enterNativeShell();
    const state = { mode: 'aware', source: 'vendor-protocol', readAt: '2026-10-10T09:00:00Z' };
    invoke.mockResolvedValue(state);

    const ipc = await loadIpc();
    await expect(ipc.getCurrentMode()).resolves.toEqual(state);
    expect(invoke).toHaveBeenCalledWith('get_current_mode', undefined);
  });

  it('passes a null reading through as null, not as a default mode', async () => {
    enterNativeShell();
    invoke.mockResolvedValue(null);
    const ipc = await loadIpc();
    await expect(ipc.getCurrentMode()).resolves.toBeNull();
  });

  it.each(['quiet', 'aware', 'custom'] as const)(
    'sends %s as a mode name and nothing else',
    async (mode) => {
      enterNativeShell();
      invoke.mockResolvedValue({ kind: 'applied', verifiedAt: '2026-10-10T09:00:00Z' });

      const ipc = await loadIpc();
      await ipc.setNoiseControlMode(mode);

      expect(invoke).toHaveBeenCalledTimes(1);
      expect(invoke).toHaveBeenCalledWith('set_noise_control_mode', { mode });
    },
  );

  it('returns an unverified outcome as it is, without upgrading it', async () => {
    enterNativeShell();
    const outcome = { kind: 'sent-unverified', reason: 'still reports Quiet' };
    invoke.mockResolvedValue(outcome);

    const ipc = await loadIpc();
    await expect(ipc.setNoiseControlMode('aware')).resolves.toEqual(outcome);
  });

  // The type forbids these at compile time. A caller that defeats the type
  // must still not reach the native layer. "custom" in particular carries no
  // slot: there is no spelling that picks one.
  it.each([
    'off',
    '',
    'Custom',
    'custom:3',
    1,
    2,
    [0x1f, 0x03, 0x05, 0x02, 0x01, 0x00],
    '1F 03 05 02 01 00',
    { index: 1 },
    { custom: 3 },
    null,
  ])('refuses %j without calling the native layer', async (bad) => {
    enterNativeShell();
    const ipc = await loadIpc();
    await expect(ipc.setNoiseControlMode(bad as never)).rejects.toMatchObject({
      kind: 'invalid-input',
    });
    expect(invoke).not.toHaveBeenCalled();
  });
});

describe('noise-cancellation level — native shell', () => {
  it.each([0, 4, 10])('sends level %i as one typed command', async (level) => {
    enterNativeShell();
    invoke.mockResolvedValue({ kind: 'applied', verifiedAt: '2026-10-10T09:00:00Z' });

    const ipc = await loadIpc();
    await ipc.setNoiseCancellationLevel(level);

    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith('execute_command', {
      command: { kind: 'setNoiseControlLevel', level },
    });
  });

  it.each([-1, 11, 255, 4.5, Number.NaN, '4', null, [4], { level: 4 }])(
    'refuses %j without calling the native layer',
    async (bad) => {
      enterNativeShell();
      const ipc = await loadIpc();
      await expect(ipc.setNoiseCancellationLevel(bad as never)).rejects.toMatchObject({
        kind: 'invalid-input',
      });
      expect(invoke).not.toHaveBeenCalled();
    },
  );
});

describe('typed noise-control operations — browser preview', () => {
  it('has no reading before the simulated device is connected', async () => {
    const ipc = await loadIpc();
    await expect(ipc.getCurrentMode()).resolves.toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });

  it('round-trips through the simulated device, labelled as simulated', async () => {
    const ipc = await loadIpc();
    await ipc.executeCommand({ kind: 'connect' });

    const outcome = await ipc.setNoiseControlMode('aware');
    expect(outcome.kind).toBe('applied');

    const state = await ipc.getCurrentMode();
    expect(state?.mode).toBe('aware');
    expect(state?.source).toBe('simulated');
    expect(invoke).not.toHaveBeenCalled();
  });

  it('has a level only in the simulated custom mode', async () => {
    const ipc = await loadIpc();
    await ipc.executeCommand({ kind: 'connect' });

    // A preset has no level to report or adjust.
    expect((await ipc.getCurrentMode())?.level).toBeUndefined();
    expect((await ipc.setNoiseCancellationLevel(3)).kind).toBe('unsupported');

    await ipc.setNoiseControlMode('custom');
    expect((await ipc.setNoiseCancellationLevel(3)).kind).toBe('applied');
    const state = await ipc.getCurrentMode();
    expect(state?.mode).toBe('custom');
    expect(state?.level).toBe(3);
    expect(state?.source).toBe('simulated');
  });
});

describe('settings', () => {
  const view = {
    startWithWindows: false,
    startMinimized: false,
    minimizeToTray: true,
    autoConnect: true,
    autoReconnect: true,
    notifyConnected: false,
    notifyDisconnected: false,
    notifyLowBattery: false,
    voiceEnabled: false,
    announceModeChanges: true,
    announceConnection: true,
    announceBattery: true,
    voiceUseDefaultOutput: false,
    voiceId: 'af_bella',
    voiceSpeedPercent: 100,
    voiceVolumePercent: 80,
    preferredDeviceName: 'Aurora',
    unavailable: {},
  };

  it('are read from the native layer', async () => {
    enterNativeShell();
    invoke.mockResolvedValue(view);
    const ipc = await loadIpc();
    await expect(ipc.getSettings()).resolves.toEqual(view);
    expect(invoke).toHaveBeenCalledWith('get_settings', undefined);
  });

  it('are changed one named switch at a time', async () => {
    enterNativeShell();
    invoke.mockResolvedValue(view);
    const ipc = await loadIpc();
    await ipc.setSetting('minimizeToTray', true);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith('set_setting', { key: 'minimizeToTray', value: true });
  });

  // Nothing but a known switch and a boolean reaches the native layer. In
  // particular there is no way to name the preferred device from here.
  it.each([
    ['preferredDeviceId', true],
    ['preferredDeviceName', true],
    ['unavailable', true],
    ['', true],
    [null, true],
    ['minimizeToTray', 'yes'],
    ['minimizeToTray', 1],
    ['minimizeToTray', null],
  ])('refuses %j = %j without calling the native layer', async (key, value) => {
    enterNativeShell();
    const ipc = await loadIpc();
    await expect(ipc.setSetting(key as never, value as never)).rejects.toMatchObject({
      kind: 'invalid-input',
    });
    expect(invoke).not.toHaveBeenCalled();
  });

  it('are all off and unavailable in the browser preview, with the reason', async () => {
    const ipc = await loadIpc();
    const s = await ipc.getSettings();
    for (const key of ipc.SETTING_KEYS) {
      expect(s[key]).toBe(false);
      expect(s.unavailable[key]).toMatch(/browser preview/);
    }
    await expect(ipc.setSetting('minimizeToTray', true)).rejects.toMatchObject({
      kind: 'unsupported',
    });
    expect(invoke).not.toHaveBeenCalled();
  });

  it('device-change listening is a no-op in the browser preview', async () => {
    const ipc = await loadIpc();
    const handler = vi.fn();
    const stop = await ipc.onDeviceChanged(handler);
    stop();
    expect(handler).not.toHaveBeenCalled();
  });
});

describe('voice feedback', () => {
  const status = {
    state: 'ready',
    detail: 'Ready.',
    model: 'kokoro-int8-en-v0_19',
    assetsDir: 'C:\\voice\\kokoro-int8-en-v0_19',
    voices: [{ id: 'af_bella', label: 'Bella' }],
    speaking: null,
    isTest: false,
    output: null,
    modelLoaded: false,
  };

  it('reads the status from the native layer', async () => {
    enterNativeShell();
    invoke.mockResolvedValue(status);
    const ipc = await loadIpc();
    await expect(ipc.getVoiceStatus()).resolves.toEqual(status);
    expect(invoke).toHaveBeenCalledWith('get_voice_status', undefined);
  });

  // The sample sentence is fixed natively. Nothing the UI holds — and in
  // particular no text — travels with the request.
  it('asks for a test with no arguments at all', async () => {
    enterNativeShell();
    invoke.mockResolvedValue(undefined);
    const ipc = await loadIpc();
    await ipc.testVoice();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith('test_voice', undefined);
    expect(ipc.testVoice).toHaveLength(0);
  });

  it('stops speech and rechecks the model with no arguments', async () => {
    enterNativeShell();
    invoke.mockResolvedValue(status);
    const ipc = await loadIpc();
    await ipc.stopSpeaking();
    expect(invoke).toHaveBeenLastCalledWith('stop_speaking', undefined);
    await expect(ipc.recheckVoiceModel()).resolves.toEqual(status);
    expect(invoke).toHaveBeenLastCalledWith('recheck_voice_model', undefined);
  });

  it.each([
    [{ kind: 'voice', id: 'am_adam' }],
    [{ kind: 'speed', percent: 50 }],
    [{ kind: 'speed', percent: 200 }],
    [{ kind: 'volume', percent: 0 }],
    [{ kind: 'volume', percent: 100 }],
  ] as const)('sends %j as one named, bounded option', async (option) => {
    enterNativeShell();
    invoke.mockResolvedValue({});
    const ipc = await loadIpc();
    await ipc.setVoiceOption(option);
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(invoke).toHaveBeenCalledWith('set_voice_option', { option });
  });

  it.each([
    [{ kind: 'voice', id: '' }],
    [{ kind: 'voice', id: 7 }],
    [{ kind: 'speed', percent: 49 }],
    [{ kind: 'speed', percent: 201 }],
    [{ kind: 'speed', percent: 99.5 }],
    [{ kind: 'volume', percent: -1 }],
    [{ kind: 'volume', percent: 101 }],
    [{ kind: 'volume', percent: '80' }],
    [{ kind: 'text', value: 'say this' }],
    [{}],
    [null],
  ])('refuses %j without calling the native layer', async (option) => {
    enterNativeShell();
    const ipc = await loadIpc();
    await expect(ipc.setVoiceOption(option as never)).rejects.toMatchObject({
      kind: 'invalid-input',
    });
    expect(invoke).not.toHaveBeenCalled();
  });

  // A browser tab has no model and no way to pick the headphones' output. It
  // says so; it does not fall back to the browser's own speech synthesis.
  it('is unavailable in the browser preview, and speaks nothing', async () => {
    const speak = vi.fn();
    (window as unknown as Record<string, unknown>).speechSynthesis = { speak };
    try {
      const ipc = await loadIpc();
      const s = await ipc.getVoiceStatus();
      expect(s.state).toBe('unavailable');
      expect(s.detail).toMatch(/browser preview/);
      expect(s.voices).toEqual([]);

      await expect(ipc.testVoice()).rejects.toMatchObject({ kind: 'unsupported' });
      await expect(ipc.setVoiceOption({ kind: 'volume', percent: 50 })).rejects.toMatchObject({
        kind: 'unsupported',
      });
      await ipc.stopSpeaking();
      expect((await ipc.recheckVoiceModel()).state).toBe('unavailable');

      const handler = vi.fn();
      (await ipc.onVoiceStatus(handler))();
      expect(handler).not.toHaveBeenCalled();
      expect(invoke).not.toHaveBeenCalled();
      expect(speak).not.toHaveBeenCalled();
    } finally {
      delete (window as unknown as Record<string, unknown>).speechSynthesis;
    }
  });

  it('exposes no way to pass text to be spoken', async () => {
    const ipc = await loadIpc();
    const exported = Object.keys(ipc).filter((name) => /speak|say|tts|synth/i.test(name));
    expect(exported).toEqual(['stopSpeaking']);
  });
});

describe('requestable noise modes', () => {
  it('are exactly quiet, aware and custom', async () => {
    const ipc = await loadIpc();
    expect(ipc.REQUESTABLE_NOISE_MODES).toEqual(['quiet', 'aware', 'custom']);
    expect(ipc.isRequestableNoiseMode('quiet')).toBe(true);
    expect(ipc.isRequestableNoiseMode('aware')).toBe(true);
    expect(ipc.isRequestableNoiseMode('custom')).toBe(true);
    expect(ipc.isRequestableNoiseMode('off')).toBe(false);
    expect(ipc.isRequestableNoiseMode('QUIET')).toBe(false);
    expect(ipc.isRequestableNoiseMode(0)).toBe(false);
  });
});
