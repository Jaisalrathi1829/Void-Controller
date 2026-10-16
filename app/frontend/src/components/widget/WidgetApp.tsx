import type { ReactNode } from "react";
import { Maximize2, Pin, PinOff, X } from "lucide-react";
import * as ipc from "@/services/ipc";
import { AppProvider, isShown, isUnavailable, useApp } from "../../store";
import { NoiseModePicker } from "../NoiseModes";
import { BatteryIndicator, Range, SectionLabel } from "../primitives";
import { cn } from "../ui/utils";

/** A small square button in the widget's header. */
function HeaderButton({
  label,
  onClick,
  active,
  children,
}: {
  label: string;
  onClick: () => void;
  active?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={active}
      onClick={onClick}
      className={cn(
        "flex size-7 items-center justify-center rounded-md outline-none transition-colors",
        "hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring",
        active ? "text-primary" : "text-muted-foreground",
      )}
    >
      {children}
    </button>
  );
}

const signed = (n: number) => (n > 0 ? `+${n}` : `${n}`);

const STATUS_TONE: Record<string, string> = {
  success: "text-success",
  warning: "text-warning",
};

/**
 * The widget: the modes and the equalizer, and nothing else.
 *
 * It is the same application as the main window, in a second, smaller
 * window. Every control here calls what the same control in the main window
 * calls, and shows what the headphones report, never what was asked for. The
 * one difference is where an outcome is said: there is no room for a toast,
 * so the last outcome is the line at the bottom.
 */
export function Widget() {
  const {
    deviceName,
    connection,
    simulated,
    battery,
    charging,
    kind,
    capabilities,
    hardwareEq,
    setHardwareEq,
    busy,
    settings,
    toggleSetting,
    events,
    deviceNotice,
    connectedTo,
  } = useApp();

  const connected = connection === "connected";
  const eqOff = isUnavailable(capabilities.eq);
  // Only what the device has: a section for something it has none of is
  // left out.
  const noiseShown = isShown(capabilities.noiseControl);
  const eqShown = isShown(capabilities.eq);
  const what = kind === "earbuds" ? "Earbuds" : "Headphones";

  const last = events[0];
  const status = deviceNotice
    ? { text: deviceNotice, tone: "text-warning" }
    : last
      ? { text: last.description, tone: STATUS_TONE[last.status] ?? "text-muted-foreground" }
      : connected && connectedTo
        ? { text: `Connected to ${connectedTo}`, tone: "text-muted-foreground" }
        : null;

  const band = (key: "bass" | "mid" | "treble", label: string) => (
    <Range
      label={label}
      min={-10}
      max={10}
      value={hardwareEq[key]}
      displayValue={signed(hardwareEq[key])}
      disabled={eqOff || busy === "setEqualizer"}
      onChange={(v) => setHardwareEq({ ...hardwareEq, [key]: v })}
    />
  );

  return (
    <div className="h-dvh w-full p-2.5 text-foreground">
      <div className="flex h-full flex-col overflow-hidden rounded-2xl border border-border bg-card shadow-[0_2px_8px_rgba(0,0,0,0.28)]">
        {/* The header is what the widget is dragged by. Everything in it
            that is not a button lets the pointer through to it. */}
        <header
          data-tauri-drag-region
          className="flex h-11 shrink-0 items-center gap-2 border-b border-border pl-3.5 pr-1.5"
        >
          <span
            aria-hidden
            className={cn(
              "pointer-events-none size-2 shrink-0 rounded-full",
              connected ? "bg-success" : "bg-muted-foreground/50",
            )}
          />
          <span className="pointer-events-none min-w-0 flex-1 truncate text-sm font-medium">
            {deviceName}
          </span>
          {simulated && (
            <span className="pointer-events-none rounded bg-warning/15 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-warning">
              Simulated
            </span>
          )}
          {connected && (
            <span className="pointer-events-none">
              <BatteryIndicator level={battery} charging={charging} />
            </span>
          )}
          <HeaderButton
            label={settings.widgetOnTop ? "Stop keeping on top" : "Keep on top"}
            active={settings.widgetOnTop}
            onClick={() => toggleSetting("widgetOnTop")}
          >
            {settings.widgetOnTop ? <Pin className="size-3.5" /> : <PinOff className="size-3.5" />}
          </HeaderButton>
          <HeaderButton
            label="Open Void"
            onClick={() => void ipc.showMainWindow().catch(() => {})}
          >
            <Maximize2 className="size-3.5" />
          </HeaderButton>
          <HeaderButton
            label="Hide the widget"
            onClick={() => void ipc.setSetting("desktopWidget", false).catch(() => {})}
          >
            <X className="size-4" />
          </HeaderButton>
        </header>

        {connected ? (
          <div className="flex min-h-0 flex-1 flex-col gap-4 px-4 py-3.5">
            {noiseShown && (
              <section>
                <SectionLabel>Noise control</SectionLabel>
                <NoiseModePicker nameCustom className="mt-2" />
              </section>
            )}
            {eqShown && (
              <section>
                <SectionLabel>Equalizer</SectionLabel>
                <div className="mt-2 space-y-2.5">
                  {band("bass", "Bass")}
                  {band("mid", "Mid")}
                  {band("treble", "Treble")}
                </div>
              </section>
            )}
          </div>
        ) : (
          <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-1 px-6 text-center">
            <p className="text-sm font-medium">{what} not connected</p>
            <p className="text-xs text-muted-foreground">
              The modes and the equalizer appear here when they are.
            </p>
          </div>
        )}

        {/* The last thing that happened, as the device reported it. */}
        <footer
          className="flex h-8 shrink-0 items-center border-t border-border px-4"
          aria-live="polite"
        >
          <p
            data-testid="widget-status"
            className={cn("truncate text-xs", status?.tone ?? "text-muted-foreground")}
            title={status?.text}
          >
            {status?.text ?? " "}
          </p>
        </footer>
      </div>
    </div>
  );
}

export function WidgetApp() {
  return (
    <AppProvider>
      <Widget />
    </AppProvider>
  );
}
