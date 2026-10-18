import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as ipc from "@/services/ipc";
import {
  chooseShown,
  type MediaAction,
  type NowPlayingView,
  type PlaybackControl,
} from "@/types/media";

/** How often what is playing is read while the window is on screen. */
const POLL_MS = 1000;

/**
 * What is playing — on this computer, and through the headphones from
 * whichever device they play from — read once a second while the window is
 * on screen, which of them to show, and a way to control each. After a
 * control the state is read again straight away and twice more, so that
 * what the application or device did shows as soon as it has done it — the
 * control itself is never shown as done.
 */
export function useNowPlaying(enabled = true) {
  const [view, setView] = useState<NowPlayingView | null>(null);
  const [loaded, setLoaded] = useState(false);
  /** A control is on its way through the headphones: they take one at a time. */
  const [passing, setPassing] = useState(false);
  const pending = useRef<number[]>([]);

  const refresh = useCallback(async () => {
    try {
      setView(await ipc.getNowPlaying());
    } catch {
      setView(null);
    } finally {
      setLoaded(true);
    }
  }, []);

  const readSoon = useCallback(() => {
    pending.current.forEach((t) => window.clearTimeout(t));
    pending.current = [80, 400, 1100].map((ms) => window.setTimeout(() => void refresh(), ms));
  }, [refresh]);

  useEffect(() => {
    if (!enabled) return;
    void refresh();
    const id = window.setInterval(() => {
      if (!document.hidden) void refresh();
    }, POLL_MS);
    const onVisible = () => {
      if (!document.hidden) void refresh();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      window.clearInterval(id);
      document.removeEventListener("visibilitychange", onVisible);
      pending.current.forEach((t) => window.clearTimeout(t));
    };
  }, [enabled, refresh]);

  const control = useCallback(
    async (action: MediaAction) => {
      try {
        await ipc.mediaControl(action);
      } catch {
        /* the next read says how things stand */
      }
      readSoon();
    },
    [readSoon],
  );

  const controlElsewhere = useCallback(
    async (what: PlaybackControl) => {
      if (passing) return;
      setPassing(true);
      try {
        await ipc.headphonesPlayback(what);
      } catch {
        /* the next read says how things stand */
      } finally {
        setPassing(false);
      }
      readSoon();
    },
    [passing, readSoon],
  );

  const shown = useMemo(() => chooseShown(view), [view]);

  return { view, shown, loaded, control, controlElsewhere, passing };
}
