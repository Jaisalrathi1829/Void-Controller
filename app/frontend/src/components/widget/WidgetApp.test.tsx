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
  executeCommand: vi.fn(),
  setNoiseControlMode: vi.fn(),
  onDeviceChanged: vi.fn(),
  onSettingsChanged: vi.fn(),
  getVoiceStatus: vi.fn(),
  onVoiceStatus: vi.fn(),
  showMainWindow: vi.fn(),
}));
vi.mock('@/services/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/services/ipc')>()),
  hasNativeLayer: () => true,
  ...ipcMock,
}));

import { isWidgetWindow, unavailableSettings, voiceUnavailable } from '@/services/ipc';
import { WidgetApp } from './WidgetApp';

function settings(over: Partial<AppSettings> = {}): AppSettings {
  return {
    ...unavailableSettings('unused'),
    unavailable: {},
    desktopWidget: true,
    widgetOnTop: true,
    ...over,
  };
}

function snapshot(over: Partial<DeviceSnapshot> = {}): DeviceSnapshot {
  const capabilities = createUnknownCapabilities('not investigated');
  for (const key of ['noiseControl', 'awareMode', 'customNoiseControl', 'equalizer'] as const) {
    capabilities[key] = { ...capabilities[key], status: 'verified' };
  }
  return {
    source: 'real',
    connection: 'connected',
    transport: 'bluetooth-classic',
    identity: { name: 'Aurora', id: 'test' },
    capabilities,
    battery: { percent: 64, source: 'windows-pnp', readAt: '2026-10-12T16:00:00Z' },
    noiseControl: { mode: 'quiet', source: 'vendor-protocol', readAt: '2026-10-12T16:00:00Z' },
    equalizer: {
      bass: 4,
      mid: 1,
      treble: -2,
      source: 'vendor-protocol',
      readAt: '2026-10-12T16:00:00Z',
    },
    systemAudio: null,
    hostName: 'Nemesis',
    lastError: null,
    updatedAt: '2026-10-12T16:00:00Z',
    ...over,
  };
}

let settingsChanged: ((s: AppSettings) => void) | undefined;

beforeEach(() => {
  vi.clearAllMocks();
  settingsChanged = undefined;
  ipcMock.getSnapshot.mockResolvedValue(snapshot());
  ipcMock.getSettings.mockResolvedValue(settings());
  ipcMock.setSetting.mockImplementation(async (key: string, value: boolean) =>
    settings({ [key]: value }),
  );
  ipcMock.getVoiceStatus.mockResolvedValue(voiceUnavailable('not under test'));
  ipcMock.onVoiceStatus.mockResolvedValue(() => {});
  ipcMock.onDeviceChanged.mockResolvedValue(() => {});
  ipcMock.onSettingsChanged.mockImplementation(async (handler: (s: AppSettings) => void) => {
    settingsChanged = handler;
    return () => {};
  });
  ipcMock.executeCommand.mockResolvedValue({ kind: 'applied' });
  ipcMock.setNoiseControlMode.mockResolvedValue({ kind: 'applied' });
  ipcMock.showMainWindow.mockResolvedValue(undefined);
});

afterEach(() => {
  window.location.hash = '';
});

async function renderWidget() {
  render(<WidgetApp />);
  await waitFor(() => expect(ipcMock.getSnapshot).toHaveBeenCalled());
  await act(async () => {});
}

/** Lets a command that has been answered finish putting the screen right. */
async function settled() {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}

describe('what the widget shows', () => {
  it('is the headphones by name, their battery, the modes and the three bands', async () => {
    await renderWidget();
    expect(screen.getByText('Aurora')).toBeInTheDocument();
    expect(screen.getByText('64%')).toBeInTheDocument();
    expect(screen.getByRole('tab', { name: 'Quiet' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('tab', { name: 'Aware' })).toHaveAttribute('aria-selected', 'false');
    expect((screen.getByRole('slider', { name: 'Bass' }) as HTMLInputElement).value).toBe('4');
    expect((screen.getByRole('slider', { name: 'Mid' }) as HTMLInputElement).value).toBe('1');
    expect((screen.getByRole('slider', { name: 'Treble' }) as HTMLInputElement).value).toBe('-2');
    expect(screen.getByText('+4')).toBeInTheDocument();
    expect(screen.getByText('-2')).toBeInTheDocument();
  });

  it('calls the custom mode by the name the headphones give it, once they are in it', async () => {
    ipcMock.getSnapshot.mockResolvedValue(
      snapshot({
        noiseControl: {
          mode: 'custom',
          label: 'Focus',
          level: 4,
          source: 'vendor-protocol',
          readAt: '2026-10-12T16:00:00Z',
        },
      }),
    );
    await renderWidget();
    expect(screen.getByRole('tab', { name: 'Focus' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.queryByRole('tab', { name: 'Custom' })).not.toBeInTheDocument();
  });

  // No mode is shown as active that the headphones have not reported.
  it('selects no mode when the headphones have not said which they are in', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ noiseControl: null }));
    await renderWidget();
    for (const name of ['Quiet', 'Aware', 'Custom']) {
      expect(screen.getByRole('tab', { name })).toHaveAttribute('aria-selected', 'false');
    }
  });

  it('says so when the headphones are not connected, and offers no controls', async () => {
    ipcMock.getSnapshot.mockResolvedValue(
      snapshot({ connection: 'disconnected', noiseControl: null, equalizer: null, battery: null }),
    );
    await renderWidget();
    expect(screen.getByText('Headphones not connected')).toBeInTheDocument();
    expect(screen.queryByRole('tab')).not.toBeInTheDocument();
    expect(screen.queryByRole('slider')).not.toBeInTheDocument();
  });

  // The simulated device is labelled wherever it appears, however small.
  it('labels the simulated device as simulated', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ source: 'mock' }));
    await renderWidget();
    expect(screen.getByText('Simulated')).toBeInTheDocument();
  });

  it('does not label real headphones as simulated', async () => {
    await renderWidget();
    expect(screen.queryByText('Simulated')).not.toBeInTheDocument();
  });

  it('greys out what the headphones have not been shown to support', async () => {
    const base = snapshot();
    const capabilities = { ...base.capabilities };
    capabilities.equalizer = { ...capabilities.equalizer, status: 'unknown' };
    ipcMock.getSnapshot.mockResolvedValue({ ...base, capabilities });
    await renderWidget();
    expect(screen.getByRole('slider', { name: 'Bass' })).toBeDisabled();
    expect(screen.getByRole('tab', { name: 'Aware' })).toBeEnabled();
  });
});

describe('changing things from the widget', () => {
  it('asks for a mode by name, once, and shows what the headphones then report', async () => {
    await renderWidget();
    ipcMock.getSnapshot.mockResolvedValue(
      snapshot({
        noiseControl: { mode: 'aware', source: 'vendor-protocol', readAt: '2026-10-12T16:00:05Z' },
      }),
    );

    await userEvent.click(screen.getByRole('tab', { name: 'Aware' }));

    await waitFor(() => expect(ipcMock.setNoiseControlMode).toHaveBeenCalledTimes(1));
    expect(ipcMock.setNoiseControlMode).toHaveBeenCalledWith('aware');
    await waitFor(() =>
      expect(screen.getByRole('tab', { name: 'Aware' })).toHaveAttribute('aria-selected', 'true'),
    );
    expect(screen.getByTestId('widget-status')).toHaveTextContent(/confirmed/);
    await settled();
  });

  // The mode shown is the one reported, not the one asked for.
  it('does not move the selection when the headphones did not change', async () => {
    ipcMock.setNoiseControlMode.mockResolvedValue({
      kind: 'sent-unverified',
      reason: 'The headphones still report Quiet.',
    });
    await renderWidget();

    await userEvent.click(screen.getByRole('tab', { name: 'Aware' }));

    await waitFor(() =>
      expect(screen.getByTestId('widget-status')).toHaveTextContent(/unverified/),
    );
    expect(screen.getByRole('tab', { name: 'Quiet' })).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByRole('tab', { name: 'Aware' })).toHaveAttribute('aria-selected', 'false');
    await settled();
  });

  it('sets one band and leaves the other two as the headphones have them', async () => {
    await renderWidget();
    const bass = screen.getByRole('slider', { name: 'Bass' });
    fireEvent.change(bass, { target: { value: '7' } });
    expect(ipcMock.executeCommand).not.toHaveBeenCalled();
    fireEvent.pointerUp(bass);

    await waitFor(() => expect(ipcMock.executeCommand).toHaveBeenCalledTimes(1));
    expect(ipcMock.executeCommand).toHaveBeenCalledWith({
      kind: 'setEqualizer',
      settings: { bass: 7, mid: 1, treble: -2 },
    });
    await settled();
  });

  it('says what went wrong in its own status line', async () => {
    ipcMock.executeCommand.mockRejectedValue({
      kind: 'control-link',
      message: 'The headphones did not answer.',
    });
    await renderWidget();
    const treble = screen.getByRole('slider', { name: 'Treble' });
    fireEvent.change(treble, { target: { value: '3' } });
    fireEvent.pointerUp(treble);
    await waitFor(() =>
      expect(screen.getByTestId('widget-status')).toHaveTextContent(
        /The headphones did not answer/,
      ),
    );
    await settled();
  });
});

describe("the widget's own buttons", () => {
  it('hides itself by switching the widget off', async () => {
    await renderWidget();
    await userEvent.click(screen.getByRole('button', { name: 'Hide the widget' }));
    expect(ipcMock.setSetting).toHaveBeenCalledWith('desktopWidget', false);
  });

  it('opens the main window', async () => {
    await renderWidget();
    await userEvent.click(screen.getByRole('button', { name: 'Open Bose QC Control Center' }));
    expect(ipcMock.showMainWindow).toHaveBeenCalledTimes(1);
  });

  it('pins and unpins itself, and shows which it is', async () => {
    await renderWidget();
    const pinned = screen.getByRole('button', { name: 'Stop keeping on top' });
    expect(pinned).toHaveAttribute('aria-pressed', 'true');

    await userEvent.click(pinned);

    await waitFor(() => expect(ipcMock.setSetting).toHaveBeenCalledWith('widgetOnTop', false));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Keep on top' })).toHaveAttribute(
        'aria-pressed',
        'false',
      ),
    );
  });

  // The switch can be flipped from the main window's Settings page too.
  it('follows a setting changed in the other window', async () => {
    await renderWidget();
    await waitFor(() => expect(settingsChanged).toBeDefined());
    await act(async () => settingsChanged?.(settings({ widgetOnTop: false })));
    expect(screen.getByRole('button', { name: 'Keep on top' })).toBeInTheDocument();
  });
});

describe('which window this is', () => {
  it('is the widget when the native layer says the window is called that', () => {
    const tauri = window as unknown as { __TAURI_INTERNALS__?: unknown };
    tauri.__TAURI_INTERNALS__ = { metadata: { currentWindow: { label: 'widget' } } };
    expect(isWidgetWindow()).toBe(true);
    tauri.__TAURI_INTERNALS__ = { metadata: { currentWindow: { label: 'main' } } };
    expect(isWidgetWindow()).toBe(false);
    // Under the native layer the address says nothing about it.
    window.location.hash = '#widget';
    expect(isWidgetWindow()).toBe(false);
    delete tauri.__TAURI_INTERNALS__;
  });
});
