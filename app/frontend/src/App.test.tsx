import { describe, expect, it } from 'vitest';
import { screenKey } from './App';
import type { ConnectionState } from './store';
import type { PageId } from './pages';

const STATES: ConnectionState[] = [
  'connected',
  'disconnected',
  'connecting',
  'discovering',
  'reconnecting',
  'error',
];

describe('which screen is on show', () => {
  // The key decides when the screen is torn down and built again. A page
  // that goes on being shown must keep the same one, or its animation starts
  // over every time the connection state so much as flickers.
  it('does not change under a page that stays on screen', () => {
    for (const page of ['device', 'settings'] as PageId[]) {
      const keys = new Set(STATES.map((state) => screenKey(page, state)));
      expect([...keys]).toEqual([page]);
    }
  });

  it('is the page itself while the headphones are connected', () => {
    for (const page of ['dashboard', 'noise', 'equalizer', 'profiles'] as PageId[]) {
      expect(screenKey(page, 'connected')).toBe(page);
    }
  });

  it('is the connection screen, in its current state, when a page needs headphones that are not there', () => {
    expect(screenKey('dashboard', 'disconnected')).toBe('offline:disconnected');
    expect(screenKey('dashboard', 'reconnecting')).toBe('offline:reconnecting');
    // The same connection screen whichever page was asked for is still a
    // different screen from the page that replaces it once connected.
    expect(screenKey('noise', 'disconnected')).not.toBe(screenKey('noise', 'connected'));
  });
});
