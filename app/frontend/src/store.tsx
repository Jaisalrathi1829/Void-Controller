import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { toast } from "sonner";
import * as ipc from "@/services/ipc";
import type {
  BatteryPart,
  CommandOutcome,
  DeviceCategory,
  DeviceModel,
  DeviceSnapshot,
  NoiseModeOption,
  PairedDevice,
} from "@/types/device";
import type { CapabilityStatus } from "@/types/capability";
import type { PageId } from "./pages";

/* ============================================================
 * Application state, backed by the Rust native layer.
 *
 * This preserves the API the Figma pages were written against, but every
 * value now comes from the backend instead of being hardcoded. Where the
 * backend cannot yet do something, this module says so plainly rather than
 * simulating success — see `docs/capability-matrix.md`.
 * ============================================================ */

export type ConnectionState =
  | "connected"
  | "disconnected"
  | "connecting"
  | "discovering"
  | "reconnecting"
  | "error"
  | "bluetooth-disabled"
  | "device-unavailable"
  | "simulated";

/** Same five states the Rust capability engine uses. */
export type Capability = CapabilityStatus;

export type NoiseMode = "quiet" | "aware" | "custom";

export interface EqValues {
  bass: number;
  mid: number;
  treble: number;
}

export interface Profile {
  id: string;
  name: string;
  eq: EqValues;
  noise: NoiseMode;
  windBlock: boolean;
  lastUsed: string;
}

export interface DiagEvent {
  id: string;
  time: string;
  type: string;
  description: string;
  status: "info" | "user" | "success" | "warning";
}

export interface FeatureCapabilities {
  battery: Capability;
  volume: Capability;
  noiseControl: Capability;
  awareMode: Capability;
  eq: Capability;
  customAnc: Capability;
  multipoint: Capability;
  rename: Capability;
  /** Connecting and disconnecting the devices the headphones are paired with. */
  connections: Capability;
}

export type ThemePref = "system" | "light" | "dark";

interface AppState {
  connection: ConnectionState;
  setConnection: (c: ConnectionState) => void;

  /**
   * True when the active backend is the mock. Deliberately separate from
   * `connection`: a simulated device can be connected, disconnected or
   * reconnecting, and the SIMULATED warning must show in every one of those
   * cases. Folding it into the connection state would hide the warning
   * exactly when the UI looks most like real hardware.
   */
  simulated: boolean;
  /** Switches between the mock and real-hardware backends. */
  setSimulated: (s: boolean) => Promise<void>;
  /** False in a plain browser preview, where no native layer exists. */
  nativeAvailable: boolean;

  battery: number | null;
  charging: boolean;
  deviceName: string;
  /** Provenance of the battery reading, shown so the claim is auditable. */
  batterySource: string | null;

  capabilities: FeatureCapabilities;
  /** Full per-capability detail, including the evidence behind each status. */
  snapshot: DeviceSnapshot | null;

  /** Which model the device is, or null when that cannot be told. */
  model: DeviceModel | null;
  /** Whether the device is headphones or earbuds; headphones when not known. */
  kind: DeviceCategory;
  /** Each battery's level on a device with several. Empty otherwise. */
  batteryParts: BatteryPart[];
  /**
   * The noise modes the device offers, with the one it is in marked. Null
   * until it has said which mode it is in.
   */
  noiseModes: NoiseModeOption[] | null;
  /** Switches to one of the modes the device listed. */
  selectNoiseMode: (mode: NoiseModeOption) => void;

  /**
   * Whether the device chooser is up, and why: on every start, and when the
   * user asks to change device. Null when it is not. The main window shows
   * it in place of everything else.
   */
  choosing: "start" | "change" | null;
  /** Puts the chooser up again. */
  changeDevice: () => void;
  /**
   * Makes the paired device listed under `id` the one this application is
   * about. Resolves to whether that worked; the chooser closes if it did.
   */
  chooseDevice: (id: string, name: string) => Promise<boolean>;
  /** Closes the chooser without choosing: carries on with what is there. */
  closeChooser: () => void;

  /**
   * The mode the device itself last reported, or null if it has not reported
   * one. Never the mode that was merely requested.
   */
  noise: NoiseMode | null;
  setNoise: (n: NoiseMode) => void;
  /** When the device reported `noise`. */
  noiseReadAt: string | null;
  /**
   * Set when the latest attempt to read the device failed. Whatever state is
   * on screen is then the last confirmed one, not a fresh reading.
   */
  deviceNotice: string | null;
  /**
   * Why the device could not be read at all, when it could not. Distinct
   * from `deviceNotice`, which rides on a snapshot that did arrive.
   */
  deviceError: ipc.IpcError | null;
  /** Asks for the Bluetooth permission again (Android). */
  allowBluetooth: () => void;
  /**
   * Noise-cancellation strength the device reports for its active mode, 0-10
   * where 10 is the most. Null when it has not reported one — which includes
   * every time the headphones are in Quiet or Aware.
   */
  ancLevel: number | null;
  setAncLevel: (n: number) => void;
  /** The device's own name for its active custom mode, if it is in one. */
  noiseLabel: string | null;
  windBlock: boolean;
  setWindBlock: (b: boolean) => void;

  volume: number;
  setVolume: (n: number) => void;
  muted: boolean;
  setMuted: (m: boolean) => void;
  /**
   * What the volume belongs to — the Windows endpoint, or on Android the
   * output media is playing through — or null if there is no reading.
   */
  audioEndpointName: string | null;
  /** Whether the system's volume has been read, and so can be shown. */
  volumeKnown: boolean;
  /**
   * What the phone or computer the headphones are connected to is called,
   * while they are connected to this one. Null otherwise.
   */
  connectedTo: string | null;

  /** Whether multipoint is on, as the headphones last said. Null if they have not. */
  multipoint: boolean | null;
  /** Switches multipoint. Off, the headphones keep one connection. */
  setMultipoint: (on: boolean) => void;
  /** What the headphones are paired with. Null if they have not said. */
  pairedDevices: PairedDevice[] | null;
  /** Renames the headphones. Resolves to whether they confirmed the name. */
  renameDevice: (name: string) => Promise<boolean>;
  /**
   * Connects the headphones to one of their paired devices, or disconnects
   * them from it. Resolves when the attempt is over, however it went.
   */
  setDeviceConnected: (device: PairedDevice, on: boolean) => Promise<void>;

  hardwareEq: EqValues;
  setHardwareEq: (e: EqValues) => void;

  profiles: Profile[];
  addProfile: (p: Profile) => void;
  updateProfile: (p: Profile) => void;
  deleteProfile: (id: string) => void;
  applyProfile: (p: Profile) => void;

  events: DiagEvent[];

  theme: ThemePref;
  setTheme: (t: ThemePref) => void;

  /** The Settings page switches, as the native layer reports them. */
  settings: ipc.AppSettings;
  toggleSetting: (k: ipc.SettingKey) => void;
  /** Where voice feedback stands, as the native layer reports it. */
  voice: ipc.VoiceStatus;
  setVoiceOption: (option: ipc.VoiceOption) => void;
  testVoice: () => void;
  stopSpeaking: () => void;
  recheckVoice: () => void;

  connect: () => void;
  reconnect: () => void;
  disconnect: () => void;
  refresh: () => Promise<void>;
  busy: string | null;
}

const AppCtx = createContext<AppState | null>(null);

const now = () => new Date().toLocaleTimeString([], { hour12: false });

/**
 * How often real, connected headphones are re-read. The mode can change from
 * the headphones' own button or from a phone, and the screen should show what
 * the device reports rather than what this app last asked for.
 */
const LIVE_REFRESH_MS = 5000;

/** Whether a capability status means the control must not be offered. */
export function isUnavailable(cap: CapabilityStatus): boolean {
  return cap === "unknown" || cap === "unsupported";
}

/**
 * Whether a feature is shown at all. Only what the device does not have —
 * what its model has none of, or what it said it has no such function for —
 * is left out; something not yet established is shown, as unavailable.
 */
export function isShown(cap: CapabilityStatus): boolean {
  return cap !== "unsupported";
}

/** The pages there is something to show on, for what the device has. */
export function shownPages(caps: FeatureCapabilities): Set<PageId> {
  const pages = new Set<PageId>(["dashboard", "device", "noise", "equalizer", "profiles", "settings"]);
  if (!isShown(caps.noiseControl)) pages.delete("noise");
  if (!isShown(caps.eq)) pages.delete("equalizer");
  // A profile is Quiet, Aware or the custom mode, and an equalizer setting.
  // A device with neither those modes nor an equalizer — the QuietComfort 35
  // generation, whose noise cancellation is a level — has no use for one.
  const profileModes = isShown(caps.noiseControl) && isShown(caps.awareMode);
  if (!profileModes && !isShown(caps.eq)) pages.delete("profiles");
  return pages;
}

const THEME_KEY = "bose-qc.theme";
const PROFILES_KEY = "bose-qc.profiles";

function loadTheme(): ThemePref {
  if (typeof localStorage === "undefined") return "system";
  const raw = localStorage.getItem(THEME_KEY);
  return raw === "light" || raw === "dark" || raw === "system" ? raw : "system";
}

function loadProfiles(): Profile[] {
  if (typeof localStorage === "undefined") return [];
  try {
    const raw = localStorage.getItem(PROFILES_KEY);
    return raw ? (JSON.parse(raw) as Profile[]) : [];
  } catch {
    return [];
  }
}

/** Maps the backend's 14 tracked capabilities onto the 6 the UI renders. */
function mapCapabilities(snapshot: DeviceSnapshot | null): FeatureCapabilities {
  const unknown: Capability = "unknown";
  if (!snapshot) {
    return {
      battery: unknown,
      volume: unknown,
      noiseControl: unknown,
      awareMode: unknown,
      eq: unknown,
      customAnc: unknown,
      multipoint: unknown,
      rename: unknown,
      connections: unknown,
    };
  }
  const c = snapshot.capabilities;
  return {
    battery: c.battery.status,
    volume: c.volume.status,
    noiseControl: c.noiseControl.status,
    awareMode: c.awareMode.status,
    eq: c.equalizer.status,
    customAnc: c.customNoiseControl.status,
    multipoint: c.multipoint.status,
    rename: c.deviceRename.status,
    connections: c.pairedDevices.status,
  };
}

function connectionFrom(snapshot: DeviceSnapshot | null): ConnectionState {
  if (!snapshot) return "disconnected";
  switch (snapshot.connection) {
    case "connected":
      return "connected";
    case "connecting":
      return "connecting";
    case "discovering":
      return "discovering";
    case "reconnecting":
      return "reconnecting";
    case "error":
      return "error";
    default:
      return "disconnected";
  }
}

export function AppProvider({ children }: { children: ReactNode }) {
  const [snapshot, setSnapshot] = useState<DeviceSnapshot | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const busyRef = useRef(false);
  const [events, setEvents] = useState<DiagEvent[]>([]);

  const [windBlock, setWindBlockRaw] = useState(false);

  const [profiles, setProfiles] = useState<Profile[]>(loadProfiles);
  const [theme, setThemeRaw] = useState<ThemePref>(loadTheme);
  // Nothing is assumed about the settings until the native layer has said
  // what they are; until then every switch is off and inert.
  const [settings, setSettings] = useState<ipc.AppSettings>(() =>
    ipc.unavailableSettings("Loading…"),
  );

  const nativeAvailable = ipc.hasNativeLayer();

  /* ---- theme ---- */
  useEffect(() => {
    const root = document.documentElement;
    const apply = () => {
      const prefersDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
      root.classList.toggle("dark", theme === "dark" || (theme === "system" && prefersDark));
    };
    apply();
    if (theme === "system") {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      mq.addEventListener("change", apply);
      return () => mq.removeEventListener("change", apply);
    }
  }, [theme]);

  const setTheme = useCallback((t: ThemePref) => {
    if (typeof localStorage !== "undefined") localStorage.setItem(THEME_KEY, t);
    setThemeRaw(t);
  }, []);

  /* ---- events ---- */
  const pushEvent = useCallback((e: Omit<DiagEvent, "id" | "time">) => {
    setEvents((prev) =>
      [
        {
          id:
            typeof crypto !== "undefined" && crypto.randomUUID
              ? crypto.randomUUID()
              : String(Math.random()),
          time: now(),
          ...e,
        },
        ...prev,
      ].slice(0, 200),
    );
  }, []);

  /* ---- snapshot ---- */
  const [deviceError, setDeviceError] = useState<ipc.IpcError | null>(null);
  const deviceErrorRef = useRef<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const s = await ipc.getSnapshot();
      setSnapshot(s);
      deviceErrorRef.current = null;
      setDeviceError(null);
    } catch (e) {
      const err = ipc.toIpcError(e);
      // Said once, not on every retry of the same failure.
      if (deviceErrorRef.current !== err.message) {
        pushEvent({ type: "ERROR", description: err.message, status: "warning" });
      }
      deviceErrorRef.current = err.message;
      setDeviceError(err);
    }
  }, [pushEvent]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // While the only thing in the way is a permission the user can grant in a
  // system dialog, keep looking: nothing else tells the window it was given.
  const awaitingPermission = deviceError?.kind === "not-permitted";
  useEffect(() => {
    if (!awaitingPermission) return;
    const id = window.setInterval(() => {
      if (!document.hidden) void refresh();
    }, 2000);
    return () => window.clearInterval(id);
  }, [awaitingPermission, refresh]);

  const allowBluetooth = useCallback(() => {
    ipc
      .requestBluetoothPermission()
      .then(() => refresh())
      .catch((e) => {
        toast.error("Could not ask for Bluetooth access.", {
          description: ipc.toIpcError(e).message,
        });
      });
  }, [refresh]);

  /* ---- settings ---- */
  useEffect(() => {
    let cancelled = false;
    ipc
      .getSettings()
      .then((s) => {
        if (!cancelled) setSettings(s);
      })
      .catch((e) => {
        if (!cancelled) setSettings(ipc.unavailableSettings(ipc.toIpcError(e).message));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // A switch flipped in the other window — or by the tray menu, or by the
  // widget being closed — shows here too.
  useEffect(() => {
    let stop: (() => void) | undefined;
    let cancelled = false;
    void ipc
      .onSettingsChanged((s) => {
        if (!cancelled) setSettings(s);
      })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else stop = unlisten;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      stop?.();
    };
  }, []);

  const toggleSetting = useCallback(
    (k: ipc.SettingKey) => {
      const blocked = settings.unavailable[k];
      if (blocked) {
        toast.warning("Not available.", { description: blocked });
        return;
      }
      ipc
        .setSetting(k, !settings[k])
        // What comes back is what the native side did, so that is what shows.
        .then(setSettings)
        .catch(async (e) => {
          toast.error("Could not change the setting.", {
            description: ipc.toIpcError(e).message,
          });
          // Whatever happened, show where things actually stand.
          try {
            setSettings(await ipc.getSettings());
          } catch {
            /* the toast above already says something went wrong */
          }
        });
    },
    [settings],
  );

  /* ---- voice feedback ----
   *
   * The status is whatever the native layer last said. Nothing here guesses
   * that speech "should" be playing: the state changes when the worker that
   * does the speaking says it has.
   */
  const [voice, setVoice] = useState<ipc.VoiceStatus>(() =>
    ipc.voiceUnavailable("Loading…"),
  );

  useEffect(() => {
    let stop: (() => void) | undefined;
    let cancelled = false;
    ipc
      .getVoiceStatus()
      .then((s) => {
        if (!cancelled) setVoice(s);
      })
      .catch((e) => {
        if (!cancelled) setVoice(ipc.voiceUnavailable(ipc.toIpcError(e).message));
      });
    void ipc
      .onVoiceStatus((s) => {
        if (!cancelled) setVoice(s);
      })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else stop = unlisten;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      stop?.();
    };
  }, []);

  const setVoiceOption = useCallback((option: ipc.VoiceOption) => {
    ipc
      .setVoiceOption(option)
      .then(setSettings)
      .catch(async (e) => {
        toast.error("Could not change the voice setting.", {
          description: ipc.toIpcError(e).message,
        });
        try {
          setSettings(await ipc.getSettings());
        } catch {
          /* the toast above already says something went wrong */
        }
      });
  }, []);

  const testVoice = useCallback(() => {
    ipc.testVoice().catch((e) => {
      toast.error("Could not test the voice.", { description: ipc.toIpcError(e).message });
    });
  }, []);

  const stopSpeaking = useCallback(() => {
    ipc.stopSpeaking().catch((e) => {
      toast.error("Could not stop the speech.", { description: ipc.toIpcError(e).message });
    });
  }, []);

  const recheckVoice = useCallback(() => {
    ipc
      .recheckVoiceModel()
      .then(setVoice)
      .catch((e) => {
        toast.error("Could not check for the voice model.", {
          description: ipc.toIpcError(e).message,
        });
      });
  }, []);

  /* ---- headphones coming and going ----
   *
   * The native layer watches for this, because the window stops refreshing
   * once the headphones are gone and so could never notice them come back.
   */
  useEffect(() => {
    let stop: (() => void) | undefined;
    let cancelled = false;
    void ipc
      .onDeviceChanged(() => {
        // Not while nobody is looking: the native layer goes on watching and
        // announcing by itself, and the screen catches up when it is shown.
        if (!busyRef.current && !document.hidden) void refresh();
      })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else stop = unlisten;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      stop?.();
    };
  }, [refresh]);

  /* ---- command dispatch ----
   *
   * Every mutating action goes through here. The outcome from the backend
   * decides what the user is told: `applied` is the only success. A command
   * that was transmitted but not confirmed produces a warning, never a
   * success toast — the device not echoing its state back is exactly the
   * situation the user needs to know about.
   */
  const report = useCallback(
    async (
      kind: string,
      dispatch: () => Promise<CommandOutcome>,
      describe: { label: string; eventType: string },
    ): Promise<CommandOutcome | null> => {
      setBusy(kind);
      busyRef.current = true;
      try {
        const outcome = await dispatch();

        switch (outcome.kind) {
          case "applied":
            toast.success(`${describe.label} applied.`, {
              description: "Confirmed by the device.",
            });
            pushEvent({
              type: describe.eventType,
              description: `${describe.label} — confirmed`,
              status: "success",
            });
            break;
          case "sent-unverified":
            toast.warning(`${describe.label}: state could not be verified.`, {
              description: outcome.reason,
            });
            pushEvent({
              type: describe.eventType,
              description: `${describe.label} — sent, unverified`,
              status: "warning",
            });
            break;
          case "rejected":
            toast.error(`${describe.label} rejected.`, { description: outcome.reason });
            pushEvent({
              type: describe.eventType,
              description: `${describe.label} — rejected`,
              status: "warning",
            });
            break;
          case "unsupported":
            toast.warning(`${describe.label} is not available.`, {
              description: outcome.reason,
            });
            pushEvent({
              type: describe.eventType,
              description: `${describe.label} — unsupported`,
              status: "warning",
            });
            break;
        }

        // Never assume the command took effect: re-read and render what the
        // device actually reports.
        await refresh();
        return outcome;
      } catch (e) {
        const err = ipc.toIpcError(e);
        toast.error(`${describe.label} failed.`, { description: err.message });
        pushEvent({
          type: "ERROR",
          description: `${describe.label} — ${err.message}`,
          status: "warning",
        });
        // A failure says nothing about what state the device is in now, so
        // ask it rather than leaving the screen as it was.
        await refresh();
        return null;
      } finally {
        busyRef.current = false;
        setBusy(null);
      }
    },
    [pushEvent, refresh],
  );

  const run = useCallback(
    (command: ipc.DeviceCommand, describe: { label: string; eventType: string }) =>
      report(command.kind, () => ipc.executeCommand(command), describe),
    [report],
  );

  /*
   * Noise modes go through the typed operation, which cannot carry anything
   * but a mode name — no index, no slot, no bytes.
   */
  const requestNoise = useCallback(
    (n: NoiseMode, describe: { label: string; eventType: string }) =>
      report("setNoiseControlMode", () => ipc.setNoiseControlMode(n), describe),
    [report],
  );

  /* ---- live refresh ---- */
  const live = snapshot?.source === "real" && snapshot.connection === "connected";
  useEffect(() => {
    if (!live) return;
    const id = window.setInterval(() => {
      // Not while a command is in flight (it re-reads when it finishes), and
      // not while nobody is looking.
      if (busyRef.current || document.hidden) return;
      void refresh();
    }, LIVE_REFRESH_MS);
    return () => window.clearInterval(id);
  }, [live, refresh]);

  /* ---- coming back into view ----
   *
   * Nothing is re-read while the window is hidden, so what it shows may be
   * old by the time it is looked at again. Read it then.
   */
  useEffect(() => {
    const onVisible = () => {
      if (!document.hidden && !busyRef.current) void refresh();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [refresh]);

  /* ---- choosing the device ---- */
  const [choosing, setChoosing] = useState<"start" | "change" | null>("start");
  const changeDevice = useCallback(() => setChoosing("change"), []);
  const closeChooser = useCallback(() => setChoosing(null), []);
  const chooseDevice = useCallback(
    async (id: string, name: string) => {
      setBusy("chooseDevice");
      busyRef.current = true;
      try {
        const next = await ipc.chooseBoseDevice(id);
        setSnapshot(next);
        deviceErrorRef.current = null;
        setDeviceError(null);
        setChoosing(null);
        pushEvent({ type: "SESSION", description: `Using ${name}`, status: "info" });
        return true;
      } catch (e) {
        toast.error(`Could not use ${name}.`, { description: ipc.toIpcError(e).message });
        return false;
      } finally {
        busyRef.current = false;
        setBusy(null);
      }
    },
    [pushEvent],
  );

  /* ---- derived ---- */
  const connection = connectionFrom(snapshot);
  const simulated = snapshot?.source === "mock";
  const capabilities = useMemo(() => mapCapabilities(snapshot), [snapshot]);

  const battery = snapshot?.battery?.percent ?? null;
  const charging = snapshot?.battery?.charging ?? false;
  const batterySource = snapshot?.battery?.source ?? null;
  const deviceName = snapshot?.identity?.name ?? "No device";

  const noise = (snapshot?.noiseControl?.mode ?? null) as NoiseMode | null;
  const noiseReadAt = snapshot?.noiseControl?.readAt ?? null;
  const ancLevel = snapshot?.noiseControl?.level ?? null;
  const noiseLabel = snapshot?.noiseControl?.label ?? null;
  const deviceNotice = snapshot?.lastError ?? null;
  const hardwareEq: EqValues = snapshot?.equalizer
    ? {
        bass: snapshot.equalizer.bass,
        mid: snapshot.equalizer.mid,
        treble: snapshot.equalizer.treble,
      }
    : { bass: 0, mid: 0, treble: 0 };

  const volume = snapshot?.systemAudio?.volumePercent ?? 0;
  const muted = snapshot?.systemAudio?.muted ?? false;
  const audioEndpointName = snapshot?.systemAudio?.endpointName ?? null;
  const volumeKnown = !!snapshot?.systemAudio;
  // Only real headphones are connected to anything, and only while they are.
  const connectedTo =
    snapshot?.source === "real" && snapshot.connection === "connected"
      ? (snapshot.hostName ?? null)
      : null;
  const multipoint = snapshot?.multipoint?.enabled ?? null;
  const pairedDevices = snapshot?.pairedDevices ?? null;
  const model = snapshot?.model ?? null;
  const kind: DeviceCategory = model?.category ?? "headphones";
  const batteryParts = useMemo(() => snapshot?.battery?.parts ?? [], [snapshot]);
  const noiseModes = snapshot?.noiseModes ?? null;

  /* ---- actions ---- */
  const setNoise = useCallback(
    (n: NoiseMode) => {
      void requestNoise(n, { label: `Noise control (${n})`, eventType: "ANC_CHANGE" });
    },
    [requestNoise],
  );

  const selectNoiseMode = useCallback(
    (mode: NoiseModeOption) => {
      void report("selectNoiseMode", () => ipc.selectNoiseMode(mode.index), {
        label: mode.name,
        eventType: "ANC_CHANGE",
      });
    },
    [report],
  );

  const setHardwareEq = useCallback(
    (e: EqValues) => {
      void run({ kind: "setEqualizer", settings: e }, { label: "Equalizer", eventType: "EQ" });
    },
    [run],
  );

  const setMultipoint = useCallback(
    (on: boolean) => {
      void run(
        { kind: "setMultipoint", enabled: on },
        { label: `Multi-device connection ${on ? "on" : "off"}`, eventType: "MULTIPOINT" },
      ).then((outcome) => {
        // Off, the headphones keep one connection, and they choose which.
        // On the tested pair it was not the device that had asked.
        if (outcome?.kind === "applied" && !on) {
          const here = ipc.hostPlatform() === "android" ? "phone" : "computer";
          toast.info("The headphones now keep one connection.", {
            description: `If it is not this ${here}'s, they have disconnected from it. Reconnect them from Bluetooth settings to use them here.`,
          });
        }
      });
    },
    [run],
  );

  const setDeviceConnected = useCallback(
    async (device: PairedDevice, on: boolean) => {
      const name = device.name || "the device";
      const outcome = await run(
        { kind: "setPairedDeviceConnection", device: device.id, connected: on },
        { label: `${on ? "Connect" : "Disconnect"} ${name}`, eventType: "CONNECTION" },
      );
      // Dropped from the device in the hand, the headphones are no longer
      // this application's to talk to.
      if (outcome?.kind === "applied" && !on && device.thisDevice) {
        const here = ipc.hostPlatform() === "android" ? "phone" : "computer";
        toast.info(`The headphones have disconnected from this ${here}.`, {
          description: "Connect them again from Bluetooth settings to use them here.",
        });
      }
    },
    [run],
  );

  const renameDevice = useCallback(
    async (name: string) => {
      const outcome = await run(
        { kind: "setDeviceName", name },
        { label: "Rename", eventType: "RENAME" },
      );
      return outcome?.kind === "applied";
    },
    [run],
  );

  const setVolume = useCallback(
    (n: number) => {
      void run(
        { kind: "setSystemVolume", percent: Math.round(n) },
        {
          label: ipc.hostPlatform() === "android" ? "Phone volume" : "Windows volume",
          eventType: "VOLUME",
        },
      );
    },
    [run],
  );

  const setMuted = useCallback(
    (m: boolean) => {
      void run({ kind: "setSystemMute", muted: m }, { label: "Mute", eventType: "VOLUME" });
    },
    [run],
  );

  // No local copy of the level is kept: the slider shows what the device
  // reports, and moves when the device reports something new.
  const setAncLevel = useCallback(
    (n: number) => {
      const level = Math.round(n);
      void report("setNoiseControlLevel", () => ipc.setNoiseCancellationLevel(level), {
        label: `Noise cancellation level ${level}`,
        eventType: "ANC_LEVEL",
      });
    },
    [report],
  );

  // No backend command exists for wind block. Rather than silently keeping a
  // local flag that pretends to control hardware, say so.
  const setWindBlock = useCallback(
    (b: boolean) => {
      setWindBlockRaw(b);
      toast.warning("Wind block is not available.", {
        description:
          "No verified interface exposes this on your headphones. The switch reflects your preference only and does not change the device.",
      });
      pushEvent({
        type: "WIND_BLOCK",
        description: "Wind block toggled — no device interface",
        status: "warning",
      });
    },
    [pushEvent],
  );

  const setConnection = useCallback(
    (c: ConnectionState) => {
      if (c === "disconnected") void run({ kind: "disconnect" }, { label: "Disconnect", eventType: "STATE" });
      else if (c === "connected") void run({ kind: "connect" }, { label: "Connect", eventType: "STATE" });
    },
    [run],
  );

  const connect = useCallback(() => {
    void run({ kind: "connect" }, { label: "Connect", eventType: "STATE" });
  }, [run]);

  const reconnect = useCallback(() => {
    void run({ kind: "reconnect" }, { label: "Reconnect", eventType: "STATE" });
  }, [run]);

  const disconnect = useCallback(() => {
    void run({ kind: "disconnect" }, { label: "Disconnect", eventType: "STATE" });
  }, [run]);

  const setSimulated = useCallback(
    async (s: boolean) => {
      setBusy("switch-source");
      try {
        const next = await ipc.setDeviceSource(s ? "mock" : "real");
        setSnapshot(next);
        pushEvent({
          type: "SESSION",
          description: s ? "Switched to simulated device" : "Switched to real hardware",
          status: "info",
        });
      } catch (e) {
        toast.error("Could not switch device source.", {
          description: ipc.toIpcError(e).message,
        });
      } finally {
        setBusy(null);
      }
    },
    [pushEvent],
  );

  /* ---- profiles (local only) ---- */
  const persistProfiles = useCallback((next: Profile[]) => {
    setProfiles(next);
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(PROFILES_KEY, JSON.stringify(next));
    }
  }, []);

  const applyProfile = useCallback(
    async (p: Profile) => {
      pushEvent({ type: "PROFILE", description: `Applying ${p.name}`, status: "user" });

      // A profile only applies settings the device is actually able to accept.
      // Each sub-command reports its own outcome, so a partially-applied
      // profile is visible rather than silently claimed as a success.
      await requestNoise(p.noise, {
        label: `${p.name}: noise control`,
        eventType: "PROFILE",
      });
      await run(
        { kind: "setEqualizer", settings: p.eq },
        { label: `${p.name}: equalizer`, eventType: "PROFILE" },
      );

      persistProfiles(
        profiles.map((i) => (i.id === p.id ? { ...i, lastUsed: "Just now" } : i)),
      );
    },
    [run, requestNoise, pushEvent, profiles, persistProfiles],
  );

  const value: AppState = useMemo(
    () => ({
      connection,
      setConnection,
      simulated: !!simulated,
      setSimulated,
      nativeAvailable,
      battery,
      charging,
      deviceName,
      batterySource,
      capabilities,
      snapshot,
      model,
      kind,
      batteryParts,
      noiseModes,
      selectNoiseMode,
      choosing,
      changeDevice,
      chooseDevice,
      closeChooser,
      noise,
      setNoise,
      noiseReadAt,
      deviceNotice,
      deviceError,
      allowBluetooth,
      ancLevel,
      setAncLevel,
      noiseLabel,
      windBlock,
      setWindBlock,
      volume,
      setVolume,
      muted,
      setMuted,
      audioEndpointName,
      volumeKnown,
      connectedTo,
      multipoint,
      setMultipoint,
      pairedDevices,
      renameDevice,
      setDeviceConnected,
      hardwareEq,
      setHardwareEq,
      profiles,
      addProfile: (p) => {
        persistProfiles([...profiles, p]);
        toast.success(`Profile “${p.name}” created.`, {
          description: "Stored locally in this application.",
        });
      },
      updateProfile: (p) => {
        persistProfiles(profiles.map((i) => (i.id === p.id ? p : i)));
        toast.success(`Profile “${p.name}” saved.`);
      },
      deleteProfile: (id) => persistProfiles(profiles.filter((i) => i.id !== id)),
      applyProfile: (p) => void applyProfile(p),
      events,
      theme,
      setTheme,
      settings,
      toggleSetting,
      voice,
      setVoiceOption,
      testVoice,
      stopSpeaking,
      recheckVoice,
      connect,
      reconnect,
      disconnect,
      refresh,
      busy,
    }),
    [
      connection, setConnection, simulated, setSimulated, nativeAvailable, connect,
      battery, charging, deviceName, batterySource, capabilities, snapshot,
      model, kind, batteryParts, noiseModes, selectNoiseMode,
      choosing, changeDevice, chooseDevice, closeChooser,
      noise, setNoise, noiseReadAt, deviceNotice, deviceError, allowBluetooth,
      ancLevel, setAncLevel, noiseLabel, windBlock, setWindBlock,
      volume, setVolume, muted, setMuted, audioEndpointName, volumeKnown, connectedTo,
      multipoint, setMultipoint, pairedDevices, renameDevice, setDeviceConnected,
      hardwareEq, setHardwareEq,
      profiles, persistProfiles, applyProfile, events,
      theme, setTheme, settings, toggleSetting, reconnect, disconnect, refresh, busy,
      voice, setVoiceOption, testVoice, stopSpeaking, recheckVoice,
    ],
  );

  return <AppCtx.Provider value={value}>{children}</AppCtx.Provider>;
}

export function useApp() {
  const ctx = useContext(AppCtx);
  if (!ctx) throw new Error("useApp must be used within AppProvider");
  return ctx;
}

/**
 * Capability metadata.
 *
 * The notes are worded to keep `supported` and `verified` clearly distinct:
 * an interface existing is not the same claim as the device demonstrating it.
 */
export const capabilityMeta: Record<
  Capability,
  { label: string; tone: "success" | "info" | "neutral" | "warning" | "muted"; note: string }
> = {
  verified: {
    label: "Verified",
    tone: "success",
    note: "Confirmed working against your physical headphones.",
  },
  supported: {
    label: "Supported",
    tone: "info",
    note: "A valid interface appears to expose this, but it has not been confirmed on your headphones.",
  },
  unknown: {
    label: "Unknown",
    tone: "neutral",
    note: "Not yet established whether this is accessible on this device.",
  },
  ready: {
    label: "Ready",
    tone: "info",
    note: "Your headphones report this, and every change is checked by reading it back from them.",
  },
  unsupported: {
    label: "Unsupported",
    tone: "muted",
    note: "Cannot currently be accessed safely through available interfaces.",
  },
};
