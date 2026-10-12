import { useState } from "react";
import { Monitor, Moon, Sun, Trash2, Download } from "lucide-react";
import {
  Button,
  Panel,
  SectionLabel,
  Toggle,
} from "../primitives";
import { Modal } from "../Modal";
import { VoiceFeedbackPanel } from "./VoiceFeedbackPanel";
import { useApp, type ThemePref } from "../../store";
import { hostPlatform, type SettingKey } from "@/services/ipc";
import { cn } from "../ui/utils";

function SettingRow({
  title,
  desc,
  unavailable,
  children,
}: {
  title: string;
  desc?: string;
  /** Explains why the control is inert, when the backing feature is missing. */
  unavailable?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-4 border-b border-border py-3.5 last:border-0">
      <div className={unavailable ? "opacity-60" : undefined}>
        <div className="text-sm">{title}</div>
        {desc && <div className="text-xs text-muted-foreground">{desc}</div>}
        {unavailable && (
          <div className="mt-0.5 text-xs text-warning">{unavailable}</div>
        )}
      </div>
      {children}
    </div>
  );
}

const themes: { value: ThemePref; label: string; icon: React.ElementType }[] = [
  { value: "system", label: "System", icon: Monitor },
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
];

const logLevels = ["Off", "Error", "Warn", "Info", "Debug", "Trace"];

export function SettingsPage() {
  const { settings, toggleSetting, theme, setTheme, logLevel, setLogLevel, deviceName } = useApp();
  const [clearOpen, setClearOpen] = useState(false);
  // Starting with the system and starting minimized are desktop notions; on
  // a phone they are left out rather than listed as unavailable. "Minimize
  // to tray" is the same switch there, under the name of what it does.
  const onPhone = hostPlatform() === "android";

  /**
   * One switch. A switch that cannot be used in this build is shown disabled
   * with the native layer's reason, rather than hidden or left to do nothing.
   */
  const row = (key: SettingKey, title: string, desc?: string) => (
    <SettingRow title={title} desc={desc} unavailable={settings.unavailable[key]}>
      <Toggle
        label={title}
        disabled={!!settings.unavailable[key]}
        checked={settings[key]}
        onChange={() => toggleSetting(key)}
      />
    </SettingRow>
  );

  return (
    <div className="mx-auto max-w-2xl space-y-5">
      <Panel className="p-6">
        <SectionLabel>General</SectionLabel>
        <div className="mt-2">
          {!onPhone &&
            row("startWithWindows", "Start with Windows", "Open the app when you sign in to Windows")}
          {!onPhone &&
            row(
              "startMinimized",
              "Start minimized",
              settings.minimizeToTray
                ? "Open in the tray instead of on screen"
                : "Open minimized to the taskbar. With Minimize to tray on, it opens in the tray instead",
            )}
          {onPhone
            ? row(
                "minimizeToTray",
                "Keep running in the background",
                "Announcements and notifications carry on while the app is not on screen. Android shows a notification for as long as this is on",
              )
            : row(
                "minimizeToTray",
                "Minimize to tray",
                "Closing or minimizing the window keeps the app running in the system tray",
              )}
          {row("autoConnect", "Auto-connect", "Connect to preferred device on launch")}
        </div>
      </Panel>

      <Panel className="p-6">
        <SectionLabel>Appearance</SectionLabel>
        <div className="mt-3 grid grid-cols-3 gap-2">
          {themes.map((t) => {
            const Icon = t.icon;
            const active = theme === t.value;
            return (
              <button
                key={t.value}
                onClick={() => setTheme(t.value)}
                className={cn(
                  "flex flex-col items-center gap-2 rounded-lg border p-4 text-sm outline-none transition-all focus-visible:ring-2 focus-visible:ring-ring",
                  active ? "border-primary bg-accent text-accent-foreground" : "border-border text-muted-foreground hover:bg-accent",
                )}
              >
                <Icon className="size-5" />
                {t.label}
              </button>
            );
          })}
        </div>
      </Panel>

      <Panel className="p-6">
        <SectionLabel>Device</SectionLabel>
        <div className="mt-2">
          <SettingRow title="Preferred Device" desc="Reconnect to this device automatically">
            <span className="text-sm text-muted-foreground" data-testid="preferred-device">
              {settings.preferredDeviceName ?? deviceName}
            </span>
          </SettingRow>
          {row("autoReconnect", "Auto reconnect", "Reconnect automatically when in range")}
        </div>
      </Panel>

      <Panel className="p-6">
        <SectionLabel>Notifications</SectionLabel>
        <div className="mt-2">
          {row("notifyConnected", "Device connected")}
          {row("notifyDisconnected", "Device disconnected")}
          {row("notifyLowBattery", "Low battery", "At 20% or below")}
        </div>
      </Panel>

      <VoiceFeedbackPanel />

      <Panel className="p-6">
        <SectionLabel>Diagnostics</SectionLabel>
        <div className="mt-3">
          <label className="mb-2 block">Logging level</label>
          <div className="flex flex-wrap gap-1.5">
            {logLevels.map((l) => (
              <button
                key={l}
                onClick={() => setLogLevel(l)}
                className={cn(
                  "rounded-md border px-3 py-1 text-sm outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring",
                  logLevel === l ? "border-primary/50 bg-accent text-accent-foreground" : "border-border text-muted-foreground hover:bg-accent",
                )}
              >
                {l}
              </button>
            ))}
          </div>
          <div className="mt-4 flex flex-wrap gap-2">
            <Button variant="outline" size="sm" icon={<Download className="size-4" />}>Export Logs</Button>
            <Button variant="ghost" size="sm" icon={<Trash2 className="size-4" />} onClick={() => setClearOpen(true)}>Clear Logs</Button>
          </div>
        </div>
      </Panel>

      <Modal
        open={clearOpen}
        onClose={() => setClearOpen(false)}
        title="Clear Logs"
        description="This will permanently remove all locally stored diagnostic logs."
        footer={
          <>
            <Button variant="outline" onClick={() => setClearOpen(false)}>Cancel</Button>
            <Button variant="danger" onClick={() => setClearOpen(false)}>Clear Logs</Button>
          </>
        }
      />
    </div>
  );
}
