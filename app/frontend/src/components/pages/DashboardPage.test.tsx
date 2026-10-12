import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { createUnknownCapabilities } from '@/types/capability';
import type { DeviceSnapshot } from '@/types/device';

const toast = vi.hoisted(() => ({ success: vi.fn(), warning: vi.fn(), error: vi.fn() }));
vi.mock('sonner', () => ({ toast }));

const ipcMock = vi.hoisted(() => ({
  getSnapshot: vi.fn(),
  getSettings: vi.fn(),
  executeCommand: vi.fn(),
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
import { unavailableSettings, voiceUnavailable } from '@/services/ipc';
import { DashboardPage } from './DashboardPage';
import { DevicePage } from './DevicePage';

const DESKTOP_UA = window.navigator.userAgent;
const ANDROID_UA =
  'Mozilla/5.0 (Linux; Android 14; SM-S928B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36';

function useAgent(value: string) {
  Object.defineProperty(window.navigator, 'userAgent', { value, configurable: true });
}

function setHidden(hidden: boolean, announce = true) {
  Object.defineProperty(document, 'hidden', { value: hidden, configurable: true });
  if (announce) document.dispatchEvent(new Event('visibilitychange'));
}

function snapshot(over: Partial<DeviceSnapshot> = {}): DeviceSnapshot {
  return {
    source: 'real',
    connection: 'connected',
    transport: 'bluetooth-classic',
    identity: { name: 'Aurora', id: 'test' },
    capabilities: createUnknownCapabilities('not investigated'),
    battery: { percent: 70, source: 'android-bluetooth', readAt: '2026-10-12T09:00:00Z' },
    noiseControl: { mode: 'quiet', source: 'vendor-protocol', readAt: '2026-10-12T09:00:00Z' },
    equalizer: null,
    systemAudio: {
      endpointName: 'Aurora',
      endpointId: 'media',
      volumePercent: 53,
      muted: false,
      isDefaultRender: true,
      isDefaultCommunications: false,
    },
    hostName: 'Galaxy S24 Ultra',
    lastError: null,
    updatedAt: '2026-10-12T09:00:00Z',
    ...over,
  };
}

let deviceChanged: (() => void) | undefined;

beforeEach(() => {
  vi.clearAllMocks();
  deviceChanged = undefined;
  ipcMock.getSnapshot.mockResolvedValue(snapshot());
  ipcMock.getSettings.mockResolvedValue(unavailableSettings('not under test'));
  ipcMock.getVoiceStatus.mockResolvedValue(voiceUnavailable('not under test'));
  ipcMock.onVoiceStatus.mockResolvedValue(() => {});
  ipcMock.onDeviceChanged.mockImplementation(async (handler: () => void) => {
    deviceChanged = handler;
    return () => {};
  });
  ipcMock.executeCommand.mockResolvedValue({ kind: 'applied' });
});

afterEach(() => {
  useAgent(DESKTOP_UA);
  // Put back quietly: telling a page that is about to be torn down that it
  // is visible again would only start a read nobody is waiting for.
  setHidden(false, false);
});

/** Lets a command that has been answered finish putting the screen right. */
async function settled() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

async function renderDashboard() {
  render(
    <AppProvider>
      <DashboardPage onNavigate={() => {}} />
    </AppProvider>,
  );
  await waitFor(() => expect(ipcMock.getSnapshot).toHaveBeenCalled());
  await act(async () => {});
}

describe('which phone or computer the headphones are connected to', () => {
  it('is named while real headphones are connected', async () => {
    await renderDashboard();
    expect(screen.getByText('Galaxy S24 Ultra')).toBeInTheDocument();
    expect(screen.getByText(/Connected to/)).toBeInTheDocument();
  });

  it('is not named for headphones that are not connected', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ connection: 'disconnected' }));
    await renderDashboard();
    expect(screen.queryByText(/Connected to/)).not.toBeInTheDocument();
    expect(screen.queryByText('Galaxy S24 Ultra')).not.toBeInTheDocument();
  });

  // The simulated device is connected to nothing, whatever this machine is
  // called.
  it('is not named for the simulated device', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ source: 'mock' }));
    await renderDashboard();
    expect(screen.queryByText(/Connected to/)).not.toBeInTheDocument();
  });

  it('is simply left out when the system gave no name', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ hostName: null }));
    await renderDashboard();
    expect(screen.queryByText(/Connected to/)).not.toBeInTheDocument();
  });

  it('is on the Device page too', async () => {
    render(
      <AppProvider>
        <DevicePage onNavigate={() => {}} />
      </AppProvider>,
    );
    await waitFor(() => expect(ipcMock.getSnapshot).toHaveBeenCalled());
    await act(async () => {});
    expect(screen.getByText('Connected to')).toBeInTheDocument();
    expect(screen.getByText('Galaxy S24 Ultra')).toBeInTheDocument();
  });
});

describe('the volume slider on a phone', () => {
  beforeEach(() => useAgent(ANDROID_UA));

  it('is the phone\'s media volume, and says what it is playing through', async () => {
    await renderDashboard();
    expect(screen.getByText('Phone volume')).toBeInTheDocument();
    const slider = screen.getByRole('slider', { name: 'Aurora' }) as HTMLInputElement;
    expect(slider).not.toBeDisabled();
    expect(slider.value).toBe('53');
    expect(screen.getByText(/phone's media volume/)).toBeInTheDocument();
  });

  it('sets the volume once, when let go, and says so in the phone\'s terms', async () => {
    await renderDashboard();
    const slider = screen.getByRole('slider', { name: 'Aurora' });
    fireEvent.change(slider, { target: { value: '80' } });
    expect(ipcMock.executeCommand).not.toHaveBeenCalled();
    fireEvent.touchEnd(slider);

    await waitFor(() => expect(ipcMock.executeCommand).toHaveBeenCalledTimes(1));
    expect(ipcMock.executeCommand).toHaveBeenCalledWith({ kind: 'setSystemVolume', percent: 80 });
    await waitFor(() => expect(toast.success).toHaveBeenCalledTimes(1));
    expect(toast.success.mock.calls[0]?.[0]).toBe('Phone volume applied.');
    await settled();
  });

  // What the phone then reports is what shows: its volume moves in steps.
  it('shows the step the phone settled on, not the number asked for', async () => {
    await renderDashboard();
    const audio = snapshot().systemAudio!;
    ipcMock.getSnapshot.mockResolvedValue(
      snapshot({ systemAudio: { ...audio, volumePercent: 80 } }),
    );
    const slider = screen.getByRole('slider', { name: 'Aurora' }) as HTMLInputElement;
    fireEvent.change(slider, { target: { value: '78' } });
    fireEvent.touchEnd(slider);
    await waitFor(() => expect(slider.value).toBe('80'));
    await settled();
  });

  it('is not offered when the phone\'s volume could not be read', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ systemAudio: null }));
    await renderDashboard();
    expect(screen.getByRole('slider', { name: 'Output' })).toBeDisabled();
    expect(screen.getByText(/could not be read/)).toBeInTheDocument();
  });
});

describe('the volume slider on a desktop', () => {
  it('keeps its own wording', async () => {
    await renderDashboard();
    expect(screen.getByText('Volume')).toBeInTheDocument();
    expect(screen.queryByText('Phone volume')).not.toBeInTheDocument();
    expect(screen.queryByText(/phone's media volume/)).not.toBeInTheDocument();

    const slider = screen.getByRole('slider', { name: 'Aurora' });
    fireEvent.change(slider, { target: { value: '30' } });
    fireEvent.pointerUp(slider);
    await waitFor(() => expect(toast.success).toHaveBeenCalledTimes(1));
    expect(toast.success.mock.calls[0]?.[0]).toBe('Windows volume applied.');
    await settled();
  });
});

describe('while nobody is looking', () => {
  it('a change reported by the native layer is not re-read until the window is shown', async () => {
    await renderDashboard();
    await waitFor(() => expect(deviceChanged).toBeDefined());
    const before = ipcMock.getSnapshot.mock.calls.length;

    await act(async () => setHidden(true));
    await act(async () => deviceChanged?.());
    expect(ipcMock.getSnapshot.mock.calls.length).toBe(before);

    // Shown again: read straight away, without waiting for anything.
    await act(async () => setHidden(false));
    await waitFor(() => expect(ipcMock.getSnapshot.mock.calls.length).toBe(before + 1));
  });

  it('a change reported while the window is showing is read at once', async () => {
    await renderDashboard();
    await waitFor(() => expect(deviceChanged).toBeDefined());
    const before = ipcMock.getSnapshot.mock.calls.length;
    await act(async () => deviceChanged?.());
    await waitFor(() => expect(ipcMock.getSnapshot.mock.calls.length).toBe(before + 1));
  });
});
