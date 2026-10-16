import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createUnknownCapabilities } from '@/types/capability';
import type { DeviceSnapshot, PairedBose } from '@/types/device';

const toast = vi.hoisted(() => ({ success: vi.fn(), warning: vi.fn(), error: vi.fn(), info: vi.fn() }));
vi.mock('sonner', () => ({ toast }));

const ipcMock = vi.hoisted(() => ({
  getSnapshot: vi.fn(),
  listBoseDevices: vi.fn(),
  chooseBoseDevice: vi.fn(),
}));
vi.mock('@/services/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/services/ipc')>()),
  hasNativeLayer: () => true,
  ...ipcMock,
}));

import { AppProvider, useApp } from '@/store';
import { DeviceChooser } from './DeviceChooser';

const AURORA: PairedBose = {
  id: '0000000000000000aa01',
  name: 'Aurora',
  model: { name: 'QuietComfort Headphones', category: 'headphones', support: 'tested' },
  connected: true,
  current: false,
};
const ULTRA: PairedBose = {
  id: '0000000000000000aa02',
  name: 'Commute',
  model: { name: 'QuietComfort Ultra Headphones (2nd Gen)', category: 'headphones', support: 'documented' },
  connected: true,
  current: false,
};
const BUDS: PairedBose = {
  id: '0000000000000000aa03',
  name: "Zed's buds",
  model: { name: 'QuietComfort Ultra Earbuds (2nd Gen)', category: 'earbuds', support: 'documented' },
  connected: true,
  current: false,
};

const disconnected: DeviceSnapshot = {
  source: 'real',
  connection: 'disconnected',
  transport: 'none',
  identity: null,
  capabilities: createUnknownCapabilities('test'),
  battery: null,
  noiseControl: null,
  equalizer: null,
  systemAudio: null,
  lastError: null,
  updatedAt: '2026-10-16T08:00:00Z',
};

/** Shows whether the chooser is still up, as the main window would. */
function Harness() {
  const { choosing } = useApp();
  return choosing ? <DeviceChooser /> : <p>chooser closed</p>;
}

async function renderChooser() {
  render(
    <AppProvider>
      <Harness />
    </AppProvider>,
  );
  await waitFor(() => expect(ipcMock.getSnapshot).toHaveBeenCalled());
  await act(async () => {});
}

const LONG = { timeout: 4000 };

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  ipcMock.getSnapshot.mockResolvedValue(disconnected);
  ipcMock.chooseBoseDevice.mockImplementation(async () => ({ ...disconnected, connection: 'connected' }));
});

describe('the device chooser', () => {
  it('asks first whether it is headphones or earbuds', async () => {
    await renderChooser();
    expect(screen.getByText('What are you using?')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^Headphones/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^Earbuds/ })).toBeInTheDocument();
    expect(ipcMock.listBoseDevices).not.toHaveBeenCalled();
  });

  it('opens the one pair of headphones that is connected by itself', async () => {
    ipcMock.listBoseDevices.mockResolvedValue([AURORA, BUDS]);
    await renderChooser();

    await userEvent.click(screen.getByRole('button', { name: /^Headphones/ }));
    expect(await screen.findByTestId('device-found', {}, LONG)).toHaveTextContent('Aurora');
    expect(screen.getByTestId('device-found')).toHaveTextContent('QuietComfort Headphones');
    await waitFor(() => expect(ipcMock.chooseBoseDevice).toHaveBeenCalledWith(AURORA.id), LONG);
    expect(await screen.findByText('chooser closed')).toBeInTheDocument();
  });

  it('opens the earbuds when earbuds are asked for', async () => {
    ipcMock.listBoseDevices.mockResolvedValue([AURORA, BUDS]);
    await renderChooser();

    await userEvent.click(screen.getByRole('button', { name: /^Earbuds/ }));
    await waitFor(() => expect(ipcMock.chooseBoseDevice).toHaveBeenCalledWith(BUDS.id), LONG);
  });

  it('lets the user choose when more than one is connected', async () => {
    ipcMock.listBoseDevices.mockResolvedValue([AURORA, ULTRA]);
    await renderChooser();

    await userEvent.click(screen.getByRole('button', { name: /^Headphones/ }));
    expect(await screen.findByText('Which headphones?', {}, LONG)).toBeInTheDocument();
    expect(ipcMock.chooseBoseDevice).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole('button', { name: /Commute/ }));
    await waitFor(() => expect(ipcMock.chooseBoseDevice).toHaveBeenCalledWith(ULTRA.id));
  });

  it('says when none is connected, and points to the kind that is', async () => {
    ipcMock.listBoseDevices.mockResolvedValue([{ ...AURORA, connected: false }, BUDS]);
    await renderChooser();

    await userEvent.click(screen.getByRole('button', { name: /^Headphones/ }));
    expect(await screen.findByText('No Bose headphones connected.', {}, LONG)).toBeInTheDocument();
    // Paired but not connected: listed, not opened.
    expect(screen.getByText('Paired, not connected')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Aurora/ })).toBeInTheDocument();
    expect(screen.getByTestId('other-kind')).toHaveTextContent("Zed's buds is connected");
    expect(ipcMock.chooseBoseDevice).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole('button', { name: 'Look for earbuds instead' }));
    await waitFor(() => expect(ipcMock.chooseBoseDevice).toHaveBeenCalledWith(BUDS.id), LONG);
  });

  it('does not open a device by itself beside one it does not recognize', async () => {
    const unknown: PairedBose = { ...AURORA, id: '0000000000000000aa09', name: 'Old pair', model: null };
    ipcMock.listBoseDevices.mockResolvedValue([AURORA, unknown]);
    await renderChooser();

    await userEvent.click(screen.getByRole('button', { name: /^Headphones/ }));
    expect(await screen.findByText('Other Bose devices', {}, LONG)).toBeInTheDocument();
    expect(screen.getByText('Model not recognized')).toBeInTheDocument();
    expect(ipcMock.chooseBoseDevice).not.toHaveBeenCalled();
  });

  it('stays up when the choice could not be made', async () => {
    ipcMock.listBoseDevices.mockResolvedValue([AURORA, ULTRA]);
    ipcMock.chooseBoseDevice.mockRejectedValue({ kind: 'not-connected', message: 'gone' });
    await renderChooser();

    await userEvent.click(screen.getByRole('button', { name: /^Headphones/ }));
    await userEvent.click(await screen.findByRole('button', { name: /Aurora/ }, LONG));
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(screen.queryByText('chooser closed')).not.toBeInTheDocument();
    expect(screen.getByText('Which headphones?')).toBeInTheDocument();
  });
});
