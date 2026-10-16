import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createUnknownCapabilities, type CapabilityStatus } from '@/types/capability';
import {
  checkDeviceName,
  MAX_DEVICE_NAME_BYTES,
  type CommandOutcome,
  type DeviceSnapshot,
  type PairedDevice,
} from '@/types/device';

const toast = vi.hoisted(() => ({
  success: vi.fn(),
  warning: vi.fn(),
  error: vi.fn(),
  info: vi.fn(),
}));
vi.mock('sonner', () => ({ toast }));

const ipcMock = vi.hoisted(() => ({
  getSnapshot: vi.fn(),
  executeCommand: vi.fn(),
}));
vi.mock('@/services/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/services/ipc')>()),
  hasNativeLayer: () => true,
  ...ipcMock,
}));

import { AppProvider } from '@/store';
import { DevicePage } from './DevicePage';

const DESKTOP_UA = window.navigator.userAgent;
const ANDROID_UA =
  'Mozilla/5.0 (Linux; Android 14; SM-S928B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Mobile Safari/537.36';

function useAgent(value: string) {
  Object.defineProperty(window.navigator, 'userAgent', { value, configurable: true });
}

const PHONE: PairedDevice = {
  id: '0000000000000000aaaa',
  name: "Jaisal's S24 Ultra",
  connected: true,
  thisDevice: true,
};
const LAPTOP: PairedDevice = {
  id: '0000000000000000bbbb',
  name: 'NEMESIS',
  connected: true,
  thisDevice: false,
};
const TABLET: PairedDevice = {
  id: '0000000000000000cccc',
  name: 'Old tablet',
  connected: false,
  thisDevice: false,
};

/** Real headphones, connected, as the native layer describes them. */
function snapshot(
  opts: {
    name?: string;
    multipoint?: boolean | null;
    paired?: PairedDevice[] | null;
    multipointCap?: CapabilityStatus;
    renameCap?: CapabilityStatus;
    connectionsCap?: CapabilityStatus;
    connection?: DeviceSnapshot['connection'];
  } = {},
): DeviceSnapshot {
  const {
    name = 'Aurora',
    multipoint = true,
    paired = [PHONE, LAPTOP],
    multipointCap = 'ready',
    renameCap = 'ready',
    connectionsCap = 'ready',
    connection = 'connected',
  } = opts;
  const capabilities = createUnknownCapabilities('not investigated');
  capabilities.multipoint = {
    ...capabilities.multipoint,
    status: multipointCap,
    mechanism: 'rfcomm-vendor',
    evidence: `multipoint evidence for ${multipointCap}`,
  };
  capabilities.deviceRename = {
    ...capabilities.deviceRename,
    status: renameCap,
    mechanism: 'rfcomm-vendor',
    evidence: `rename evidence for ${renameCap}`,
  };
  capabilities.pairedDevices = {
    ...capabilities.pairedDevices,
    status: connectionsCap,
    mechanism: 'rfcomm-vendor',
    evidence: `connections evidence for ${connectionsCap}`,
  };
  return {
    source: 'real',
    connection,
    transport: connection === 'connected' ? 'bluetooth-classic' : 'none',
    identity: connection === 'connected' ? { name, id: 'test' } : null,
    capabilities,
    battery: null,
    noiseControl: null,
    equalizer: null,
    multipoint:
      multipoint === null
        ? null
        : { enabled: multipoint, source: 'vendor-protocol', readAt: '2026-10-13T07:00:00Z' },
    pairedDevices: paired,
    systemAudio: null,
    hostName: "Jaisal's S24 Ultra",
    lastError: null,
    updatedAt: '2026-10-13T07:00:00Z',
  };
}

const applied: CommandOutcome = { kind: 'applied', verifiedAt: '2026-10-13T07:00:01Z' };

async function renderPage() {
  render(
    <AppProvider>
      <DevicePage />
    </AppProvider>,
  );
  await waitFor(() => expect(ipcMock.getSnapshot).toHaveBeenCalled());
  await act(async () => {});
}

const multiSwitch = () => screen.getByRole('switch', { name: 'Multi-device connection' });
const deviceSwitch = (name: string) => screen.getByRole('switch', { name: `${name} connected` });
const deviceList = () => screen.getByRole('list', { name: 'Devices the headphones are paired with' });
/** The line under a device's name: which it is, and whether it is connected. */
const stateOf = (name: string) =>
  within(within(deviceList()).getByText(name).closest('li') as HTMLElement).getByTestId('device-state');

beforeEach(() => {
  vi.clearAllMocks();
  useAgent(ANDROID_UA);
  ipcMock.getSnapshot.mockResolvedValue(snapshot());
  ipcMock.executeCommand.mockResolvedValue(applied);
});

afterEach(() => {
  useAgent(DESKTOP_UA);
});

describe('the Device page', () => {
  it('opens with the device information, not a picture of the headphones', async () => {
    await renderPage();
    // The card that repeated the name and offered to reconnect is not there.
    expect(screen.queryByRole('button', { name: 'Reconnect' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Disconnect' })).not.toBeInTheDocument();
    expect(screen.getByText('Device Information')).toBeInTheDocument();
    expect(screen.getByText('Multi-device connection')).toBeInTheDocument();
  });

  it('still has a way to reconnect when the headphones are not connected', async () => {
    ipcMock.getSnapshot.mockResolvedValue(
      snapshot({ connection: 'disconnected', multipoint: null, paired: null }),
    );
    await renderPage();

    await userEvent.click(screen.getByRole('button', { name: 'Reconnect' }));
    await waitFor(() => expect(ipcMock.executeCommand).toHaveBeenCalledWith({ kind: 'connect' }));
  });

  it('is the same on a desktop: no card, and a way to reconnect when it is needed', async () => {
    useAgent(DESKTOP_UA);
    await renderPage();
    expect(screen.queryByRole('button', { name: 'Reconnect' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Disconnect' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Diagnostics/ })).not.toBeInTheDocument();
    expect(screen.getByText('Device Information')).toBeInTheDocument();
  });

  it('offers to reconnect on a desktop too when the headphones are not connected', async () => {
    useAgent(DESKTOP_UA);
    ipcMock.getSnapshot.mockResolvedValue(
      snapshot({ connection: 'disconnected', multipoint: null, paired: null }),
    );
    await renderPage();

    await userEvent.click(screen.getByRole('button', { name: 'Reconnect' }));
    await waitFor(() => expect(ipcMock.executeCommand).toHaveBeenCalledWith({ kind: 'connect' }));
  });
});

describe('multi-device connection', () => {
  it('shows what the headphones report: on, and each device with a switch that is on while it is connected', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ paired: [PHONE, LAPTOP, TABLET] }));
    await renderPage();

    expect(multiSwitch()).toHaveAttribute('aria-checked', 'true');
    expect(deviceSwitch("Jaisal's S24 Ultra")).toHaveAttribute('aria-checked', 'true');
    expect(deviceSwitch('NEMESIS')).toHaveAttribute('aria-checked', 'true');
    expect(deviceSwitch('Old tablet')).toHaveAttribute('aria-checked', 'false');
    // Which of them is the one in the hand, and how each stands.
    expect(stateOf("Jaisal's S24 Ultra")).toHaveTextContent('This phone · Connected');
    expect(stateOf('NEMESIS')).toHaveTextContent(/^Connected$/);
    expect(stateOf('Old tablet')).toHaveTextContent(/^Not connected$/);
  });

  it('calls this device a computer on a computer', async () => {
    useAgent(DESKTOP_UA);
    ipcMock.getSnapshot.mockResolvedValue(
      snapshot({ paired: [{ ...PHONE, thisDevice: false }, { ...LAPTOP, thisDevice: true }] }),
    );
    await renderPage();
    expect(stateOf('NEMESIS')).toHaveTextContent('This computer · Connected');
    expect(screen.queryByText(/This phone/)).not.toBeInTheDocument();
  });

  it('never shows anything of a device but its name', async () => {
    await renderPage();
    const text = deviceList().textContent ?? '';
    expect(text).not.toContain(PHONE.id);
    expect(text).not.toContain(LAPTOP.id);
    expect(text).not.toMatch(/([0-9A-F]{2}:){5}[0-9A-F]{2}/i);
  });

  it('asks before switching off, and says the headphones may drop this phone', async () => {
    await renderPage();

    await userEvent.click(multiSwitch());

    // Nothing sent yet.
    expect(ipcMock.executeCommand).not.toHaveBeenCalled();
    expect(screen.getByText('Turn off multi-device connection?')).toBeInTheDocument();
    expect(screen.getByText(/If they drop this phone, they disconnect from it/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Turn off' }));

    await waitFor(() =>
      expect(ipcMock.executeCommand).toHaveBeenCalledWith({ kind: 'setMultipoint', enabled: false }),
    );
    expect(ipcMock.executeCommand).toHaveBeenCalledTimes(1);
    // And says afterwards what that means for this phone.
    await waitFor(() => expect(toast.info).toHaveBeenCalled());
    expect(toast.info.mock.calls[0]?.[1]?.description).toMatch(/this phone/);
  });

  it('sends nothing when switching off is called off', async () => {
    await renderPage();

    await userEvent.click(multiSwitch());
    await userEvent.click(screen.getByRole('button', { name: 'Keep it on' }));

    expect(ipcMock.executeCommand).not.toHaveBeenCalled();
    // The question goes away (it fades, so not on the instant).
    await waitFor(() =>
      expect(screen.queryByText('Turn off multi-device connection?')).not.toBeInTheDocument(),
    );
    expect(multiSwitch()).toHaveAttribute('aria-checked', 'true');
  });

  it('switches on without asking', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ multipoint: false, paired: [PHONE] }));
    await renderPage();
    expect(multiSwitch()).toHaveAttribute('aria-checked', 'false');

    await userEvent.click(multiSwitch());

    await waitFor(() =>
      expect(ipcMock.executeCommand).toHaveBeenCalledWith({ kind: 'setMultipoint', enabled: true }),
    );
    expect(screen.queryByText('Turn off multi-device connection?')).not.toBeInTheDocument();
    expect(toast.info).not.toHaveBeenCalled();
  });

  it('shows what the headphones then report, not what was asked', async () => {
    // Asked for off; the headphones go on reporting on.
    ipcMock.executeCommand.mockResolvedValue({
      kind: 'sent-unverified',
      reason: 'they report multipoint on after 30 read-back(s).',
    } satisfies CommandOutcome);
    await renderPage();

    await userEvent.click(multiSwitch());
    await userEvent.click(screen.getByRole('button', { name: 'Turn off' }));

    await waitFor(() => expect(toast.warning).toHaveBeenCalled());
    expect(multiSwitch()).toHaveAttribute('aria-checked', 'true');
    expect(toast.info).not.toHaveBeenCalled();
  });

  it('cannot be switched on headphones that have not reported it, and says why', async () => {
    ipcMock.getSnapshot.mockResolvedValue(
      snapshot({ multipoint: null, paired: null, multipointCap: 'unknown' }),
    );
    await renderPage();

    expect(multiSwitch()).toBeDisabled();
    expect(multiSwitch()).toHaveAttribute('aria-checked', 'false');
    expect(screen.getByText(/multipoint evidence for unknown/)).toBeInTheDocument();
    expect(screen.getByTestId('paired-unknown')).toBeInTheDocument();
  });

  it('shows no devices when the headphones are not connected', async () => {
    ipcMock.getSnapshot.mockResolvedValue(
      snapshot({ connection: 'disconnected', multipoint: null, paired: null, multipointCap: 'unknown' }),
    );
    await renderPage();

    expect(multiSwitch()).toBeDisabled();
    expect(screen.getByText('Connect the headphones to see this.')).toBeInTheDocument();
    expect(
      screen.queryByRole('list', { name: 'Devices the headphones are paired with' }),
    ).not.toBeInTheDocument();
  });
});

describe("a paired device's switch", () => {
  it('disconnects another device straight away, by its id and not by anything else', async () => {
    await renderPage();

    await userEvent.click(deviceSwitch('NEMESIS'));

    await waitFor(() =>
      expect(ipcMock.executeCommand).toHaveBeenCalledWith({
        kind: 'setPairedDeviceConnection',
        device: LAPTOP.id,
        connected: false,
      }),
    );
    expect(ipcMock.executeCommand).toHaveBeenCalledTimes(1);
    // Another device going is nothing to do with this phone.
    expect(toast.info).not.toHaveBeenCalled();
  });

  it('connects a paired device that is not connected', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ paired: [PHONE, TABLET] }));
    await renderPage();

    await userEvent.click(deviceSwitch('Old tablet'));

    await waitFor(() =>
      expect(ipcMock.executeCommand).toHaveBeenCalledWith({
        kind: 'setPairedDeviceConnection',
        device: TABLET.id,
        connected: true,
      }),
    );
  });

  it('moves only when the headphones report the device differently', async () => {
    // Asked to disconnect; the headphones go on reporting it connected.
    ipcMock.executeCommand.mockResolvedValue({
      kind: 'sent-unverified',
      reason: 'they report the device as connected after 16 look(s).',
    } satisfies CommandOutcome);
    await renderPage();

    await userEvent.click(deviceSwitch('NEMESIS'));

    await waitFor(() => expect(toast.warning).toHaveBeenCalled());
    expect(deviceSwitch('NEMESIS')).toHaveAttribute('aria-checked', 'true');
    expect(toast.success).not.toHaveBeenCalled();
  });

  it('follows the headphones when they report the change', async () => {
    await renderPage();
    ipcMock.getSnapshot.mockResolvedValue(
      snapshot({ paired: [PHONE, { ...LAPTOP, connected: false }] }),
    );

    await userEvent.click(deviceSwitch('NEMESIS'));

    await waitFor(() => expect(deviceSwitch('NEMESIS')).toHaveAttribute('aria-checked', 'false'));
    expect(stateOf('NEMESIS')).toHaveTextContent(/^Not connected$/);
  });

  it('asks before disconnecting this phone itself', async () => {
    await renderPage();

    await userEvent.click(deviceSwitch("Jaisal's S24 Ultra"));

    expect(ipcMock.executeCommand).not.toHaveBeenCalled();
    expect(screen.getByText('Disconnect from this phone?')).toBeInTheDocument();
    expect(screen.getByText(/this app will lose them/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Disconnect' }));

    await waitFor(() =>
      expect(ipcMock.executeCommand).toHaveBeenCalledWith({
        kind: 'setPairedDeviceConnection',
        device: PHONE.id,
        connected: false,
      }),
    );
    await waitFor(() => expect(toast.info).toHaveBeenCalled());
    expect(toast.info.mock.calls[0]?.[0]).toMatch(/disconnected from this phone/);
  });

  it('sends nothing when disconnecting this phone is called off', async () => {
    await renderPage();

    await userEvent.click(deviceSwitch("Jaisal's S24 Ultra"));
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(ipcMock.executeCommand).not.toHaveBeenCalled();
    expect(deviceSwitch("Jaisal's S24 Ultra")).toHaveAttribute('aria-checked', 'true');
  });

  it('asks before connecting another device while multi-device is off, because that drops this phone', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ multipoint: false, paired: [PHONE, TABLET] }));
    await renderPage();

    await userEvent.click(deviceSwitch('Old tablet'));

    expect(ipcMock.executeCommand).not.toHaveBeenCalled();
    expect(screen.getByText('Connect Old tablet?')).toBeInTheDocument();
    expect(screen.getByText(/disconnects them from this phone/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Connect' }));
    await waitFor(() =>
      expect(ipcMock.executeCommand).toHaveBeenCalledWith({
        kind: 'setPairedDeviceConnection',
        device: TABLET.id,
        connected: true,
      }),
    );
  });

  it('cannot be used when the headphones have not listed their devices as expected', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ connectionsCap: 'unknown' }));
    await renderPage();
    expect(deviceSwitch('NEMESIS')).toBeDisabled();
    expect(deviceSwitch("Jaisal's S24 Ultra")).toBeDisabled();
  });
});

describe('renaming the headphones', () => {
  const renameButton = () => screen.getByRole('button', { name: 'Rename headphones' });
  const input = () => screen.getByRole('textbox', { name: 'New name for the headphones' });

  it('shows the name the headphones report, whatever it is', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ name: 'Studio Cans' }));
    await renderPage();
    expect(screen.getAllByText('Studio Cans').length).toBeGreaterThan(0);
    expect(screen.queryByText('Aurora')).not.toBeInTheDocument();
  });

  it('starts from the current name and sends the new one, trimmed', async () => {
    ipcMock.executeCommand.mockResolvedValue(applied);
    await renderPage();

    await userEvent.click(renameButton());
    expect(input()).toHaveValue('Aurora');

    await userEvent.clear(input());
    await userEvent.type(input(), '  Nova  ');
    // What the headphones report after the change.
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ name: 'Nova' }));
    await userEvent.click(screen.getByRole('button', { name: 'Save name' }));

    await waitFor(() =>
      expect(ipcMock.executeCommand).toHaveBeenCalledWith({ kind: 'setDeviceName', name: 'Nova' }),
    );
    await waitFor(() => expect(screen.queryByRole('textbox')).not.toBeInTheDocument());
    expect(screen.getAllByText('Nova').length).toBeGreaterThan(0);
  });

  it('saves on Enter and gives up on Escape', async () => {
    ipcMock.executeCommand.mockResolvedValue(applied);
    await renderPage();

    await userEvent.click(renameButton());
    await userEvent.type(input(), '{Escape}');
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(ipcMock.executeCommand).not.toHaveBeenCalled();

    await userEvent.click(renameButton());
    await userEvent.clear(input());
    await userEvent.type(input(), 'Nova{Enter}');
    await waitFor(() =>
      expect(ipcMock.executeCommand).toHaveBeenCalledWith({ kind: 'setDeviceName', name: 'Nova' }),
    );
  });

  it('will not send a name that is empty or too long, and says which', async () => {
    await renderPage();
    await userEvent.click(renameButton());

    await userEvent.clear(input());
    expect(screen.getByRole('button', { name: 'Save name' })).toBeDisabled();
    expect(screen.getByTestId('rename-hint')).toHaveTextContent('A name cannot be empty.');

    await userEvent.type(input(), 'x'.repeat(MAX_DEVICE_NAME_BYTES + 1));
    expect(screen.getByRole('button', { name: 'Save name' })).toBeDisabled();
    expect(screen.getByTestId('rename-hint')).toHaveTextContent(/too long: 32 bytes/);
    await userEvent.type(input(), '{Enter}');
    expect(ipcMock.executeCommand).not.toHaveBeenCalled();
  });

  it('keeps the editor open, and the old name, when the headphones do not confirm', async () => {
    ipcMock.executeCommand.mockResolvedValue({
      kind: 'sent-unverified',
      reason: 'they still call themselves "Aurora" after 30 read-back(s).',
    } satisfies CommandOutcome);
    await renderPage();

    await userEvent.click(renameButton());
    await userEvent.clear(input());
    await userEvent.type(input(), 'Nova{Enter}');

    await waitFor(() => expect(toast.warning).toHaveBeenCalled());
    expect(input()).toHaveValue('Nova');
    expect(toast.success).not.toHaveBeenCalled();
  });

  it('is not offered when the headphones have not reported their name', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ renameCap: 'unknown' }));
    await renderPage();
    expect(screen.queryByRole('button', { name: 'Rename headphones' })).not.toBeInTheDocument();
    expect(screen.getByTestId('rename-unavailable')).toHaveTextContent('rename evidence for unknown');
  });

  it('is not offered while they are not connected', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ connection: 'disconnected' }));
    await renderPage();
    expect(screen.queryByRole('button', { name: 'Rename headphones' })).not.toBeInTheDocument();
  });
});

describe('a name for the headphones', () => {
  it('is trimmed, and has to be something', () => {
    expect(checkDeviceName('  Aurora  ')).toEqual({ ok: true, name: 'Aurora' });
    expect(checkDeviceName('').ok).toBe(false);
    expect(checkDeviceName('   ').ok).toBe(false);
  });

  it('is counted in bytes, as it is sent', () => {
    expect(checkDeviceName('x'.repeat(31)).ok).toBe(true);
    expect(checkDeviceName('x'.repeat(32)).ok).toBe(false);
    // Two bytes each.
    expect(checkDeviceName('é'.repeat(15)).ok).toBe(true);
    expect(checkDeviceName('é'.repeat(16)).ok).toBe(false);
  });

  it('has no control characters', () => {
    for (const bad of ['Auro\nra', 'A\tB', 'Aurora\u0000', '\u001b[31mred']) {
      expect(checkDeviceName(bad).ok, JSON.stringify(bad)).toBe(false);
    }
  });
});

describe('the model', () => {
  it('is named, with how far it has been tried', async () => {
    ipcMock.getSnapshot.mockResolvedValue({
      ...snapshot(),
      model: { name: 'QuietComfort Ultra Headphones (2nd Gen)', category: 'headphones', support: 'documented' },
    });
    await renderPage();

    expect(screen.getByText('QuietComfort Ultra Headphones (2nd Gen)')).toBeInTheDocument();
    expect(screen.getByTestId('model-support')).toHaveTextContent('Not yet tested with Void.');
  });

  it('says so for the tested model', async () => {
    ipcMock.getSnapshot.mockResolvedValue({
      ...snapshot(),
      model: { name: 'QuietComfort Headphones', category: 'headphones', support: 'tested' },
    });
    await renderPage();
    expect(screen.getByTestId('model-support')).toHaveTextContent('Tested with Void.');
  });

  it('shows each battery of a pair of earbuds', async () => {
    ipcMock.getSnapshot.mockResolvedValue({
      ...snapshot(),
      model: { name: 'QuietComfort Ultra Earbuds (2nd Gen)', category: 'earbuds', support: 'documented' },
      battery: {
        percent: 70,
        source: 'vendor-protocol',
        readAt: '2026-10-16T08:00:00Z',
        parts: [
          { name: 'Right', percent: 80 },
          { name: 'Left', percent: 70 },
        ],
      },
    });
    await renderPage();
    const parts = screen.getByTestId('battery-parts');
    expect(parts).toHaveTextContent('Right80%');
    expect(parts).toHaveTextContent('Left70%');
  });
});

describe('what the device does not have', () => {
  it('has no multi-device panel when it has neither the switch nor the list', async () => {
    ipcMock.getSnapshot.mockResolvedValue(
      snapshot({ multipointCap: 'unsupported', connectionsCap: 'unsupported', multipoint: null, paired: null }),
    );
    await renderPage();
    expect(screen.queryByTestId('multi-device')).not.toBeInTheDocument();
  });

  it('keeps the list when only the switch is missing', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ multipointCap: 'unsupported', multipoint: null }));
    await renderPage();
    expect(screen.queryByRole('switch', { name: 'Multi-device connection' })).not.toBeInTheDocument();
    expect(deviceList()).toBeInTheDocument();
  });

  it('offers no rename on a device that cannot be renamed', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ renameCap: 'unsupported' }));
    await renderPage();
    expect(screen.queryByRole('button', { name: 'Rename headphones' })).not.toBeInTheDocument();
    expect(screen.queryByTestId('rename-unavailable')).not.toBeInTheDocument();
  });
});
