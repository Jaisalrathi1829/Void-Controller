/**
 * Browser-preview stand-in for what is playing on the computer, so the
 * now-playing card can be seen and tried without the desktop application.
 * Labelled as simulated, like everything else in the preview. With `?np=
 * elsewhere` in the address it stands in for headphones playing from
 * another device instead.
 */
import type { CommandOutcome } from '@/types/device';
import type {
  HeadphonesPlaying,
  MediaAction,
  NowPlaying,
  NowPlayingView,
  PlaybackControl,
  RepeatMode,
} from '@/types/media';

const TRACKS = [
  { title: 'Simulated track', artist: 'Browser preview', album: 'Void', durationMs: 208_000 },
  { title: 'Another simulated track', artist: 'Browser preview', album: 'Void', durationMs: 174_000 },
];

export class BrowserSimulatedPlayer {
  private index = 0;
  private playing = true;
  private positionMs = 16_000;
  private sampledAtMs = Date.now();
  private shuffle = false;
  private repeat: RepeatMode = 'off';

  private now(): number {
    const moved = this.playing ? Date.now() - this.sampledAtMs : 0;
    return Math.min(this.positionMs + moved, TRACKS[this.index]!.durationMs);
  }

  private resample(positionMs = this.now()) {
    this.positionMs = positionMs;
    this.sampledAtMs = Date.now();
  }

  async nowPlaying(): Promise<NowPlaying> {
    const track = TRACKS[this.index]!;
    return {
      ...track,
      app: 'Simulated player',
      artwork: null,
      state: this.playing ? 'playing' : 'paused',
      positionMs: this.positionMs,
      sampledAtMs: this.sampledAtMs,
      canPlayPause: true,
      canNext: true,
      canPrevious: true,
      canSeek: true,
      shuffle: this.shuffle,
      repeat: this.repeat,
    };
  }

  private elsewherePlaying = true;

  private showsElsewhere(): boolean {
    try {
      return new URLSearchParams(window.location.search).get('np') === 'elsewhere';
    } catch {
      return false;
    }
  }

  private elsewhere(): HeadphonesPlaying {
    return {
      headphones: 'Simulated headphones',
      device: 'Simulated phone',
      thisDevice: false,
      cable: false,
      state: this.elsewherePlaying ? 'playing' : 'paused',
    };
  }

  /** A phone's own media is not read, so on a phone there is no `local`. */
  async view(onPhone = false): Promise<NowPlayingView> {
    if (this.showsElsewhere()) return { local: null, headphones: this.elsewhere() };
    return { local: onPhone ? null : await this.nowPlaying(), headphones: null };
  }

  async headphonesControl(control: PlaybackControl): Promise<CommandOutcome> {
    if (control === 'playPause') {
      this.elsewherePlaying = !this.elsewherePlaying;
      return { kind: 'applied', verifiedAt: new Date().toISOString() };
    }
    return {
      kind: 'sent-unverified',
      reason: 'Simulated: a skip is passed on and cannot be confirmed.',
    };
  }

  async control(action: MediaAction): Promise<boolean> {
    switch (action.kind) {
      case 'playPause':
        this.resample();
        this.playing = !this.playing;
        break;
      case 'next':
      case 'previous':
        this.index = (this.index + 1) % TRACKS.length;
        this.resample(0);
        break;
      case 'seek':
        this.resample(Math.max(0, Math.min(action.positionMs, TRACKS[this.index]!.durationMs)));
        break;
      case 'shuffle':
        this.shuffle = action.on;
        break;
      case 'repeat':
        this.repeat = action.mode;
        break;
    }
    return true;
  }
}
