import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createUnknownCapabilities } from '@/types/capability';
import type { DeviceSnapshot } from '@/types/device';
import type { AppSettings } from '@/services/ipc';

const toast = vi.hoisted(() => ({ success: vi.fn(), warning: vi.fn(), error: vi.fn() }));
vi.mock('sonner', () => ({ toast }));

const ipcMock = vi.hoisted(() => ({
  getSnapshot: vi.fn(),
  getSettings: vi.fn(),
  setSetting: vi.fn(),
  onDeviceChanged: vi.fn(),
  getVoiceStatus: vi.fn(),
  onVoiceStatus: vi.fn(),
  requestBluetoothPermission: vi.fn(),
}));
vi.mock('@/services/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/services/ipc')>()),
  hasNativeLayer: () => true,
  ...ipcMock,
}));

import { AppProvider } from '@/store';
import { ConnectionScreen } from './ConnectionScreens';
import { BottomNav } from './Sidebar';
import { Range } from './primitives';
import { SettingsPage } from './pages/SettingsPage';
import { VoiceFeedbackPanel } from './pages/VoiceFeedbackPanel';
import { EqualizerPage } from './pages/EqualizerPage';
import { DevicePage } from './pages/DevicePage';
import { pageTitles } from '../pages';

const DESKTOP_UA = window.navigator.userAgent;
const ANDROID_UA =
  'Mozilla/5.0 (Linux; Android 14; SM-S928B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36';

function useAgent(value: string) {
  Object.defineProperty(window.navigator, 'userAgent', { value, configurable: true });
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
    voiceId: 'af_bella',
    voiceSpeedPercent: 100,
    voiceVolumePercent: 80,
    preferredDeviceName: 'Aurora',
    unavailable: {},
    ...over,
  };
}

const snapshot: DeviceSnapshot = {
  source: 'real',
  connection: 'connected',
  transport: 'bluetooth-classic',
  identity: { name: 'Aurora', id: 'test' },
  capabilities: createUnknownCapabilities('not investigated'),
  battery: { percent: 70, source: 'android-bluetooth', readAt: '2026-10-11T09:00:00Z' },
  noiseControl: { mode: 'quiet', source: 'vendor-protocol', readAt: '2026-10-11T09:00:00Z' },
  equalizer: null,
  systemAudio: null,
  lastError: null,
  updatedAt: '2026-10-11T09:00:00Z',
};

const NOT_PERMITTED = {
  kind: 'not-permitted',
  message: 'This application has not been allowed to use Bluetooth.',
};

beforeEach(() => {
  vi.clearAllMocks();
  ipcMock.getSnapshot.mockResolvedValue(snapshot);
  ipcMock.getSettings.mockResolvedValue(settings());
  ipcMock.onDeviceChanged.mockResolvedValue(() => {});
  ipcMock.onVoiceStatus.mockResolvedValue(() => {});
  ipcMock.getVoiceStatus.mockResolvedValue({
    state: 'not-installed',
    detail: 'The voice model is not installed.',
    model: 'kokoro-int8-en-v0_19',
    assetsDir: '/data/user/0/app/files/voice/kokoro-int8-en-v0_19',
    voices: [],
    speaking: null,
    isTest: false,
    output: null,
    modelLoaded: false,
  });
  ipcMock.requestBluetoothPermission.mockResolvedValue(undefined);
});

afterEach(() => {
  useAgent(DESKTOP_UA);
  vi.useRealTimers();
});

describe('a Bluetooth permission that has not been given', () => {
  beforeEach(() => {
    ipcMock.getSnapshot.mockRejectedValue(NOT_PERMITTED);
  });

  async function renderDisconnected() {
    render(
      <AppProvider>
        <ConnectionScreen state="disconnected" />
      </AppProvider>,
    );
    await waitFor(() => expect(ipcMock.getSnapshot).toHaveBeenCalled());
    await act(async () => {});
  }

  it('is named as the problem, instead of "no headphones connected"', async () => {
    await renderDisconnected();
    expect(screen.getByRole('heading', { name: 'Bluetooth access is needed.' })).toBeInTheDocument();
    expect(screen.getByTestId('permission-reason')).toHaveTextContent(NOT_PERMITTED.message);
    expect(screen.queryByText(/No Bose headphones connected/)).not.toBeInTheDocument();
  });

  it('can be asked for again, and the device is read once it is given', async () => {
    await renderDisconnected();
    ipcMock.getSnapshot.mockResolvedValue(snapshot);

    await userEvent.click(screen.getByRole('button', { name: 'Allow Bluetooth access' }));

    await waitFor(() => expect(ipcMock.requestBluetoothPermission).toHaveBeenCalledTimes(1));
    expect(ipcMock.requestBluetoothPermission).toHaveBeenCalledWith();
    await waitFor(() =>
      expect(screen.queryByRole('heading', { name: 'Bluetooth access is needed.' })).not.toBeInTheDocument(),
    );
  });

  // The system dialog is answered outside the page, and nothing tells the
  // page it was. So it keeps looking.
  it('is looked for again without being asked', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    await renderDisconnected();
    const before = ipcMock.getSnapshot.mock.calls.length;

    await act(async () => {
      vi.advanceTimersByTime(2100);
    });
    expect(ipcMock.getSnapshot.mock.calls.length).toBeGreaterThan(before);

    // And stops once the device can be read.
    ipcMock.getSnapshot.mockResolvedValue(snapshot);
    await act(async () => {
      vi.advanceTimersByTime(2100);
    });
    await waitFor(() =>
      expect(screen.queryByRole('heading', { name: 'Bluetooth access is needed.' })).not.toBeInTheDocument(),
    );
    // Two more of its intervals pass without another look. (Kept short of
    // five seconds, when the ordinary refresh of connected headphones is due.)
    const settled = ipcMock.getSnapshot.mock.calls.length;
    await act(async () => {
      vi.advanceTimersByTime(4200);
    });
    expect(ipcMock.getSnapshot.mock.calls.length).toBe(settled);
  });

  it('reports a request that could not be made', async () => {
    ipcMock.requestBluetoothPermission.mockRejectedValue({
      kind: 'unsupported',
      message: 'There is no Bluetooth permission to ask for on this system.',
    });
    await renderDisconnected();
    await userEvent.click(screen.getByRole('button', { name: 'Allow Bluetooth access' }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
  });
});

describe('any other reason the device cannot be read', () => {
  it('shows the ordinary disconnected screen', async () => {
    ipcMock.getSnapshot.mockRejectedValue({ kind: 'bluetooth-unavailable', message: 'no radio' });
    render(
      <AppProvider>
        <ConnectionScreen state="disconnected" />
      </AppProvider>,
    );
    await waitFor(() => expect(ipcMock.getSnapshot).toHaveBeenCalled());
    await act(async () => {});
    expect(screen.getByText(/No Bose headphones connected/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Allow Bluetooth access' })).not.toBeInTheDocument();
  });
});

describe('settings on Android', () => {
  async function renderSettings() {
    render(
      <AppProvider>
        <SettingsPage />
      </AppProvider>,
    );
    await waitFor(() => expect(ipcMock.getSettings).toHaveBeenCalled());
    await act(async () => {});
  }

  it('leave out what only a desktop has', async () => {
    useAgent(ANDROID_UA);
    await renderSettings();
    expect(screen.queryByRole('switch', { name: 'Start with Windows' })).not.toBeInTheDocument();
    // A phone has no desktop to put a widget on.
    expect(screen.queryByRole('switch', { name: 'Desktop widget' })).not.toBeInTheDocument();
    expect(screen.queryByRole('switch', { name: 'Start minimized' })).not.toBeInTheDocument();
    expect(screen.queryByRole('switch', { name: 'Minimize to tray' })).not.toBeInTheDocument();
    expect(screen.getByRole('switch', { name: 'Auto-connect' })).toBeInTheDocument();
  });

  it('offer the background switch under the name of what it does', async () => {
    useAgent(ANDROID_UA);
    ipcMock.setSetting.mockResolvedValue(settings({ minimizeToTray: true }));
    await renderSettings();

    const keep = screen.getByRole('switch', { name: 'Keep running in the background' });
    await userEvent.click(keep);

    // The same setting as the tray one, by its own name underneath.
    await waitFor(() => expect(ipcMock.setSetting).toHaveBeenCalledWith('minimizeToTray', true));
    await waitFor(() => expect(keep).toHaveAttribute('aria-checked', 'true'));
  });

  it('are unchanged on a desktop', async () => {
    useAgent(DESKTOP_UA);
    await renderSettings();
    expect(screen.getByRole('switch', { name: 'Start with Windows' })).toBeInTheDocument();
    expect(screen.getByRole('switch', { name: 'Desktop widget' })).toBeInTheDocument();
    expect(screen.getByRole('switch', { name: 'Minimize to tray' })).toBeInTheDocument();
    expect(
      screen.queryByRole('switch', { name: 'Keep running in the background' }),
    ).not.toBeInTheDocument();
  });
});

describe('a missing voice model on Android', () => {
  it('is not answered with a PowerShell command', async () => {
    useAgent(ANDROID_UA);
    render(
      <AppProvider>
        <VoiceFeedbackPanel />
      </AppProvider>,
    );
    await waitFor(() => expect(ipcMock.getVoiceStatus).toHaveBeenCalled());
    await act(async () => {});

    const install = screen.getByTestId('voice-install');
    expect(install).toHaveTextContent('built without it');
    expect(install).not.toHaveTextContent('powershell');
    expect(install).not.toHaveTextContent('install-voice-model');
    expect(screen.getByText(/phone and headphone volume are left alone/)).toBeInTheDocument();
  });

  it('is not described in terms of Windows or a computer', async () => {
    useAgent(ANDROID_UA);
    const { container } = render(
      <AppProvider>
        <VoiceFeedbackPanel />
      </AppProvider>,
    );
    await waitFor(() => expect(ipcMock.getVoiceStatus).toHaveBeenCalled());
    await act(async () => {});

    expect(container).not.toHaveTextContent(/Windows/);
    expect(container).not.toHaveTextContent(/this computer/);
    expect(screen.getByText(/Synthesized on this phone/)).toBeInTheDocument();
  });
});

// Taken out of the phone build first, and then of the desktop one at the
// owner's request, so that the two are the same application.
describe.each([
  ['a phone', ANDROID_UA],
  ['a desktop', DESKTOP_UA],
])('what the application no longer has, on %s', (_where, agent) => {
  async function renderPage(page: React.ReactNode) {
    useAgent(agent);
    render(<AppProvider>{page}</AppProvider>);
    await waitFor(() => expect(ipcMock.getSnapshot).toHaveBeenCalled());
    await act(async () => {});
  }

  it('has no Diagnostics page to go to', () => {
    expect(Object.keys(pageTitles)).not.toContain('diagnostics');

    useAgent(agent);
    render(<BottomNav page="noise" onNavigate={vi.fn()} />);
    expect(screen.queryByRole('button', { name: 'Diagnostics' })).not.toBeInTheDocument();
    for (const name of ['Dashboard', 'Device', 'Noise Control', 'Equalizer', 'Profiles', 'Settings']) {
      expect(screen.getByRole('button', { name })).toBeInTheDocument();
    }
  });

  it('has no way to Diagnostics from the Device page, and no card at its top', async () => {
    await renderPage(<DevicePage />);
    expect(screen.queryByRole('button', { name: /Diagnostics/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Disconnect' })).not.toBeInTheDocument();
    expect(screen.getByText('Device Information')).toBeInTheDocument();
  });

  it('has no diagnostics settings', async () => {
    await renderPage(<SettingsPage />);
    expect(screen.queryByText('Logging level')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Export Logs' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Clear Logs' })).not.toBeInTheDocument();
  });

  it('shows the equalizer as the headphones’ own, with no software panel beside it', async () => {
    await renderPage(<EqualizerPage />);
    expect(screen.queryByText('Software EQ')).not.toBeInTheDocument();
    expect(screen.queryByText('Windows Audio')).not.toBeInTheDocument();
    expect(screen.getByText('Bass')).toBeInTheDocument();
    expect(pageTitles.equalizer.subtitle).not.toMatch(/software/i);
  });
});

describe('a phone', () => {
  it('never says Windows on the Device or Equalizer pages', async () => {
    useAgent(ANDROID_UA);
    render(
      <AppProvider>
        <DevicePage />
        <EqualizerPage />
      </AppProvider>,
    );
    await waitFor(() => expect(ipcMock.getSnapshot).toHaveBeenCalled());
    await act(async () => {});
    expect(document.body.textContent).not.toMatch(/Windows/);
  });
});

describe('bottom navigation', () => {
  it('offers every section and says which one is open', async () => {
    const onNavigate = vi.fn();
    render(<BottomNav page="noise" onNavigate={onNavigate} />);

    const names = ['Dashboard', 'Device', 'Noise Control', 'Equalizer', 'Profiles', 'Settings'];
    for (const name of names) {
      expect(screen.getByRole('button', { name })).toBeInTheDocument();
    }
    expect(screen.getByRole('button', { name: 'Noise Control' })).toHaveAttribute('aria-current', 'page');
    expect(screen.getByRole('button', { name: 'Settings' })).not.toHaveAttribute('aria-current');

    await userEvent.click(screen.getByRole('button', { name: 'Equalizer' }));
    expect(onNavigate).toHaveBeenCalledWith('equalizer');
  });
});

describe('a slider under a finger', () => {
  function renderRange(onChange: (v: number) => void) {
    render(<Range label="Level" value={4} min={0} max={10} onChange={onChange} />);
    return screen.getByRole('slider', { name: 'Level' }) as HTMLInputElement;
  }

  it('commits when the finger lifts, with no pointer-up at all', () => {
    const onChange = vi.fn();
    const slider = renderRange(onChange);
    fireEvent.change(slider, { target: { value: '7' } });
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.touchEnd(slider);
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith(7);
  });

  // Both events arrive on some phones. The slider drives a hardware command,
  // and that command must go out once.
  it('commits once when letting go is reported twice', () => {
    const onChange = vi.fn();
    const slider = renderRange(onChange);
    fireEvent.change(slider, { target: { value: '7' } });
    fireEvent.pointerUp(slider);
    fireEvent.touchEnd(slider);
    fireEvent.keyUp(slider);
    expect(onChange).toHaveBeenCalledTimes(1);
  });

  it('still commits a second, different value straight away', () => {
    const onChange = vi.fn();
    const slider = renderRange(onChange);
    fireEvent.change(slider, { target: { value: '7' } });
    fireEvent.touchEnd(slider);
    fireEvent.change(slider, { target: { value: '9' } });
    fireEvent.touchEnd(slider);
    expect(onChange.mock.calls).toEqual([[7], [9]]);
  });

  it('sends nothing for a touch that did not move it', () => {
    const onChange = vi.fn();
    const slider = renderRange(onChange);
    fireEvent.touchEnd(slider);
    fireEvent.pointerUp(slider);
    expect(onChange).not.toHaveBeenCalled();
  });
});
