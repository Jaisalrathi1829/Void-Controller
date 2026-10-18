import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createUnknownCapabilities } from '@/types/capability';
import type { DeviceSnapshot } from '@/types/device';
import type { AppSettings, SettingKey, VoiceStatus } from '@/services/ipc';

const toast = vi.hoisted(() => ({ success: vi.fn(), warning: vi.fn(), error: vi.fn() }));
vi.mock('sonner', () => ({ toast }));

const ipcMock = vi.hoisted(() => ({
  getSnapshot: vi.fn(),
  getSettings: vi.fn(),
  setSetting: vi.fn(),
  onDeviceChanged: vi.fn(),
  getVoiceStatus: vi.fn(),
  onVoiceStatus: vi.fn(),
  setVoiceOption: vi.fn(),
  testVoice: vi.fn(),
  stopSpeaking: vi.fn(),
  recheckVoiceModel: vi.fn(),
}));
vi.mock('@/services/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/services/ipc')>()),
  hasNativeLayer: () => true,
  ...ipcMock,
}));

import { AppProvider } from '@/store';
import { VoiceFeedbackPanel } from './VoiceFeedbackPanel';

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
    voiceEnabled: true,
    announceModeChanges: true,
    announceConnection: true,
    announceBattery: true,
    voiceUseDefaultOutput: false,
    desktopWidget: false,
    widgetOnTop: true,
    rememberLastSeen: false,
    voiceId: 'af_bella',
    voiceSpeedPercent: 100,
    voiceVolumePercent: 80,
    preferredDeviceName: 'Aurora',
    unavailable: {},
    ...over,
  };
}

const VOICES = [
  { id: 'af_bella', label: 'Bella — American, female' },
  { id: 'am_adam', label: 'Adam — American, male' },
  { id: 'bm_george', label: 'George — British, male' },
];

function status(over: Partial<VoiceStatus> = {}): VoiceStatus {
  return {
    state: 'ready',
    detail: 'Ready.',
    model: 'kokoro-int8-en-v0_19',
    assetsDir: 'C:\\Users\\me\\AppData\\Local\\app\\voice\\kokoro-int8-en-v0_19',
    voices: VOICES,
    speaking: null,
    isTest: false,
    output: null,
    modelLoaded: false,
    ...over,
  };
}

function notInstalled(): VoiceStatus {
  return status({
    state: 'not-installed',
    detail: 'The voice model is not installed. Expected it at C:\\voice.',
    voices: [],
  });
}

const snapshot: DeviceSnapshot = {
  source: 'real',
  connection: 'connected',
  transport: 'bluetooth-classic',
  identity: { name: 'Aurora', id: 'test' },
  capabilities: createUnknownCapabilities('not investigated'),
  battery: null,
  noiseControl: { mode: 'quiet', source: 'vendor-protocol', readAt: '2026-10-10T09:00:00Z' },
  equalizer: null,
  systemAudio: null,
  lastError: null,
  updatedAt: '2026-10-10T09:00:00Z',
};

/** The handler the store registered for native status changes. */
let pushStatus: ((s: VoiceStatus) => void) | undefined;

async function renderPanel() {
  render(
    <AppProvider>
      <VoiceFeedbackPanel />
    </AppProvider>,
  );
  await waitFor(() => expect(ipcMock.getVoiceStatus).toHaveBeenCalled());
  await waitFor(() => expect(ipcMock.getSettings).toHaveBeenCalled());
  await act(async () => {});
}

const toggle = (name: string) => screen.getByRole('switch', { name });
const button = (name: string) => screen.getByRole('button', { name });
const voiceSelect = () => screen.getByLabelText('Voice') as HTMLSelectElement;
const slider = (name: string) => screen.getByRole('slider', { name }) as HTMLInputElement;

/** Drags a slider to a value and lets go, as the Range control expects. */
function drag(el: HTMLInputElement, value: number) {
  fireEvent.change(el, { target: { value: String(value) } });
  fireEvent.pointerUp(el);
}

beforeEach(() => {
  vi.clearAllMocks();
  pushStatus = undefined;
  ipcMock.getSnapshot.mockResolvedValue(snapshot);
  ipcMock.onDeviceChanged.mockResolvedValue(() => {});
  ipcMock.getSettings.mockResolvedValue(settings());
  ipcMock.getVoiceStatus.mockResolvedValue(status());
  ipcMock.onVoiceStatus.mockImplementation(async (handler: (s: VoiceStatus) => void) => {
    pushStatus = handler;
    return () => {};
  });
  ipcMock.testVoice.mockResolvedValue(undefined);
  ipcMock.stopSpeaking.mockResolvedValue(undefined);
});

describe('model status', () => {
  it.each<[VoiceStatus['state'], string]>([
    ['not-installed', 'Not installed'],
    ['initializing', 'Initializing'],
    ['ready', 'Ready'],
    ['synthesizing', 'Synthesizing'],
    ['playing', 'Playing'],
    ['unavailable', 'Unavailable'],
    ['error', 'Error'],
  ])('%s is shown as "%s" with the native explanation', async (state, label) => {
    ipcMock.getVoiceStatus.mockResolvedValue(status({ state, detail: `detail for ${state}` }));
    await renderPanel();
    expect(screen.getByTestId('voice-state')).toHaveTextContent(label);
    expect(screen.getByTestId('voice-detail')).toHaveTextContent(`detail for ${state}`);
  });

  it('follows the native layer as it changes, without being asked', async () => {
    await renderPanel();
    await waitFor(() => expect(pushStatus).toBeDefined());

    await act(async () => {
      pushStatus?.(status({ state: 'playing', detail: 'Speaking through Headphones (Aurora).' }));
    });
    expect(screen.getByTestId('voice-state')).toHaveTextContent('Playing');
    expect(screen.getByTestId('voice-detail')).toHaveTextContent('Headphones (Aurora)');

    await act(async () => {
      pushStatus?.(status({ state: 'error', detail: 'Playback through Speakers failed: gone' }));
    });
    expect(screen.getByTestId('voice-state')).toHaveTextContent('Error');
    expect(screen.getByTestId('voice-detail')).toHaveTextContent('Playback through Speakers failed');
  });

  it('says so, rather than showing Ready, when the status cannot be read', async () => {
    ipcMock.getVoiceStatus.mockRejectedValue({ kind: 'platform', message: 'no voice service' });
    await renderPanel();
    expect(screen.getByTestId('voice-state')).toHaveTextContent('Unavailable');
    expect(screen.getByTestId('voice-detail')).toHaveTextContent('no voice service');
    expect(button('Test Voice')).toBeDisabled();
  });
});

describe('a missing voice model', () => {
  beforeEach(() => {
    ipcMock.getVoiceStatus.mockResolvedValue(notInstalled());
  });

  it('is explained, with the command to install it and where it goes', async () => {
    await renderPanel();
    const install = screen.getByTestId('voice-install');
    expect(install).toHaveTextContent('install-voice-model.ps1');
    expect(install).toHaveTextContent('not fetched automatically');
    expect(install).toHaveTextContent('kokoro-int8-en-v0_19');
  });

  it('offers no voices and no way to speak', async () => {
    await renderPanel();
    expect(voiceSelect()).toBeDisabled();
    expect(within(voiceSelect()).getAllByRole('option')).toHaveLength(1);
    expect(voiceSelect()).toHaveTextContent('No voice model installed');
    expect(button('Test Voice')).toBeDisabled();
    expect(slider('Speed')).toBeDisabled();
    expect(slider('Speech volume')).toBeDisabled();

    await userEvent.click(button('Test Voice'));
    expect(ipcMock.testVoice).not.toHaveBeenCalled();
  });

  it('is looked for again on request, and the result shown', async () => {
    ipcMock.recheckVoiceModel.mockResolvedValue(status());
    await renderPanel();

    await userEvent.click(button('Check again'));

    await waitFor(() => expect(ipcMock.recheckVoiceModel).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.getByTestId('voice-state')).toHaveTextContent('Ready'));
    expect(screen.queryByTestId('voice-install')).not.toBeInTheDocument();
    expect(button('Test Voice')).toBeEnabled();
  });

  it('is not shown as installable once the model is there', async () => {
    ipcMock.getVoiceStatus.mockResolvedValue(status());
    await renderPanel();
    expect(screen.queryByTestId('voice-install')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Check again' })).not.toBeInTheDocument();
  });
});

describe('voice, speed and volume', () => {
  it('offers exactly the voices the installed model reports', async () => {
    await renderPanel();
    const options = within(voiceSelect()).getAllByRole('option') as HTMLOptionElement[];
    expect(options.map((o) => o.value)).toEqual(['af_bella', 'am_adam', 'bm_george']);
    expect(voiceSelect().value).toBe('af_bella');
  });

  it('asks the native layer for a chosen voice and shows what it answers', async () => {
    ipcMock.setVoiceOption.mockResolvedValue(settings({ voiceId: 'am_adam' }));
    await renderPanel();

    await userEvent.selectOptions(voiceSelect(), 'am_adam');

    await waitFor(() =>
      expect(ipcMock.setVoiceOption).toHaveBeenCalledWith({ kind: 'voice', id: 'am_adam' }),
    );
    await waitFor(() => expect(voiceSelect().value).toBe('am_adam'));
  });

  it('sends the speed once, when the slider is let go', async () => {
    ipcMock.setVoiceOption.mockResolvedValue(settings({ voiceSpeedPercent: 150 }));
    await renderPanel();
    const speed = slider('Speed');
    expect(speed.min).toBe('50');
    expect(speed.max).toBe('200');

    fireEvent.change(speed, { target: { value: '120' } });
    fireEvent.change(speed, { target: { value: '150' } });
    expect(ipcMock.setVoiceOption).not.toHaveBeenCalled();
    fireEvent.pointerUp(speed);

    await waitFor(() => expect(ipcMock.setVoiceOption).toHaveBeenCalledTimes(1));
    expect(ipcMock.setVoiceOption).toHaveBeenCalledWith({ kind: 'speed', percent: 150 });
    await waitFor(() => expect(screen.getByText('1.50×')).toBeInTheDocument());
  });

  it('sends the volume as a speech-only setting', async () => {
    ipcMock.setVoiceOption.mockResolvedValue(settings({ voiceVolumePercent: 40 }));
    await renderPanel();

    drag(slider('Speech volume'), 40);

    await waitFor(() =>
      expect(ipcMock.setVoiceOption).toHaveBeenCalledWith({ kind: 'volume', percent: 40 }),
    );
    expect(screen.getByText(/Windows and headphone volume are left alone/)).toBeInTheDocument();
  });

  it('shows the stored value again when a change is refused', async () => {
    ipcMock.setVoiceOption.mockRejectedValue({
      kind: 'invalid-input',
      message: 'There is no voice called "am_adam".',
    });
    await renderPanel();

    await userEvent.selectOptions(voiceSelect(), 'am_adam');

    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    expect(toast.error).toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({ description: 'There is no voice called "am_adam".' }),
    );
    await waitFor(() => expect(voiceSelect().value).toBe('af_bella'));
  });
});

describe('Test Voice and Stop Speaking', () => {
  it('asks the native layer to speak the sample, passing nothing', async () => {
    await renderPanel();
    await userEvent.click(button('Test Voice'));
    await waitFor(() => expect(ipcMock.testVoice).toHaveBeenCalledTimes(1));
    expect(ipcMock.testVoice).toHaveBeenCalledWith();
  });

  it('does not claim to be speaking until the native layer says so', async () => {
    await renderPanel();
    await userEvent.click(button('Test Voice'));
    await waitFor(() => expect(ipcMock.testVoice).toHaveBeenCalled());

    expect(screen.getByTestId('voice-state')).toHaveTextContent('Ready');
    expect(screen.queryByTestId('voice-speaking')).not.toBeInTheDocument();
  });

  it('works with voice feedback switched off, to choose a voice first', async () => {
    ipcMock.getSettings.mockResolvedValue(settings({ voiceEnabled: false }));
    await renderPanel();
    expect(button('Test Voice')).toBeEnabled();
    await userEvent.click(button('Test Voice'));
    await waitFor(() => expect(ipcMock.testVoice).toHaveBeenCalledTimes(1));
  });

  it('reports a test that could not be started', async () => {
    ipcMock.testVoice.mockRejectedValue({
      kind: 'voice-unavailable',
      message: 'The voice model is not installed.',
    });
    await renderPanel();
    await userEvent.click(button('Test Voice'));
    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    expect(toast.error).toHaveBeenCalledWith(
      expect.any(String),
      expect.objectContaining({ description: 'The voice model is not installed.' }),
    );
  });

  it('labels test speech as a test and a real announcement as an announcement', async () => {
    await renderPanel();
    await waitFor(() => expect(pushStatus).toBeDefined());

    await act(async () => {
      pushStatus?.(
        status({ state: 'playing', speaking: 'Voice feedback is ready.', isTest: true }),
      );
    });
    expect(screen.getByTestId('voice-speaking')).toHaveTextContent(
      'Test: “Voice feedback is ready.”',
    );

    await act(async () => {
      pushStatus?.(status({ state: 'playing', speaking: 'Quiet mode activated.', isTest: false }));
    });
    expect(screen.getByTestId('voice-speaking')).toHaveTextContent(
      'Announcement: “Quiet mode activated.”',
    );
  });

  it.each<VoiceStatus['state']>(['initializing', 'synthesizing', 'playing'])(
    'can stop while %s',
    async (state) => {
      ipcMock.getVoiceStatus.mockResolvedValue(status({ state }));
      await renderPanel();
      expect(button('Stop Speaking')).toBeEnabled();
      await userEvent.click(button('Stop Speaking'));
      await waitFor(() => expect(ipcMock.stopSpeaking).toHaveBeenCalledTimes(1));
    },
  );

  it('has nothing to stop when nothing is being said', async () => {
    await renderPanel();
    expect(button('Stop Speaking')).toBeDisabled();
  });

  it('names the output last spoken through', async () => {
    ipcMock.getVoiceStatus.mockResolvedValue(status({ output: 'Headphones (Aurora)' }));
    await renderPanel();
    expect(screen.getByTestId('voice-output')).toHaveTextContent('Headphones (Aurora)');
  });
});

describe('what is announced', () => {
  it.each<[string, SettingKey]>([
    ['Voice feedback', 'voiceEnabled'],
    ['Mode changes', 'announceModeChanges'],
    ['Connection and disconnection', 'announceConnection'],
    ['Low battery warning', 'announceBattery'],
  ])('%s is its own switch', async (name, key) => {
    ipcMock.setSetting.mockResolvedValue(settings({ [key]: false }));
    await renderPanel();
    expect(toggle(name)).toHaveAttribute('aria-checked', 'true');

    await userEvent.click(toggle(name));

    await waitFor(() => expect(ipcMock.setSetting).toHaveBeenCalledWith(key, false));
    expect(ipcMock.setSetting).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(toggle(name)).toHaveAttribute('aria-checked', 'false'));
  });

  it('the fallback output is off unless it has been switched on', async () => {
    ipcMock.setSetting.mockResolvedValue(settings({ voiceUseDefaultOutput: true }));
    await renderPanel();
    const name = 'Use another output when the headphones are unavailable';
    expect(toggle(name)).toHaveAttribute('aria-checked', 'false');

    await userEvent.click(toggle(name));
    await waitFor(() =>
      expect(ipcMock.setSetting).toHaveBeenCalledWith('voiceUseDefaultOutput', true),
    );
  });

  it('the kinds of announcement wait on the main switch', async () => {
    ipcMock.getSettings.mockResolvedValue(settings({ voiceEnabled: false }));
    await renderPanel();
    expect(toggle('Voice feedback')).toBeEnabled();
    for (const name of [
      'Mode changes',
      'Connection and disconnection',
      'Low battery warning',
    ]) {
      expect(toggle(name)).toBeDisabled();
    }
  });

  it('names the headphones in the connection sentences', async () => {
    await renderPanel();
    expect(screen.getByText(/“Aurora connected to …”, naming this computer/)).toBeInTheDocument();
    expect(screen.getByText(/“Aurora disconnected.”/)).toBeInTheDocument();
  });

  // The custom mode is announced like the other two, by its own name, and
  // the switch says so rather than naming only Quiet and Aware.
  it('says that a custom mode is announced by its own name', async () => {
    await renderPanel();
    expect(screen.getByText(/a custom mode by the name the headphones give it/)).toBeInTheDocument();
  });

  it('does not invent a name when no headphones are remembered', async () => {
    ipcMock.getSettings.mockResolvedValue(settings({ preferredDeviceName: null }));
    ipcMock.getSnapshot.mockResolvedValue({ ...snapshot, identity: null });
    await renderPanel();
    expect(screen.getByText(/The headphones’ name, then “connected to”/)).toBeInTheDocument();
    expect(screen.queryByText(/No device/)).not.toBeInTheDocument();
  });
});

describe('before the settings are known', () => {
  it('every control is inert', async () => {
    ipcMock.getSettings.mockReturnValue(new Promise(() => {}));
    render(
      <AppProvider>
        <VoiceFeedbackPanel />
      </AppProvider>,
    );
    await act(async () => {});

    for (const el of screen.getAllByRole('switch')) expect(el).toBeDisabled();
    expect(button('Test Voice')).toBeDisabled();
    expect(button('Stop Speaking')).toBeDisabled();
    expect(voiceSelect()).toBeDisabled();
    expect(slider('Speed')).toBeDisabled();
  });
});
