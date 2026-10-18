import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from "react";
import { AnimatePresence, motion } from "motion/react";
import {
  Cable,
  Headphones,
  MonitorSmartphone,
  Music2,
  Pause,
  Play,
  Repeat,
  Repeat1,
  Shuffle,
  SkipBack,
  SkipForward,
} from "lucide-react";
import { cn } from "../ui/utils";
import { useApp } from "../../store";
import {
  clock,
  positionNow,
  type HeadphonesPlaying,
  type MediaAction,
  type NowPlaying,
  type PlaybackControl,
} from "@/types/media";
import { hostPlatform } from "@/services/ipc";

type Tone = "art" | "theme";

/** Colours for the two surfaces the card sits on: its own artwork backdrop,
 * which is always dark, or the theme's card. */
const TONES: Record<Tone, { fg: string; muted: string; faint: string; track: string; fill: string; knob: string; hover: string }> = {
  art: {
    fg: "text-white",
    muted: "text-white/70",
    faint: "text-white/50",
    track: "bg-white/20",
    fill: "bg-white",
    knob: "bg-white",
    hover: "hover:bg-white/10",
  },
  theme: {
    fg: "text-foreground",
    muted: "text-muted-foreground",
    faint: "text-muted-foreground/80",
    track: "bg-muted",
    fill: "bg-foreground",
    knob: "bg-foreground",
    hover: "hover:bg-accent",
  },
};

/** Three bars that move while something plays, and rest while it does not. */
function PlayingBars({ playing, className }: { playing: boolean; className?: string }) {
  return (
    <span aria-hidden className={cn("np-bars inline-flex h-3 items-end gap-[2px]", playing && "np-bars-on", className)}>
      <span />
      <span />
      <span />
    </span>
  );
}

/**
 * The progress bar. Its position is carried forward from the last sample on
 * every animation frame, set straight on the elements so nothing else
 * re-renders; the times under it change once a second. Where the application
 * allows it, it can be dragged or clicked to seek, and arrowed by keyboard.
 */
function ProgressBar({
  np,
  tone,
  onSeek,
  compact,
}: {
  np: NowPlaying;
  tone: Tone;
  onSeek: (ms: number) => void;
  compact?: boolean;
}) {
  const t = TONES[tone];
  const duration = np.durationMs ?? 0;
  const trackRef = useRef<HTMLDivElement>(null);
  const fillRef = useRef<HTMLDivElement>(null);
  const knobRef = useRef<HTMLDivElement>(null);
  const drag = useRef<number | null>(null);
  /** Where a seek put it, held until the application's own report catches up. */
  const held = useRef<{ target: number; at: number } | null>(null);
  const shownSecond = useRef(-1);
  const [shown, setShown] = useState(() => positionNow(np, Date.now()));
  const [dragging, setDragging] = useState(false);

  useEffect(() => {
    let frame = 0;
    const tick = () => {
      const now = Date.now();
      let at = positionNow(np, now);
      const hold = held.current;
      if (hold) {
        const settled = Math.abs(at - hold.target) < 2500 || now - hold.at > 3000;
        if (settled) held.current = null;
        else at = Math.min(duration, hold.target + (np.state === "playing" ? now - hold.at : 0));
      }
      const fraction =
        drag.current ?? (duration > 0 ? Math.min(1, Math.max(0, at / duration)) : 0);
      if (fillRef.current) fillRef.current.style.transform = `scaleX(${fraction})`;
      if (knobRef.current) knobRef.current.style.left = `${fraction * 100}%`;
      const label = drag.current != null ? drag.current * duration : at;
      const second = Math.floor(label / 1000);
      if (second !== shownSecond.current) {
        shownSecond.current = second;
        setShown(label);
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [np, duration]);

  const fractionAt = (clientX: number) => {
    const rect = trackRef.current?.getBoundingClientRect();
    if (!rect || rect.width === 0) return 0;
    return Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
  };
  const commit = (fraction: number) => {
    const target = Math.round(fraction * duration);
    held.current = { target, at: Date.now() };
    onSeek(target);
  };

  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (!np.canSeek) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    drag.current = fractionAt(e.clientX);
    setDragging(true);
  };
  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (drag.current != null) drag.current = fractionAt(e.clientX);
  };
  const onPointerUp = () => {
    if (drag.current == null) return;
    const fraction = drag.current;
    drag.current = null;
    setDragging(false);
    commit(fraction);
  };

  return (
    <div className="group/bar w-full">
      <div
        ref={trackRef}
        role="slider"
        aria-label="Position"
        aria-valuemin={0}
        aria-valuemax={Math.round(duration / 1000)}
        aria-valuenow={Math.round(shown / 1000)}
        aria-valuetext={`${clock(shown)} of ${clock(duration)}`}
        aria-disabled={!np.canSeek}
        tabIndex={np.canSeek ? 0 : -1}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onKeyDown={(e) => {
          if (!np.canSeek || duration <= 0) return;
          const step = e.key === "ArrowRight" ? 5000 : e.key === "ArrowLeft" ? -5000 : 0;
          if (!step) return;
          e.preventDefault();
          commit(Math.min(1, Math.max(0, (shown + step) / duration)));
        }}
        className={cn(
          "relative flex items-center rounded-full outline-none focus-visible:ring-2 focus-visible:ring-ring",
          compact ? "h-3" : "h-4",
          np.canSeek && "cursor-pointer touch-none",
        )}
      >
        <div
          className={cn(
            "w-full overflow-hidden rounded-full transition-[height] duration-150",
            t.track,
            dragging ? "h-1.5" : "h-1 group-hover/bar:h-1.5",
          )}
        >
          <div ref={fillRef} className={cn("h-full w-full origin-left rounded-full", t.fill)} style={{ transform: "scaleX(0)" }} />
        </div>
        {np.canSeek && (
          <div
            ref={knobRef}
            aria-hidden
            className={cn(
              "pointer-events-none absolute top-1/2 size-3 -translate-x-1/2 -translate-y-1/2 rounded-full shadow-[0_1px_4px_rgba(0,0,0,0.35)] transition-[opacity,transform] duration-150",
              t.knob,
              dragging ? "scale-110 opacity-100" : "scale-75 opacity-0 group-hover/bar:scale-100 group-hover/bar:opacity-100",
            )}
          />
        )}
      </div>
      <div className={cn("mt-1 flex justify-between tabular-nums", compact ? "text-[10px]" : "text-[11px]", t.faint)}>
        <span>{clock(shown)}</span>
        <span>{duration > 0 ? clock(duration) : ""}</span>
      </div>
    </div>
  );
}

function IconButton({
  label,
  onClick,
  disabled,
  active,
  tone,
  size = "md",
  children,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  active?: boolean;
  tone: Tone;
  size?: "sm" | "md";
  children: React.ReactNode;
}) {
  const t = TONES[tone];
  return (
    <motion.button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={active}
      disabled={disabled}
      onClick={onClick}
      whileTap={disabled ? undefined : { scale: 0.88 }}
      className={cn(
        "relative inline-flex items-center justify-center rounded-full outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-35",
        size === "sm" ? "size-8" : "size-9",
        active ? t.fg : t.muted,
        !disabled && t.hover,
      )}
    >
      {children}
      {active && <span className={cn("absolute bottom-0.5 size-1 rounded-full", t.fill)} />}
    </motion.button>
  );
}

function PlayPauseButton({
  playing,
  disabled,
  onClick,
  tone,
  size = "md",
}: {
  playing: boolean;
  disabled: boolean;
  onClick: () => void;
  tone: Tone;
  size?: "sm" | "md";
}) {
  const filled = tone === "art" ? "bg-white text-black" : "bg-foreground text-background";
  const icon = size === "sm" ? "size-3.5" : "size-[18px]";
  return (
    <motion.button
      type="button"
      aria-label={playing ? "Pause" : "Play"}
      title={playing ? "Pause" : "Play"}
      disabled={disabled}
      onClick={onClick}
      whileHover={disabled ? undefined : { scale: 1.06 }}
      whileTap={disabled ? undefined : { scale: 0.9 }}
      transition={{ type: "spring", stiffness: 500, damping: 28 }}
      className={cn(
        "inline-flex shrink-0 items-center justify-center rounded-full shadow-[0_2px_10px_rgba(0,0,0,0.3)] outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-40",
        size === "sm" ? "size-8" : "size-11",
        filled,
      )}
    >
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span
          key={playing ? "pause" : "play"}
          initial={{ scale: 0.4, opacity: 0, rotate: -30 }}
          animate={{ scale: 1, opacity: 1, rotate: 0 }}
          exit={{ scale: 0.4, opacity: 0, rotate: 30 }}
          transition={{ duration: 0.16, ease: "easeOut" }}
          className="inline-flex"
        >
          {playing ? (
            <Pause className={icon} fill="currentColor" strokeWidth={0} />
          ) : (
            <Play className={cn(icon, "translate-x-px")} fill="currentColor" strokeWidth={0} />
          )}
        </motion.span>
      </AnimatePresence>
    </motion.button>
  );
}

function Artwork({ np, className }: { np: NowPlaying; className: string }) {
  return (
    <div className={cn("relative shrink-0 overflow-hidden bg-black/30 ring-1 ring-white/10", className)}>
      {np.artwork ? (
        <img src={np.artwork} alt="" className="size-full object-cover" draggable={false} />
      ) : (
        <div className="flex size-full items-center justify-center bg-gradient-to-br from-primary/60 to-chart-4/60">
          <Music2 className="size-1/2 text-white/80" />
        </div>
      )}
    </div>
  );
}

/** What it is playing on: the headphones' own name when they are the output. */
function useOutputName(): string | null {
  const { audioEndpointName, deviceName, connection } = useApp();
  if (!audioEndpointName) return null;
  if (connection === "connected" && deviceName && audioEndpointName.toLowerCase().includes(deviceName.toLowerCase())) {
    return deviceName;
  }
  return audioEndpointName;
}

const trackKey = (np: NowPlaying) => `${np.app}|${np.title}|${np.artist}`;

/**
 * What is playing on this computer, on the dashboard: the artwork, softly
 * blurred, as its backdrop; the track, the application and what it is
 * playing on; the position; and the controls the application allows.
 */
export function NowPlayingCard({
  np,
  onControl,
}: {
  np: NowPlaying | null;
  onControl: (action: MediaAction) => void;
}) {
  const output = useOutputName();

  if (!np) {
    return (
      <div
        className="panel-enter flex items-center gap-3 rounded-xl border border-border bg-card p-5 shadow-[var(--shadow-sm)]"
        data-testid="now-playing-empty"
      >
        <span className="flex size-11 items-center justify-center rounded-lg bg-muted text-muted-foreground">
          <Music2 className="size-5" />
        </span>
        <div>
          <div className="text-sm font-medium">Nothing playing</div>
          <div className="text-xs text-muted-foreground">
            What plays on this computer, or through your headphones from another device, shows
            here with its controls.
          </div>
        </div>
      </div>
    );
  }

  const playing = np.state === "playing";
  const key = trackKey(np);

  return (
    <div
      className="panel-enter relative isolate overflow-hidden rounded-xl border border-white/10 bg-neutral-900 text-white shadow-[var(--shadow-md)]"
      data-testid="now-playing"
    >
      {/* The artwork as backdrop: a still, blurred layer that crossfades
          with the track. Nothing here animates its blur. */}
      <AnimatePresence initial={false}>
        <motion.div
          key={np.artwork ?? key}
          aria-hidden
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.6 }}
          className="absolute inset-0 -z-10"
        >
          {np.artwork ? (
            <img src={np.artwork} alt="" className="absolute inset-[-25%] size-[150%] scale-110 object-cover blur-2xl saturate-150" />
          ) : (
            <div className="absolute inset-0 bg-gradient-to-br from-primary/50 via-neutral-900 to-chart-4/40" />
          )}
          <div className="absolute inset-0 bg-gradient-to-b from-black/55 via-black/45 to-black/70" />
        </motion.div>
      </AnimatePresence>

      <div className="p-5">
        <div className="flex items-center justify-between gap-3 text-[11px] font-medium uppercase tracking-[0.12em] text-white/60">
          <span className="inline-flex items-center gap-2">
            <PlayingBars playing={playing} />
            Now playing
          </span>
          {output && (
            <span className="inline-flex min-w-0 items-center gap-1.5 normal-case tracking-normal text-white/75">
              <Headphones className="size-3.5 shrink-0" />
              <span className="truncate">{output}</span>
            </span>
          )}
        </div>

        {/* A new track's details fade in over the old place. Entrance only,
            in CSS: the new title is there at once, whatever the frame rate. */}
        <div key={key} className="np-enter mt-4 flex items-center gap-4">
            <Artwork np={np} className="size-16 rounded-lg shadow-[0_6px_20px_rgba(0,0,0,0.45)]" />
            <div className="min-w-0">
              <div className="truncate text-base font-semibold leading-tight" title={np.title}>
                {np.title || "Untitled"}
              </div>
              <div className="mt-0.5 truncate text-sm text-white/70" title={np.artist}>
                {np.artist || np.album}
              </div>
              <div className="mt-1 truncate text-xs text-white/45">{np.app}</div>
            </div>
        </div>

        <div className="mt-4">
          {np.durationMs != null ? (
            <ProgressBar
              np={np}
              tone="art"
              onSeek={(positionMs) => onControl({ kind: "seek", positionMs })}
            />
          ) : (
            <div className="h-4" />
          )}
        </div>

        <div className="mt-2 flex items-center justify-center gap-3">
          {np.shuffle != null && (
            <IconButton
              tone="art"
              label={np.shuffle ? "Shuffle on" : "Shuffle off"}
              active={np.shuffle}
              onClick={() => onControl({ kind: "shuffle", on: !np.shuffle })}
            >
              <Shuffle className="size-4" />
            </IconButton>
          )}
          <IconButton
            tone="art"
            label="Previous"
            disabled={!np.canPrevious}
            onClick={() => onControl({ kind: "previous" })}
          >
            <SkipBack className="size-[18px]" fill="currentColor" />
          </IconButton>
          <PlayPauseButton
            tone="art"
            playing={playing}
            disabled={!np.canPlayPause}
            onClick={() => onControl({ kind: "playPause" })}
          />
          <IconButton
            tone="art"
            label="Next"
            disabled={!np.canNext}
            onClick={() => onControl({ kind: "next" })}
          >
            <SkipForward className="size-[18px]" fill="currentColor" />
          </IconButton>
          {np.repeat != null && (
            <IconButton
              tone="art"
              label={np.repeat === "off" ? "Repeat off" : np.repeat === "track" ? "Repeating this track" : "Repeating"}
              active={np.repeat !== "off"}
              onClick={() =>
                onControl({
                  kind: "repeat",
                  mode: np.repeat === "off" ? "list" : np.repeat === "list" ? "track" : "off",
                })
              }
            >
              {np.repeat === "track" ? <Repeat1 className="size-4" /> : <Repeat className="size-4" />}
            </IconButton>
          )}
        </div>
      </div>
    </div>
  );
}

/**
 * The same, for the desktop widget: one row of artwork, track and the three
 * main controls, and the position under it, on the widget's own surface.
 */
export function NowPlayingRow({
  np,
  onControl,
}: {
  np: NowPlaying | null;
  onControl: (action: MediaAction) => void;
}) {
  if (!np) {
    return (
      <div className="flex h-[74px] items-center gap-3 text-muted-foreground" data-testid="now-playing-empty">
        <span className="flex size-10 items-center justify-center rounded-md bg-muted">
          <Music2 className="size-4" />
        </span>
        <span className="text-xs">Nothing playing</span>
      </div>
    );
  }
  const playing = np.state === "playing";
  return (
    <div data-testid="now-playing">
      <div className="flex items-center gap-3">
        <div key={trackKey(np)} className="np-enter flex min-w-0 flex-1 items-center gap-3">
            <Artwork np={np} className="size-10 rounded-md" />
            <div className="min-w-0">
              <div className="flex items-center gap-1.5">
                <PlayingBars playing={playing} className="text-primary" />
                <span className="truncate text-sm font-medium" title={np.title}>
                  {np.title || "Untitled"}
                </span>
              </div>
              <div className="truncate text-xs text-muted-foreground" title={np.artist}>
                {np.artist || np.app}
              </div>
            </div>
        </div>
        <div className="flex shrink-0 items-center">
          <IconButton tone="theme" size="sm" label="Previous" disabled={!np.canPrevious} onClick={() => onControl({ kind: "previous" })}>
            <SkipBack className="size-3.5" fill="currentColor" />
          </IconButton>
          <PlayPauseButton tone="theme" size="sm" playing={playing} disabled={!np.canPlayPause} onClick={() => onControl({ kind: "playPause" })} />
          <IconButton tone="theme" size="sm" label="Next" disabled={!np.canNext} onClick={() => onControl({ kind: "next" })}>
            <SkipForward className="size-3.5" fill="currentColor" />
          </IconButton>
        </div>
      </div>
      {np.durationMs != null && (
        <div className="mt-1.5">
          <ProgressBar compact np={np} tone="theme" onSeek={(positionMs) => onControl({ kind: "seek", positionMs })} />
        </div>
      )}
    </div>
  );
}

/** The device the headphones are playing from, by what they call it. */
function sourceName(hp: HeadphonesPlaying): string {
  if (hp.thisDevice) return hostPlatform() === "android" ? "This phone" : "This computer";
  if (hp.cable) return "Audio cable";
  return hp.device ?? "Another device";
}

const STATE_WORD: Record<HeadphonesPlaying["state"], string> = {
  playing: "Playing",
  paused: "Paused",
  stopped: "Stopped",
};

function SourceTile({ hp, className, iconClass }: { hp: HeadphonesPlaying; className: string; iconClass: string }) {
  const Icon = hp.cable ? Cable : MonitorSmartphone;
  return (
    <div className={cn("flex shrink-0 items-center justify-center", className)}>
      <Icon className={iconClass} strokeWidth={1.6} />
    </div>
  );
}

/**
 * What the headphones are playing from another device — a phone, say — on
 * the dashboard: the device, whether it plays, and play/pause and skip
 * passed on to it through the headphones. No track and no position: the
 * headphones keep the track for the Bose app's signed-in connection, and
 * nothing here makes one up.
 */
export function PlayingElsewhereCard({
  hp,
  onControl,
  busy = false,
}: {
  hp: HeadphonesPlaying;
  onControl: (control: PlaybackControl) => void;
  busy?: boolean;
}) {
  const playing = hp.state === "playing";
  const name = sourceName(hp);
  return (
    <div
      className="panel-enter relative isolate overflow-hidden rounded-xl border border-white/10 bg-neutral-900 text-white shadow-[var(--shadow-md)]"
      data-testid="now-playing-elsewhere"
    >
      {/* No artwork to blur: a still gradient, and a soft light that drifts
          while it plays and rests when it does not. Transform only. */}
      <div aria-hidden className="absolute inset-0 -z-10">
        <div className="absolute inset-0 bg-gradient-to-br from-primary/45 via-neutral-900 to-chart-4/35" />
        <div className={cn("np-glow absolute -left-16 top-6 size-56 rounded-full bg-primary/35 blur-3xl", playing && "np-glow-on")} />
        <div className="absolute inset-0 bg-gradient-to-b from-black/35 via-black/35 to-black/65" />
      </div>

      <div className="p-5">
        <div className="flex items-center justify-between gap-3 text-[11px] font-medium uppercase tracking-[0.12em] text-white/60">
          <span className="inline-flex items-center gap-2">
            <PlayingBars playing={playing} />
            Now playing
          </span>
          <span className="inline-flex min-w-0 items-center gap-1.5 normal-case tracking-normal text-white/75">
            <Headphones className="size-3.5 shrink-0" />
            <span className="truncate">{hp.headphones}</span>
          </span>
        </div>

        <div key={name} className="np-enter mt-4 flex items-center gap-4">
          <SourceTile
            hp={hp}
            className="size-16 rounded-lg bg-white/10 text-white/85 ring-1 ring-white/15 shadow-[0_6px_20px_rgba(0,0,0,0.35)]"
            iconClass="size-7"
          />
          <div className="min-w-0">
            <div className="truncate text-base font-semibold leading-tight" title={name}>
              {name}
            </div>
            <div className="mt-0.5 text-sm text-white/70">{STATE_WORD[hp.state]}</div>
            <div className="mt-1 text-xs text-white/45">
              The headphones don&apos;t share the song&apos;s name
            </div>
          </div>
        </div>

        <div className="mt-6 flex items-center justify-center gap-3">
          <IconButton tone="art" label="Previous" disabled={busy} onClick={() => onControl("previous")}>
            <SkipBack className="size-[18px]" fill="currentColor" />
          </IconButton>
          <PlayPauseButton tone="art" playing={playing} disabled={busy} onClick={() => onControl("playPause")} />
          <IconButton tone="art" label="Next" disabled={busy} onClick={() => onControl("next")}>
            <SkipForward className="size-[18px]" fill="currentColor" />
          </IconButton>
        </div>
      </div>
    </div>
  );
}

/** The same, for the desktop widget's row. */
export function PlayingElsewhereRow({
  hp,
  onControl,
  busy = false,
}: {
  hp: HeadphonesPlaying;
  onControl: (control: PlaybackControl) => void;
  busy?: boolean;
}) {
  const playing = hp.state === "playing";
  const name = sourceName(hp);
  return (
    <div className="flex min-h-[74px] items-center gap-3" data-testid="now-playing-elsewhere">
      <div key={name} className="np-enter flex min-w-0 flex-1 items-center gap-3">
        <SourceTile hp={hp} className="size-10 rounded-md bg-muted text-muted-foreground" iconClass="size-5" />
        <div className="min-w-0">
          <div className="flex items-center gap-1.5">
            <PlayingBars playing={playing} className="text-primary" />
            <span className="truncate text-sm font-medium" title={name}>
              {name}
            </span>
          </div>
          <div className="truncate text-xs text-muted-foreground">
            {STATE_WORD[hp.state]} · through {hp.headphones}
          </div>
        </div>
      </div>
      <div className="flex shrink-0 items-center">
        <IconButton tone="theme" size="sm" label="Previous" disabled={busy} onClick={() => onControl("previous")}>
          <SkipBack className="size-3.5" fill="currentColor" />
        </IconButton>
        <PlayPauseButton tone="theme" size="sm" playing={playing} disabled={busy} onClick={() => onControl("playPause")} />
        <IconButton tone="theme" size="sm" label="Next" disabled={busy} onClick={() => onControl("next")}>
          <SkipForward className="size-3.5" fill="currentColor" />
        </IconButton>
      </div>
    </div>
  );
}
