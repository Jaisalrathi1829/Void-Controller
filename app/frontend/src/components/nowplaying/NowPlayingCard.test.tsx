import { describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { chooseShown, clock, positionNow, type HeadphonesPlaying, type NowPlaying } from '@/types/media';

vi.mock('sonner', () => ({ toast: { success: vi.fn(), warning: vi.fn(), error: vi.fn() } }));

import { AppProvider } from '@/store';
import { NowPlayingCard, NowPlayingRow, PlayingElsewhereCard } from './NowPlayingCard';

function track(over: Partial<NowPlaying> = {}): NowPlaying {
  return {
    title: 'Honest',
    artist: 'The Chainsmokers',
    album: 'Honest',
    app: 'Amazon Music',
    artwork: null,
    state: 'playing',
    positionMs: 16_000,
    durationMs: 208_000,
    sampledAtMs: Date.now(),
    canPlayPause: true,
    canNext: true,
    canPrevious: true,
    canSeek: true,
    shuffle: false,
    repeat: 'off',
    ...over,
  };
}

describe('the time helpers', () => {
  it('carry the position forward only while playing, and not past the end', () => {
    const at = 1_000_000;
    const np = track({ positionMs: 10_000, sampledAtMs: at });
    expect(positionNow(np, at + 5_000)).toBe(15_000);
    expect(positionNow({ ...np, state: 'paused' }, at + 5_000)).toBe(10_000);
    expect(positionNow(np, at + 10_000_000)).toBe(208_000);
  });

  it('read like a player', () => {
    expect(clock(0)).toBe('0:00');
    expect(clock(83_000)).toBe('1:23');
    expect(clock(3_723_000)).toBe('1:02:03');
  });
});

describe('the now playing card', () => {
  it('shows the track, the application and the controls it allows', () => {
    const onControl = vi.fn();
    render(
      <AppProvider>
        <NowPlayingCard np={track({ canNext: false, shuffle: null })} onControl={onControl} />
      </AppProvider>,
    );
    expect(screen.getByText('Honest', { selector: 'div' })).toBeInTheDocument();
    expect(screen.getByText('The Chainsmokers')).toBeInTheDocument();
    expect(screen.getByText('Amazon Music')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Pause' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: /Shuffle/ })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Repeat off' })).toBeInTheDocument();
  });

  it('asks the application, and does not show the change itself', async () => {
    const onControl = vi.fn();
    render(
      <AppProvider>
        <NowPlayingCard np={track()} onControl={onControl} />
      </AppProvider>,
    );
    await userEvent.click(screen.getByRole('button', { name: 'Pause' }));
    expect(onControl).toHaveBeenCalledWith({ kind: 'playPause' });
    // Still what the application last reported until it reports otherwise.
    expect(screen.getByRole('button', { name: 'Pause' })).toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Repeat off' }));
    expect(onControl).toHaveBeenCalledWith({ kind: 'repeat', mode: 'list' });
  });

  it('seeks by keyboard where the application allows it, and not where it does not', () => {
    const onControl = vi.fn();
    const { rerender } = render(
      <AppProvider>
        <NowPlayingCard np={track({ state: 'paused', positionMs: 16_000 })} onControl={onControl} />
      </AppProvider>,
    );
    const slider = screen.getByRole('slider', { name: 'Position' });
    act(() => {
      fireEvent.keyDown(slider, { key: 'ArrowRight' });
    });
    expect(onControl).toHaveBeenCalledWith({ kind: 'seek', positionMs: 21_000 });

    onControl.mockClear();
    rerender(
      <AppProvider>
        <NowPlayingCard np={track({ canSeek: false })} onControl={onControl} />
      </AppProvider>,
    );
    act(() => {
      fireEvent.keyDown(screen.getByRole('slider', { name: 'Position' }), { key: 'ArrowRight' });
    });
    expect(onControl).not.toHaveBeenCalled();
  });

  it('says so when nothing is playing', () => {
    render(
      <AppProvider>
        <NowPlayingCard np={null} onControl={vi.fn()} />
        <NowPlayingRow np={null} onControl={vi.fn()} />
      </AppProvider>,
    );
    expect(screen.getAllByTestId('now-playing-empty')).toHaveLength(2);
    expect(screen.queryByTestId('now-playing')).not.toBeInTheDocument();
  });

  it('has no position bar for something with no length', () => {
    render(
      <AppProvider>
        <NowPlayingRow np={track({ durationMs: null, canSeek: false })} onControl={vi.fn()} />
      </AppProvider>,
    );
    expect(screen.queryByRole('slider')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Pause' })).toBeInTheDocument();
  });
});

function phone(over: Partial<HeadphonesPlaying> = {}): HeadphonesPlaying {
  return {
    headphones: 'Aurora',
    device: "Jaisal's S24 Ultra",
    thisDevice: false,
    cable: false,
    state: 'playing',
    ...over,
  };
}

describe('which is shown', () => {
  it('what plays wins over what is paused, wherever it is', () => {
    const paused = track({ state: 'paused' });
    expect(chooseShown({ local: paused, headphones: phone() })?.kind).toBe('elsewhere');
    expect(chooseShown({ local: track(), headphones: phone({ state: 'paused' }) })?.kind).toBe('local');
  });

  it("with both paused, the headphones' source; with both playing, too", () => {
    const paused = track({ state: 'paused' });
    expect(chooseShown({ local: paused, headphones: phone({ state: 'paused' }) })?.kind).toBe('elsewhere');
    expect(chooseShown({ local: track(), headphones: phone() })?.kind).toBe('elsewhere');
  });

  it('this computer is shown from its own reading, which has the track', () => {
    const here = phone({ thisDevice: true, device: 'DESKTOP' });
    expect(chooseShown({ local: track(), headphones: here })).toEqual({ kind: 'local', np: track() });
    // A phone has no reading of its own: the headphones' is shown.
    expect(chooseShown({ local: null, headphones: here })?.kind).toBe('elsewhere');
  });

  it('a stopped source, and nothing at all, show nothing', () => {
    expect(chooseShown({ local: null, headphones: phone({ state: 'stopped' }) })).toBeNull();
    expect(chooseShown({ local: null, headphones: null })).toBeNull();
    expect(chooseShown(null)).toBeNull();
  });
});

describe('playing on another device', () => {
  it('names the device and the headphones, and makes up no track or position', () => {
    render(
      <AppProvider>
        <PlayingElsewhereCard hp={phone()} onControl={vi.fn()} />
      </AppProvider>,
    );
    expect(screen.getByText("Jaisal's S24 Ultra")).toBeInTheDocument();
    expect(screen.getByText('Aurora')).toBeInTheDocument();
    expect(screen.getByText('Playing')).toBeInTheDocument();
    expect(screen.getByText(/don.t share the song.s name/)).toBeInTheDocument();
    expect(screen.queryByRole('slider')).not.toBeInTheDocument();
  });

  it('passes the controls on, and shows paused only once the headphones say so', async () => {
    const onControl = vi.fn();
    const { rerender } = render(
      <AppProvider>
        <PlayingElsewhereCard hp={phone()} onControl={onControl} />
      </AppProvider>,
    );
    await userEvent.click(screen.getByRole('button', { name: 'Pause' }));
    expect(onControl).toHaveBeenCalledWith('playPause');
    expect(screen.getByRole('button', { name: 'Pause' })).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Next' }));
    expect(onControl).toHaveBeenCalledWith('next');

    rerender(
      <AppProvider>
        <PlayingElsewhereCard hp={phone({ state: 'paused' })} onControl={onControl} />
      </AppProvider>,
    );
    expect(screen.getByRole('button', { name: 'Play' })).toBeInTheDocument();
    expect(screen.getByText('Paused')).toBeInTheDocument();
  });

  it('takes no second control while one is on its way', () => {
    render(
      <AppProvider>
        <PlayingElsewhereCard hp={phone()} onControl={vi.fn()} busy />
      </AppProvider>,
    );
    expect(screen.getByRole('button', { name: 'Pause' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Previous' })).toBeDisabled();
  });

  it('a device the headphones did not name, and the cable', () => {
    const { rerender } = render(
      <AppProvider>
        <PlayingElsewhereCard hp={phone({ device: null })} onControl={vi.fn()} />
      </AppProvider>,
    );
    expect(screen.getByText('Another device')).toBeInTheDocument();
    rerender(
      <AppProvider>
        <PlayingElsewhereCard hp={phone({ device: null, cable: true })} onControl={vi.fn()} />
      </AppProvider>,
    );
    expect(screen.getByText('Audio cable')).toBeInTheDocument();
  });
});
