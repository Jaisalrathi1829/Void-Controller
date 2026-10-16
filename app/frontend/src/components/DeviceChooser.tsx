import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowLeft, Bluetooth, ChevronRight, FlaskConical, Loader2, RefreshCw, X } from "lucide-react";
import { motion } from "motion/react";
import { toast } from "sonner";
import { DeviceMark, DeviceVisual } from "./DeviceVisual";
import { Button } from "./primitives";
import { cn } from "./ui/utils";
import { useApp } from "../store";
import * as ipc from "@/services/ipc";
import { supportNote, type DeviceCategory, type PairedBose } from "@/types/device";
import wolfLogo from "@/assets/wolf-logo.png";

/** How long the device that was found is shown before it opens. */
export const FOUND_PAUSE_MS = 900;

/** The shortest a search is shown for, so that it does not merely flash. */
const SEARCH_AT_LEAST_MS = 600;

const KIND_KEY = "void.kind";

const KINDS: { kind: DeviceCategory; label: string; examples: string }[] = [
  {
    kind: "headphones",
    label: "Headphones",
    examples: "QuietComfort, QuietComfort Ultra, QuietComfort 45, 700",
  },
  {
    kind: "earbuds",
    label: "Earbuds",
    examples: "QuietComfort Earbuds, QuietComfort Ultra Earbuds, Ultra Open",
  },
];

const noun = (kind: DeviceCategory) => (kind === "earbuds" ? "earbuds" : "headphones");

function lastKind(): DeviceCategory | null {
  try {
    const raw = localStorage.getItem(KIND_KEY);
    return raw === "headphones" || raw === "earbuds" ? raw : null;
  } catch {
    return null;
  }
}

function rememberKind(kind: DeviceCategory) {
  try {
    localStorage.setItem(KIND_KEY, kind);
  } catch {
    /* only a convenience */
  }
}

async function openBluetoothSettings() {
  try {
    await ipc.openBluetoothSettings();
  } catch (e) {
    toast.error("Could not open Bluetooth settings.", { description: ipc.toIpcError(e).message });
  }
}

/** What the search found, sorted into what the screen says about it. */
interface Found {
  /** Connected, and of the kind asked for. */
  connected: PairedBose[];
  /** Of the kind asked for, paired, not connected. */
  paired: PairedBose[];
  /** Connected Bose devices whose model Void does not know. */
  unknown: PairedBose[];
  /** Connected, and of the other kind. */
  otherKind: PairedBose[];
}

function sort(devices: PairedBose[], kind: DeviceCategory): Found {
  const mine = devices.filter((d) => d.model?.category === kind);
  return {
    connected: mine.filter((d) => d.connected),
    paired: mine.filter((d) => !d.connected),
    unknown: devices.filter((d) => !d.model && d.connected),
    otherKind: devices.filter((d) => d.model && d.model.category !== kind && d.connected),
  };
}

/**
 * The chooser's page. The glows are behind it and do not scroll; each step
 * is a scroller of its own, so a new step starts at its top.
 */
function Screen({ step, children }: { step: string; children: React.ReactNode }) {
  return (
    <div className="relative h-dvh w-full overflow-hidden bg-background text-foreground">
      <div aria-hidden className="pointer-events-none absolute inset-0 overflow-hidden">
      <div
        aria-hidden
        className="ambient-glow ambient-glow-a pointer-events-none absolute -left-72 -top-72 size-[44rem] rounded-full"
        style={{
          background:
            "radial-gradient(circle, color-mix(in oklch, var(--primary) 13%, transparent) 0%, color-mix(in oklch, var(--primary) 6%, transparent) 35%, transparent 68%)",
        }}
      />
      <div
        aria-hidden
        className="ambient-glow ambient-glow-b pointer-events-none absolute -bottom-72 -right-40 size-[48rem] rounded-full"
        style={{
          background:
            "radial-gradient(circle, color-mix(in oklch, var(--chart-4) 11%, transparent) 0%, color-mix(in oklch, var(--chart-4) 5%, transparent) 35%, transparent 68%)",
        }}
      />
      </div>
      <div key={step} className="absolute inset-0 overflow-auto">
        <div className="relative mx-auto flex min-h-full w-full max-w-xl flex-col px-4 py-8 md:py-12">
          {children}
        </div>
      </div>
    </div>
  );
}

function Brand() {
  return (
    <div className="flex items-center gap-3">
      <div className="flex size-11 shrink-0 items-center justify-center overflow-hidden rounded-xl bg-white ring-1 ring-border shadow-[var(--shadow-md)]">
        <img src={wolfLogo} alt="" className="size-full object-cover" />
      </div>
      <div className="text-lg font-semibold tracking-[0.2em]">VOID</div>
    </div>
  );
}

/** One device to choose. */
function DeviceRow({
  device,
  onChoose,
  disabled,
}: {
  device: PairedBose;
  onChoose: (d: PairedBose) => void;
  disabled: boolean;
}) {
  const kind = device.model?.category ?? "headphones";
  return (
    <li>
      <button
        onClick={() => onChoose(device)}
        disabled={disabled}
        className={cn(
          "flex w-full items-center gap-3.5 rounded-xl border border-border bg-card px-4 py-3 text-left outline-none transition-colors",
          "hover:border-primary/40 hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50",
        )}
      >
        <span
          aria-hidden
          className="flex size-12 shrink-0 items-center justify-center rounded-xl bg-surface-2 p-1.5"
        >
          <DeviceMark kind={kind} active={device.connected} />
        </span>
        <span className="min-w-0 flex-1">
          <span className="block truncate text-base font-medium">{device.name}</span>
          <span className="block truncate text-sm text-muted-foreground">
            {device.model?.name ?? "Model not recognized"}
          </span>
          <span className="mt-0.5 block text-xs text-muted-foreground" data-testid="device-row-state">
            {[
              device.connected ? "Connected" : "Not connected",
              device.model?.support === "tested" ? "Tested with Void" : null,
              device.current ? "In use" : null,
            ]
              .filter(Boolean)
              .join(" · ")}
          </span>
        </span>
        <ChevronRight className="size-4 shrink-0 text-muted-foreground" />
      </button>
    </li>
  );
}

/**
 * The first thing the main window shows: headphones or earbuds, then the
 * connected device of that kind, found and opened.
 *
 * Nothing here talks to any device. The search lists what the system knows
 * about its paired devices; choosing one tells the native layer which device
 * this application is about, and it is from then on that the device itself
 * is asked anything — its name, its modes, what it has.
 */
export function DeviceChooser() {
  const { choosing, chooseDevice, closeChooser, setSimulated, nativeAvailable, busy } = useApp();
  const [kind, setKind] = useState<DeviceCategory | null>(null);
  const [found, setFound] = useState<Found | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [opening, setOpening] = useState<PairedBose | null>(null);
  const [preferred] = useState(lastKind);
  const search = useRef(0);

  const choose = useCallback(
    async (device: PairedBose) => {
      setOpening(device);
      const ok = await chooseDevice(device.id, device.name);
      if (!ok) setOpening(null);
    },
    [chooseDevice],
  );

  const look = useCallback(
    async (wanted: DeviceCategory) => {
      const mine = ++search.current;
      setFound(null);
      setFailed(null);
      setOpening(null);
      const started = Date.now();
      let devices: PairedBose[] = [];
      let error: string | null = null;
      try {
        devices = await ipc.listBoseDevices();
      } catch (e) {
        error = ipc.toIpcError(e).message;
      }
      const left = SEARCH_AT_LEAST_MS - (Date.now() - started);
      if (left > 0) await new Promise((r) => setTimeout(r, left));
      if (search.current !== mine) return;
      if (error) {
        setFailed(error);
        return;
      }
      const sorted = sort(devices, wanted);
      setFound(sorted);
      // One connected, and nothing else connected that it could be
      // confused with: it is the one, and it opens by itself.
      const only = sorted.connected[0];
      if (only && sorted.connected.length === 1 && sorted.unknown.length === 0) {
        setOpening(only);
        await new Promise((r) => setTimeout(r, FOUND_PAUSE_MS));
        if (search.current !== mine) return;
        await choose(only);
      }
    },
    [choose],
  );

  const pick = (wanted: DeviceCategory) => {
    rememberKind(wanted);
    setKind(wanted);
    void look(wanted);
  };

  const back = () => {
    search.current += 1;
    setKind(null);
    setFound(null);
    setFailed(null);
    setOpening(null);
  };

  // Escape backs out, or closes a chooser that was opened to change device.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (kind) back();
      else if (choosing === "change") closeChooser();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const trySimulated = async () => {
    await setSimulated(true);
    closeChooser();
  };

  /* ---- headphones or earbuds ---- */
  if (!kind) {
    return (
      <Screen step="kind">
        <div className="flex items-center justify-between">
          <Brand />
          {choosing === "change" && (
            <button
              onClick={closeChooser}
              aria-label="Keep the current device"
              className="rounded-md p-2 text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
            >
              <X className="size-5" />
            </button>
          )}
        </div>
        <div className="my-auto py-10">
          <h1 className="text-2xl font-semibold md:text-3xl">What are you using?</h1>
          <p className="mt-2 text-sm text-muted-foreground">
            Void looks for the one that is connected, works out which model it is, and shows what
            that model can do.
          </p>
          <div className="mt-8 grid grid-cols-1 gap-3 sm:grid-cols-2">
            {KINDS.map((k, i) => (
              <motion.button
                key={k.kind}
                onClick={() => pick(k.kind)}
                initial={{ opacity: 0, y: 12 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ delay: 0.05 + i * 0.07 }}
                whileHover={{ y: -4 }}
                whileTap={{ scale: 0.98 }}
                className={cn(
                  "flex flex-col items-center rounded-2xl border bg-card px-5 pb-5 pt-4 text-center outline-none transition-colors",
                  "hover:border-primary/50 focus-visible:ring-2 focus-visible:ring-ring",
                  preferred === k.kind ? "border-primary/40" : "border-border",
                )}
              >
                <span aria-hidden className="flex size-32 items-center justify-center">
                  <DeviceMark kind={k.kind} />
                </span>
                <span className="mt-1 text-lg font-semibold">{k.label}</span>
                <span className="mt-1 text-xs text-muted-foreground">{k.examples}</span>
              </motion.button>
            ))}
          </div>
        </div>
      </Screen>
    );
  }

  const header = (
    <div className="flex items-center justify-between">
      <button
        onClick={back}
        className="inline-flex items-center gap-1.5 rounded-md px-2 py-1.5 text-sm text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
      >
        <ArrowLeft className="size-4" /> Back
      </button>
      <Brand />
    </div>
  );

  /* ---- found the one: it opens ---- */
  if (opening) {
    const openingKind = opening.model?.category ?? kind;
    return (
      <Screen step="found">
        {header}
        <div className="my-auto flex flex-col items-center py-10 text-center" data-testid="device-found">
          <DeviceVisual size={240} kind={openingKind} active={opening.connected} />
          <p className="mt-4 text-sm text-muted-foreground">Found</p>
          <h1 className="text-2xl font-semibold">{opening.name}</h1>
          <p className="mt-1 text-sm">{opening.model?.name ?? "A Bose device Void does not know"}</p>
          {opening.model && (
            <p className="mt-3 max-w-sm text-xs text-muted-foreground">
              {supportNote(opening.model.support)}
            </p>
          )}
          <p className="mt-5 inline-flex items-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="size-4 animate-spin text-primary" /> Opening…
          </p>
        </div>
      </Screen>
    );
  }

  /* ---- looking ---- */
  if (!found && !failed) {
    return (
      <Screen step="searching">
        {header}
        <div className="my-auto flex flex-col items-center py-10 text-center" data-testid="device-searching">
          <DeviceVisual size={220} kind={kind} active />
          <h1 className="mt-4 text-xl font-semibold">Looking for your {noun(kind)}…</h1>
          <p className="mt-2 max-w-sm text-sm text-muted-foreground">
            Checking which Bose {noun(kind)} are connected to this{" "}
            {ipc.hostPlatform() === "android" ? "phone" : "computer"}.
          </p>
        </div>
      </Screen>
    );
  }

  const choosingDisabled = busy === "chooseDevice";
  // A device of the other kind that is connected, when none of this kind is.
  const other = found?.otherKind[0];
  const otherKind = other?.model ? { device: other, kind: other.model.category } : null;

  /* ---- what the search found ---- */
  return (
    <Screen step="results">
      {header}
      <div className="py-8">
        {failed ? (
          <>
            <h1 className="text-xl font-semibold">Could not look for {noun(kind)}.</h1>
            <p className="mt-2 text-sm text-muted-foreground">{failed}</p>
          </>
        ) : found && found.connected.length > 0 ? (
          <>
            <h1 className="text-xl font-semibold">Which {noun(kind)}?</h1>
            <p className="mt-2 text-sm text-muted-foreground">
              {found.connected.length > 1
                ? "More than one is connected. Choose the one to control."
                : "Choose the one to control."}
            </p>
            <ul className="mt-5 space-y-2" aria-label={`Connected ${noun(kind)}`}>
              {found.connected.map((d) => (
                <DeviceRow key={d.id} device={d} onChoose={choose} disabled={choosingDisabled} />
              ))}
            </ul>
          </>
        ) : (
          <>
            <h1 className="text-xl font-semibold">No Bose {noun(kind)} connected.</h1>
            <p className="mt-2 text-sm text-muted-foreground">
              Turn them on and connect them in Bluetooth settings, then search again.
            </p>
          </>
        )}

        {found && found.unknown.length > 0 && (
          <section className="mt-6">
            <h2 className="text-sm font-medium text-muted-foreground">Other Bose devices</h2>
            <p className="mt-1 text-xs text-muted-foreground">
              Connected, but Void does not know which model they are, so only what the system
              reports about them can be shown.
            </p>
            <ul className="mt-3 space-y-2">
              {found.unknown.map((d) => (
                <DeviceRow key={d.id} device={d} onChoose={choose} disabled={choosingDisabled} />
              ))}
            </ul>
          </section>
        )}

        {found && found.connected.length === 0 && otherKind && (
          <section className="mt-6 rounded-xl border border-border bg-card/60 p-4" data-testid="other-kind">
            <p className="text-sm">
              <span className="font-medium">{otherKind.device.name}</span> is connected —{" "}
              {otherKind.kind === "earbuds" ? "a pair of earbuds" : "a pair of headphones"}.
            </p>
            <Button
              size="sm"
              variant="outline"
              className="mt-3"
              onClick={() => pick(otherKind.kind)}
            >
              Look for {noun(otherKind.kind)} instead
            </Button>
          </section>
        )}

        {found && found.paired.length > 0 && (
          <section className="mt-6">
            <h2 className="text-sm font-medium text-muted-foreground">
              Paired, not connected
            </h2>
            <ul className="mt-3 space-y-2">
              {found.paired.map((d) => (
                <DeviceRow key={d.id} device={d} onChoose={choose} disabled={choosingDisabled} />
              ))}
            </ul>
          </section>
        )}

        <div className="mt-8 flex flex-wrap gap-2">
          <Button icon={<RefreshCw className="size-4" />} onClick={() => void look(kind)}>
            Search again
          </Button>
          {nativeAvailable && (
            <Button
              variant="outline"
              icon={<Bluetooth className="size-4" />}
              onClick={() => void openBluetoothSettings()}
            >
              Bluetooth settings
            </Button>
          )}
          {nativeAvailable && found && found.connected.length === 0 && (
            <Button
              variant="ghost"
              icon={<FlaskConical className="size-4" />}
              onClick={() => void trySimulated()}
            >
              Try a simulated device
            </Button>
          )}
        </div>
      </div>
    </Screen>
  );
}
