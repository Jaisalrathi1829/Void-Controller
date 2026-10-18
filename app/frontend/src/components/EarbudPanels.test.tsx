import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createUnknownCapabilities, type CapabilityStatus } from '@/types/capability';
import type { DeviceSnapshot, LastSeen } from '@/types/device';
import type { AppSettings } from '@/services/ipc';

const toast = vi.hoisted(() => ({ success: vi.fn(), warning: vi.fn(), error: vi.fn(), info: vi.fn() }));
vi.mock('sonner', () => ({ toast }));

const ipcMock = vi.hoisted(() => ({
  getSnapshot: vi.fn(),
  executeCommand: vi.fn(),
  getSettings: vi.fn(),
  setSetting: vi.fn(),
  getLastSeen: vi.fn(),
  onSettingsChanged: vi.fn(),
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
import { EarbudControlsPanel, FindMyBudsPanel } from './EarbudPanels';

function snapshot(category: 'earbuds' | 'headphones', status: CapabilityStatus = 'ready'): DeviceSnapshot {
  const capabilities = createUnknownCapabilities('test');
  for (const key of ['touchControls', 'inEarDetection', 'autoAnswer'] as const) {
    capabilities[key] = { ...capabilities[key], status, evidence: `${key} ${status}` };
  }
  return {
    source: 'real',
    connection: 'connected',
    transport: 'bluetooth-classic',
    identity: { name: 'Buds', id: 'test' },
    capabilities,
    battery: null,
    noiseControl: null,
    equalizer: null,
    model: { name: 'QuietComfort Ultra Earbuds (2nd Gen)', category, support: 'documented' },
    shortcut: {
      control: 'Shortcut',
      gesture: 'Press and hold',
      action: 3,
      actionName: 'Battery level',
      options: [
        { action: 3, name: 'Battery level' },
        { action: 14, name: 'Off' },
        { action: 16, name: 'Spotify Tap' },
      ],
      readAt: '2026-10-17T10:00:00Z',
    },
    wear: { autoPause: true, autoAnswer: false },
    systemAudio: null,
    lastError: null,
    updatedAt: '2026-10-17T10:00:00Z',
  };
}

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
    desktopWidget: false,
    widgetOnTop: true,
    rememberLastSeen: false,
    preferredDeviceName: null,
    voiceId: 'af_heart',
    voiceSpeedPercent: 100,
    voiceVolumePercent: 80,
    unavailable: {},
    ...over,
  };
}

async function renderPanels() {
  render(
    <AppProvider>
      <EarbudControlsPanel />
      <FindMyBudsPanel />
    </AppProvider>,
  );
  await waitFor(() => expect(ipcMock.getSnapshot).toHaveBeenCalled());
  await act(async () => {});
}

beforeEach(() => {
  vi.clearAllMocks();
  ipcMock.getSettings.mockResolvedValue(settings());
  ipcMock.getLastSeen.mockResolvedValue(null);
  ipcMock.onSettingsChanged.mockResolvedValue(() => {});
  ipcMock.onDeviceChanged.mockResolvedValue(() => {});
  ipcMock.onVoiceStatus.mockResolvedValue(() => {});
  ipcMock.getVoiceStatus.mockRejectedValue({ kind: 'platform', message: 'none' });
  ipcMock.executeCommand.mockResolvedValue({ kind: 'applied', verifiedAt: 'now' });
});

describe('the earbuds panels', () => {
  it('are shown for earbuds, with the actions the earbuds list', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot('earbuds'));
    await renderPanels();

    expect(screen.getByText('Touch controls')).toBeInTheDocument();
    expect(screen.getByRole('tab', { name: 'Battery level' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('switch', { name: 'Pause when taken out' })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('switch', { name: 'Answer calls when put in' })).toHaveAttribute('aria-checked', 'false');
    expect(screen.getByTestId('find-my-buds')).toBeInTheDocument();
  });

  it('are not shown for headphones', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot('headphones'));
    await renderPanels();

    expect(screen.queryByTestId('earbud-controls')).not.toBeInTheDocument();
    expect(screen.queryByTestId('find-my-buds')).not.toBeInTheDocument();
  });

  it('leave out what the earbuds do not have', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot('earbuds', 'unsupported'));
    await renderPanels();
    expect(screen.queryByTestId('earbud-controls')).not.toBeInTheDocument();
  });

  it('set the shortcut and the switches by what was chosen', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot('earbuds'));
    await renderPanels();

    await userEvent.click(screen.getByRole('tab', { name: 'Spotify Tap' }));
    await waitFor(() =>
      expect(ipcMock.executeCommand).toHaveBeenCalledWith({ kind: 'setShortcut', action: 16 }),
    );
    await userEvent.click(screen.getByRole('switch', { name: 'Answer calls when put in' }));
    await waitFor(() =>
      expect(ipcMock.executeCommand).toHaveBeenCalledWith({ kind: 'setAutoAnswer', enabled: true }),
    );
  });
});

describe('find my buds', () => {
  it('is switched on by the setting', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot('earbuds'));
    ipcMock.setSetting.mockResolvedValue(settings({ rememberLastSeen: true }));
    await renderPanels();

    expect(screen.queryByTestId('last-seen')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('switch', { name: 'Note where they were last connected' }));
    expect(ipcMock.setSetting).toHaveBeenCalledWith('rememberLastSeen', true);
  });

  it('shows when and where they were last connected', async () => {
    const seen: LastSeen = {
      deviceName: 'Buds',
      at: '2026-10-17T10:00:00Z',
      place: { latitudeE6: 28613900, longitudeE6: 77209000, accuracyM: 12, at: '2026-10-17T09:59:00Z' },
    };
    ipcMock.getSnapshot.mockResolvedValue(snapshot('earbuds'));
    ipcMock.getSettings.mockResolvedValue(settings({ rememberLastSeen: true }));
    ipcMock.getLastSeen.mockResolvedValue(seen);
    await renderPanels();

    const note = await screen.findByTestId('last-seen');
    expect(note).toHaveTextContent('Last connected');
    expect(note).toHaveTextContent('28.61390, 77.20900');
    expect(note).toHaveTextContent('within about 12 m');
  });
});
