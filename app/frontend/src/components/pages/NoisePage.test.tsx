import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createUnknownCapabilities, type CapabilityStatus } from '@/types/capability';
import type { CommandOutcome, DeviceSnapshot, NoiseControlMode } from '@/types/device';

const toast = vi.hoisted(() => ({ success: vi.fn(), warning: vi.fn(), error: vi.fn() }));
vi.mock('sonner', () => ({ toast }));

const ipcMock = vi.hoisted(() => ({
  getSnapshot: vi.fn(),
  setNoiseControlMode: vi.fn(),
  setNoiseCancellationLevel: vi.fn(),
  executeCommand: vi.fn(),
}));
vi.mock('@/services/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/services/ipc')>()),
  hasNativeLayer: () => true,
  ...ipcMock,
}));

import { AppProvider } from '@/store';
import { SimulatedBanner } from '../SimulatedBanner';
import { useApp } from '@/store';
import { NoisePage } from './NoisePage';

/** Real headphones, connected, as the native layer describes them. */
function snapshot(opts: {
  mode?: NoiseControlMode | null;
  noise?: CapabilityStatus;
  custom?: CapabilityStatus;
  /** Strength the headphones report for the custom mode. */
  level?: number | null;
  label?: string | null;
  source?: 'real' | 'mock';
  lastError?: string | null;
} = {}): DeviceSnapshot {
  const {
    mode = 'quiet',
    noise = 'experimental',
    custom = 'unknown',
    level = null,
    label = null,
    source = 'real',
    lastError = null,
  } = opts;
  const capabilities = createUnknownCapabilities('not investigated');
  for (const key of ['noiseControl', 'awareMode'] as const) {
    capabilities[key] = {
      ...capabilities[key],
      status: noise,
      mechanism: 'rfcomm-vendor',
      hardwareVerified: noise === 'verified',
      evidence: `evidence for ${noise}`,
    };
  }
  capabilities.customNoiseControl = {
    ...capabilities.customNoiseControl,
    status: custom,
    mechanism: 'rfcomm-vendor',
    hardwareVerified: custom === 'verified',
    evidence: `custom evidence for ${custom}`,
  };
  return {
    source,
    connection: 'connected',
    transport: source === 'real' ? 'bluetooth-classic' : 'simulated',
    identity: { name: 'Aurora', id: 'test' },
    capabilities,
    battery: null,
    noiseControl: mode
      ? {
          mode,
          ...(level !== null ? { level } : {}),
          ...(label !== null ? { label } : {}),
          source: source === 'real' ? 'vendor-protocol' : 'simulated',
          readAt: '2026-10-10T09:00:00Z',
        }
      : null,
    equalizer: null,
    systemAudio: null,
    lastError,
    updatedAt: '2026-10-10T09:00:00Z',
  };
}

/** The headphones in their custom mode, "Focus", at the given strength. */
const inFocus = (level: number | null = 4, custom: CapabilityStatus = 'experimental') =>
  snapshot({ mode: 'custom', custom, level, label: 'Focus' });

function Harness() {
  const { simulated } = useApp();
  return (
    <>
      {simulated && <SimulatedBanner />}
      <NoisePage />
    </>
  );
}

async function renderPage() {
  render(
    <AppProvider>
      <Harness />
    </AppProvider>,
  );
  // The first snapshot arrives asynchronously after mount.
  await waitFor(() => expect(ipcMock.getSnapshot).toHaveBeenCalled());
  await act(async () => {});
}

const tab = (name: string) => screen.getByRole('tab', { name });
const slider = () => screen.getByRole('slider', { name: 'Noise Cancellation' }) as HTMLInputElement;
const applied: CommandOutcome = { kind: 'applied', verifiedAt: '2026-10-10T09:00:01Z' };

/** Drags the slider to `value` and lets go, which is when it commits. */
function dragTo(value: number) {
  fireEvent.change(slider(), { target: { value: String(value) } });
  fireEvent.pointerUp(slider());
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe('noise control on real headphones', () => {
  it('shows the mode the headphones reported', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ mode: 'aware' }));
    await renderPage();

    expect(tab('Aware')).toHaveAttribute('aria-selected', 'true');
    expect(tab('Quiet')).toHaveAttribute('aria-selected', 'false');
    expect(screen.getByTestId('noise-read-at')).toHaveTextContent('Reported by the headphones at');
  });

  it('selects nothing when the headphones have not reported a mode', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ mode: null }));
    await renderPage();

    for (const name of ['Quiet', 'Aware', 'Custom']) {
      expect(tab(name)).toHaveAttribute('aria-selected', 'false');
    }
  });

  it('says the control is experimental and why', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ noise: 'experimental' }));
    await renderPage();

    expect(screen.getByText(/^Experimental\./)).toBeInTheDocument();
    expect(screen.getByText(/evidence for experimental/)).toBeInTheDocument();
  });

  it('drops the experimental caveat once verified', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ noise: 'verified' }));
    await renderPage();

    expect(screen.queryByText(/^Experimental\./)).not.toBeInTheDocument();
    expect(tab('Aware')).toBeEnabled();
  });

  it('disables the whole control while the capability is unknown', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ noise: 'unknown', mode: null }));
    await renderPage();

    expect(tab('Quiet')).toBeDisabled();
    expect(tab('Aware')).toBeDisabled();
    expect(screen.getByText(/evidence for unknown/)).toBeInTheDocument();
  });

  it('requests a preset through the typed operation only', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ mode: 'quiet' }));
    ipcMock.setNoiseControlMode.mockResolvedValue(applied);
    await renderPage();

    await userEvent.click(tab('Aware'));

    await waitFor(() => expect(ipcMock.setNoiseControlMode).toHaveBeenCalledWith('aware'));
    expect(ipcMock.setNoiseControlMode).toHaveBeenCalledTimes(1);
    expect(ipcMock.executeCommand).not.toHaveBeenCalled();
  });

  it('moves the selection only when the headphones report the new mode', async () => {
    ipcMock.getSnapshot.mockResolvedValueOnce(snapshot({ mode: 'quiet' }));
    ipcMock.setNoiseControlMode.mockResolvedValue(applied);
    // The re-read after the command is what moves the selection.
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ mode: 'aware' }));
    await renderPage();
    expect(tab('Quiet')).toHaveAttribute('aria-selected', 'true');

    await userEvent.click(tab('Aware'));

    await waitFor(() => expect(tab('Aware')).toHaveAttribute('aria-selected', 'true'));
    expect(toast.success).toHaveBeenCalledTimes(1);
    expect(toast.warning).not.toHaveBeenCalled();
  });

  it('keeps the last confirmed mode and warns when a change is sent but unverified', async () => {
    // The headphones keep reporting Quiet before and after the request.
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ mode: 'quiet' }));
    ipcMock.setNoiseControlMode.mockResolvedValue({
      kind: 'sent-unverified',
      reason: 'The device acknowledged the request, but it still reports Quiet.',
    });
    await renderPage();

    await userEvent.click(tab('Aware'));

    await waitFor(() => expect(toast.warning).toHaveBeenCalledTimes(1));
    expect(toast.warning).toHaveBeenCalledWith(
      expect.stringMatching(/could not be verified/),
      expect.objectContaining({
        description: 'The device acknowledged the request, but it still reports Quiet.',
      }),
    );
    expect(toast.success).not.toHaveBeenCalled();

    // The requested mode is not shown as active; the confirmed one is.
    await waitFor(() => expect(tab('Quiet')).toBeEnabled());
    expect(tab('Quiet')).toHaveAttribute('aria-selected', 'true');
    expect(tab('Aware')).toHaveAttribute('aria-selected', 'false');
  });

  it('reports a rejected change as rejected', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ mode: 'quiet' }));
    ipcMock.setNoiseControlMode.mockResolvedValue({
      kind: 'rejected',
      reason: 'The headphones refused the change.',
    });
    await renderPage();

    await userEvent.click(tab('Aware'));

    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    expect(toast.success).not.toHaveBeenCalled();
    expect(tab('Quiet')).toHaveAttribute('aria-selected', 'true');
  });

  it('re-reads the headphones after a failed request instead of guessing', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ mode: 'quiet' }));
    ipcMock.setNoiseControlMode.mockRejectedValue({
      kind: 'control-link',
      message: 'the control connection to the headphones failed',
    });
    await renderPage();
    const readsBefore = ipcMock.getSnapshot.mock.calls.length;

    await userEvent.click(tab('Aware'));

    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(ipcMock.getSnapshot.mock.calls.length).toBeGreaterThan(readsBefore),
    );
    expect(tab('Quiet')).toHaveAttribute('aria-selected', 'true');
    expect(toast.success).not.toHaveBeenCalled();
  });

  it('says so when the mode on screen is not a fresh reading', async () => {
    ipcMock.getSnapshot.mockResolvedValue(
      snapshot({
        mode: 'quiet',
        lastError: 'The noise-control mode could not be read from the headphones: timeout',
      }),
    );
    await renderPage();

    const alert = screen.getByRole('alert');
    expect(within(alert).getByText(/could not be read from the headphones/)).toBeInTheDocument();
    expect(within(alert).getByText(/last one the headphones confirmed/)).toBeInTheDocument();
    expect(tab('Quiet')).toHaveAttribute('aria-selected', 'true');
  });
});

describe('the custom mode', () => {
  it('is not offered until the headphones are known to have one, and says why', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ custom: 'unknown' }));
    await renderPage();

    expect(tab('Quiet')).toBeEnabled();
    expect(tab('Aware')).toBeEnabled();
    expect(tab('Custom')).toBeDisabled();
    expect(screen.getByTestId('custom-unavailable')).toHaveTextContent(
      'custom evidence for unknown',
    );
    expect(screen.queryByTestId('custom-panel')).not.toBeInTheDocument();
  });

  it('is offered once the headphones list a custom mode', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ custom: 'experimental' }));
    await renderPage();

    expect(tab('Custom')).toBeEnabled();
    expect(screen.queryByTestId('custom-unavailable')).not.toBeInTheDocument();
  });

  it('is requested by name, with no slot', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ custom: 'experimental' }));
    ipcMock.setNoiseControlMode.mockResolvedValue(applied);
    await renderPage();

    await userEvent.click(tab('Custom'));

    await waitFor(() => expect(ipcMock.setNoiseControlMode).toHaveBeenCalledWith('custom'));
    expect(ipcMock.setNoiseControlMode).toHaveBeenCalledTimes(1);
    expect(ipcMock.executeCommand).not.toHaveBeenCalled();
  });

  it('is still shown as active when it cannot be requested', async () => {
    ipcMock.getSnapshot.mockResolvedValue(
      snapshot({ mode: 'custom', custom: 'unknown', label: 'Focus' }),
    );
    await renderPage();

    expect(tab('Custom')).toHaveAttribute('aria-selected', 'true');
    expect(tab('Custom')).toBeDisabled();
  });

  it('shows the name and level the headphones report', async () => {
    ipcMock.getSnapshot.mockResolvedValue(inFocus(4));
    await renderPage();

    const panel = screen.getByTestId('custom-panel');
    expect(within(panel).getByText('Focus')).toBeInTheDocument();
    expect(within(panel).getByText(/saved to that mode on your headphones/)).toBeInTheDocument();
    expect(slider().value).toBe('4');
    expect(within(panel).getByText(/custom evidence for experimental/)).toBeInTheDocument();
  });

  it('has no slider in Quiet or Aware', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ mode: 'quiet', custom: 'experimental' }));
    await renderPage();

    expect(screen.queryByTestId('custom-panel')).not.toBeInTheDocument();
    expect(screen.queryByRole('slider')).not.toBeInTheDocument();
  });

  it('shows no slider rather than a made-up level when none was reported', async () => {
    ipcMock.getSnapshot.mockResolvedValue(inFocus(null));
    await renderPage();

    expect(screen.getByTestId('level-unknown')).toBeInTheDocument();
    expect(screen.queryByRole('slider')).not.toBeInTheDocument();
  });

  it('sends one level when the slider is released, not one per step', async () => {
    ipcMock.getSnapshot.mockResolvedValue(inFocus(4));
    ipcMock.setNoiseCancellationLevel.mockResolvedValue(applied);
    await renderPage();

    // Dragging through several values...
    fireEvent.change(slider(), { target: { value: '5' } });
    fireEvent.change(slider(), { target: { value: '6' } });
    fireEvent.change(slider(), { target: { value: '7' } });
    expect(ipcMock.setNoiseCancellationLevel).not.toHaveBeenCalled();
    // ...commits only where it is let go.
    fireEvent.pointerUp(slider());

    await waitFor(() => expect(ipcMock.setNoiseCancellationLevel).toHaveBeenCalledWith(7));
    expect(ipcMock.setNoiseCancellationLevel).toHaveBeenCalledTimes(1);
    expect(ipcMock.executeCommand).not.toHaveBeenCalled();
  });

  it('moves to the new level only when the headphones report it', async () => {
    ipcMock.getSnapshot.mockResolvedValueOnce(inFocus(4));
    ipcMock.setNoiseCancellationLevel.mockResolvedValue(applied);
    ipcMock.getSnapshot.mockResolvedValue(inFocus(7));
    await renderPage();

    dragTo(7);

    await waitFor(() => expect(toast.success).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(slider().value).toBe('7'));
  });

  it('snaps back to the confirmed level when a change is sent but unverified', async () => {
    // The headphones keep reporting 4 whatever was asked for.
    ipcMock.getSnapshot.mockResolvedValue(inFocus(4));
    ipcMock.setNoiseCancellationLevel.mockResolvedValue({
      kind: 'sent-unverified',
      reason: 'The device acknowledged the change, but "Focus" still reports level 4.',
    });
    await renderPage();

    dragTo(9);

    await waitFor(() => expect(toast.warning).toHaveBeenCalledTimes(1));
    expect(toast.success).not.toHaveBeenCalled();
    await waitFor(() => expect(slider()).toBeEnabled());
    expect(slider().value).toBe('4');
  });

  it('re-reads the headphones after a level change fails outright', async () => {
    ipcMock.getSnapshot.mockResolvedValue(inFocus(4));
    ipcMock.setNoiseCancellationLevel.mockRejectedValue({
      kind: 'control-link',
      message: 'the control connection to the headphones failed',
    });
    await renderPage();
    const readsBefore = ipcMock.getSnapshot.mock.calls.length;

    dragTo(2);

    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    await waitFor(() =>
      expect(ipcMock.getSnapshot.mock.calls.length).toBeGreaterThan(readsBefore),
    );
    await waitFor(() => expect(slider()).toBeEnabled());
    expect(slider().value).toBe('4');
  });

  it('does not offer wind block on real headphones', async () => {
    ipcMock.getSnapshot.mockResolvedValue(inFocus(4));
    await renderPage();

    expect(screen.queryByText('Wind Block')).not.toBeInTheDocument();
  });
});

describe('simulated labelling', () => {
  it('shows the SIMULATED banner for the mock device', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ source: 'mock' }));
    await renderPage();

    expect(screen.getByText('Simulated Device')).toBeInTheDocument();
    expect(screen.getByText(/Every value shown is fabricated/)).toBeInTheDocument();
    // A simulated reading is not described as coming from the headphones.
    expect(screen.queryByTestId('noise-read-at')).not.toBeInTheDocument();
  });

  it('does not describe a simulated custom level as saved to headphones', async () => {
    ipcMock.getSnapshot.mockResolvedValue(
      snapshot({
        source: 'mock',
        mode: 'custom',
        custom: 'experimental',
        level: 6,
        label: 'Simulated custom',
      }),
    );
    await renderPage();

    expect(screen.getByText('Simulated Device')).toBeInTheDocument();
    expect(screen.getByTestId('custom-panel')).toBeInTheDocument();
    expect(screen.queryByText(/saved to that mode on your headphones/)).not.toBeInTheDocument();
  });

  it('does not show it for real hardware', async () => {
    ipcMock.getSnapshot.mockResolvedValue(snapshot({ source: 'real' }));
    await renderPage();

    expect(screen.queryByText('Simulated Device')).not.toBeInTheDocument();
  });
});
