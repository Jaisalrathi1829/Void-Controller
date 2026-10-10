import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createUnknownCapabilities } from '@/types/capability';
import type { DeviceSnapshot } from '@/types/device';
import type { AppSettings, SettingKey } from '@/services/ipc';

const toast = vi.hoisted(() => ({ success: vi.fn(), warning: vi.fn(), error: vi.fn() }));
vi.mock('sonner', () => ({ toast }));

const ipcMock = vi.hoisted(() => ({
  getSnapshot: vi.fn(),
  getSettings: vi.fn(),
  setSetting: vi.fn(),
  onDeviceChanged: vi.fn(),
  getVoiceStatus: vi.fn(),
  onVoiceStatus: vi.fn(),
}));
vi.mock('@/services/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/services/ipc')>()),
  hasNativeLayer: () => true,
  ...ipcMock,
}));

import { AppProvider } from '@/store';
import { SettingsPage } from './SettingsPage';

function settings(over: Partial<AppSettings> = {}): AppSettings {
  return {
    startWithWindows: false,
    startMinimized: false,
    minimizeToTray: false,
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
    ...over,
  };
}

function snapshot(mode: 'quiet' | 'aware' = 'quiet'): DeviceSnapshot {
  return {
    source: 'real',
    connection: 'connected',
    transport: 'bluetooth-classic',
    identity: { name: 'Aurora', id: 'test' },
    capabilities: createUnknownCapabilities('not investigated'),
    battery: null,
    noiseControl: { mode, source: 'vendor-protocol', readAt: '2026-10-10T09:00:00Z' },
    equalizer: null,
    windowsAudio: null,
    lastError: null,
    updatedAt: '2026-10-10T09:00:00Z',
  };
}

/** Captures the handler the store registers for native device changes. */
let deviceChanged: (() => void) | undefined;

async function renderPage() {
  render(
    <AppProvider>
      <SettingsPage />
    </AppProvider>,
  );
  await waitFor(() => expect(ipcMock.getSettings).toHaveBeenCalled());
  await act(async () => {});
}

const toggle = (name: string) => screen.getByRole('switch', { name });

beforeEach(() => {
  vi.clearAllMocks();
  deviceChanged = undefined;
  ipcMock.getSnapshot.mockResolvedValue(snapshot());
  ipcMock.onDeviceChanged.mockImplementation(async (handler: () => void) => {
    deviceChanged = handler;
    return () => {};
  });
  ipcMock.getVoiceStatus.mockResolvedValue({
    state: 'ready',
    detail: 'Ready.',
    model: 'kokoro-int8-en-v0_19',
    assetsDir: 'C:\\voice',
    voices: [{ id: 'af_bella', label: 'Bella' }],
    speaking: null,
    isTest: false,
    output: null,
    modelLoaded: false,
  });
  ipcMock.onVoiceStatus.mockResolvedValue(() => {});
});

describe('settings switches', () => {
  it('show what the native layer reports, not defaults of their own', async () => {
    ipcMock.getSettings.mockResolvedValue(
      settings({ minimizeToTray: true, notifyLowBattery: true, autoConnect: false }),
    );
    await renderPage();

    expect(toggle('Minimize to tray')).toHaveAttribute('aria-checked', 'true');
    expect(toggle('Low battery')).toHaveAttribute('aria-checked', 'true');
    expect(toggle('Auto-connect')).toHaveAttribute('aria-checked', 'false');
    expect(toggle('Auto reconnect')).toHaveAttribute('aria-checked', 'true');
    expect(toggle('Start minimized')).toHaveAttribute('aria-checked', 'false');
  });

  it('are all usable when nothing is unavailable', async () => {
    ipcMock.getSettings.mockResolvedValue(settings());
    await renderPage();

    for (const name of [
      'Start with Windows',
      'Start minimized',
      'Minimize to tray',
      'Auto-connect',
      'Auto reconnect',
      'Device connected',
      'Device disconnected',
      'Low battery',
    ]) {
      expect(toggle(name)).toBeEnabled();
    }
    expect(screen.queryByText(/Not implemented/)).not.toBeInTheDocument();
  });

  it.each<[string, SettingKey, boolean]>([
    ['Start minimized', 'startMinimized', true],
    ['Minimize to tray', 'minimizeToTray', true],
    ['Auto-connect', 'autoConnect', false],
    ['Auto reconnect', 'autoReconnect', false],
    ['Device connected', 'notifyConnected', true],
    ['Device disconnected', 'notifyDisconnected', true],
    ['Low battery', 'notifyLowBattery', true],
    ['Start with Windows', 'startWithWindows', true],
  ])('%s asks the native layer for the opposite value', async (name, key, wanted) => {
    ipcMock.getSettings.mockResolvedValue(settings());
    ipcMock.setSetting.mockResolvedValue(settings({ [key]: wanted }));
    await renderPage();

    await userEvent.click(toggle(name));

    await waitFor(() => expect(ipcMock.setSetting).toHaveBeenCalledWith(key, wanted));
    expect(ipcMock.setSetting).toHaveBeenCalledTimes(1);
    await waitFor(() =>
      expect(toggle(name)).toHaveAttribute('aria-checked', String(wanted)),
    );
  });

  it('show what the native layer did, even if that is not what was asked', async () => {
    ipcMock.getSettings.mockResolvedValue(settings());
    // Asked to turn on, but the native side reports it still off.
    ipcMock.setSetting.mockResolvedValue(settings({ minimizeToTray: false }));
    await renderPage();

    await userEvent.click(toggle('Minimize to tray'));

    await waitFor(() => expect(ipcMock.setSetting).toHaveBeenCalled());
    expect(toggle('Minimize to tray')).toHaveAttribute('aria-checked', 'false');
  });

  it('report a failed change and re-read the real state', async () => {
    ipcMock.getSettings.mockResolvedValue(settings());
    ipcMock.setSetting.mockRejectedValue({
      kind: 'platform',
      message: 'could not create the tray icon',
    });
    await renderPage();
    const readsBefore = ipcMock.getSettings.mock.calls.length;

    await userEvent.click(toggle('Minimize to tray'));

    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    expect(toast.error).toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({ description: 'could not create the tray icon' }),
    );
    await waitFor(() =>
      expect(ipcMock.getSettings.mock.calls.length).toBeGreaterThan(readsBefore),
    );
    expect(toggle('Minimize to tray')).toHaveAttribute('aria-checked', 'false');
  });

  it('disable a switch that cannot work in this build, and say why', async () => {
    const reason = 'Only available in an installed build.';
    ipcMock.getSettings.mockResolvedValue(
      settings({ unavailable: { startWithWindows: reason } }),
    );
    await renderPage();

    expect(toggle('Start with Windows')).toBeDisabled();
    expect(screen.getByText(reason)).toBeInTheDocument();
    // The others are unaffected.
    expect(toggle('Minimize to tray')).toBeEnabled();
    expect(ipcMock.setSetting).not.toHaveBeenCalled();
  });

  it('stay inert until the settings have been read', async () => {
    ipcMock.getSettings.mockReturnValue(new Promise(() => {}));
    render(
      <AppProvider>
        <SettingsPage />
      </AppProvider>,
    );
    await act(async () => {});

    for (const el of screen.getAllByRole('switch')) {
      expect(el).toBeDisabled();
      expect(el).toHaveAttribute('aria-checked', 'false');
    }
  });

  it('explain where "start minimized" will put the window', async () => {
    ipcMock.getSettings.mockResolvedValue(settings({ minimizeToTray: false }));
    await renderPage();
    expect(screen.getByText(/minimized to the taskbar/)).toBeInTheDocument();
  });
});

describe('preferred device', () => {
  it('shows the remembered name', async () => {
    ipcMock.getSettings.mockResolvedValue(settings({ preferredDeviceName: 'Aurora' }));
    await renderPage();
    expect(screen.getByTestId('preferred-device')).toHaveTextContent('Aurora');
  });
});

describe('headphones coming and going', () => {
  it('re-reads the device when the native layer says it changed', async () => {
    ipcMock.getSettings.mockResolvedValue(settings());
    await renderPage();
    await waitFor(() => expect(deviceChanged).toBeDefined());
    const readsBefore = ipcMock.getSnapshot.mock.calls.length;

    await act(async () => {
      deviceChanged?.();
    });

    await waitFor(() =>
      expect(ipcMock.getSnapshot.mock.calls.length).toBeGreaterThan(readsBefore),
    );
  });
});
