/** Whether something is playing. */
export type PlaybackState = 'playing' | 'paused' | 'stopped' | 'changing';

export type RepeatMode = 'off' | 'track' | 'list';

/**
 * What is playing on this computer, as the playing application reports it
 * through Windows' media sessions. Not the headphones': they play what the
 * computer sends them.
 */
export interface NowPlaying {
  title: string;
  artist: string;
  album: string;
  /** The application playing it. */
  app: string;
  /** A `data:` URL, or null when there is no artwork. */
  artwork: string | null;
  state: PlaybackState;
  /** How far in, at `sampledAtMs`. */
  positionMs: number;
  /** How long it is; null when the application does not say. */
  durationMs: number | null;
  /** When `positionMs` was true, in ms since 1970. */
  sampledAtMs: number;
  canPlayPause: boolean;
  canNext: boolean;
  canPrevious: boolean;
  canSeek: boolean;
  /** Present only where the application lets it be changed. */
  shuffle: boolean | null;
  repeat: RepeatMode | null;
}

export type MediaAction =
  | { kind: 'playPause' }
  | { kind: 'next' }
  | { kind: 'previous' }
  | { kind: 'seek'; positionMs: number }
  | { kind: 'shuffle'; on: boolean }
  | { kind: 'repeat'; mode: RepeatMode };

/**
 * What the headphones say they are playing from: which of the devices they
 * are connected to, and whether it plays. Not the track: the headphones keep
 * that for the Bose app's signed-in connection.
 */
export interface HeadphonesPlaying {
  /** The headphones, as the system calls them. */
  headphones: string;
  /** What the headphones call the device; null for the cable, or one they did not name. */
  device: string | null;
  /** The device playing is the one Void is running on. */
  thisDevice: boolean;
  cable: boolean;
  state: 'playing' | 'paused' | 'stopped';
}

/** Both readings, each on its own: one that could not be read is null. */
export interface NowPlayingView {
  /** On this computer, from Windows' media sessions. Never on a phone. */
  local: NowPlaying | null;
  /** Through the headphones. */
  headphones: HeadphonesPlaying | null;
}

/** Passed on through the headphones to the device they play from. */
export type PlaybackControl = 'playPause' | 'next' | 'previous';

export type Shown =
  | { kind: 'local'; np: NowPlaying }
  | { kind: 'elsewhere'; hp: HeadphonesPlaying };

/**
 * Which to show. What plays wins over what is paused; between two of a kind
 * the headphones' source wins, it being what is in the listener's ears.
 * What plays on this computer is shown from its own reading, which has the
 * track; the headphones' reading is shown for another device, or for this
 * one when there is no reading of its own (a phone). A stopped source is
 * not shown.
 */
export function chooseShown(view: NowPlayingView | null): Shown | null {
  if (!view) return null;
  const { local, headphones: hp } = view;
  const other = hp && !hp.thisDevice ? hp : null;
  if (other?.state === 'playing') return { kind: 'elsewhere', hp: other };
  if (local?.state === 'playing') return { kind: 'local', np: local };
  if (other?.state === 'paused') return { kind: 'elsewhere', hp: other };
  if (local) return { kind: 'local', np: local };
  if (hp && hp.state !== 'stopped') return { kind: 'elsewhere', hp };
  return null;
}

/** Where playback is now: the sample, carried forward while playing. */
export function positionNow(np: NowPlaying, now: number): number {
  const moved = np.state === 'playing' ? Math.max(0, now - np.sampledAtMs) : 0;
  const at = np.positionMs + moved;
  return np.durationMs == null ? at : Math.min(at, np.durationMs);
}

/** 83000 → "1:23"; an hour or more → "1:02:03". */
export function clock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const ss = String(s).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}
