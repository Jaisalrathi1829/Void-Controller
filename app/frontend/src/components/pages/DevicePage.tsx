import { useState } from "react";
import {
  Activity,
  Bluetooth,
  Eye,
  EyeOff,
  Power,
  RefreshCw,
} from "lucide-react";
import {
  BatteryIndicator,
  Button,
  ConnectionBadge,
  Panel,
  SectionLabel,
} from "../primitives";
import { DeviceVisual } from "../DeviceVisual";
import { useApp } from "../../store";
import { cn } from "../ui/utils";

/**
 * The upstream export shipped a hardcoded three-entry timeline with specific
 * timestamps. Those were invented, and they rendered identically whether or
 * not a device had ever connected. The timeline below is built from real
 * connection events recorded during this session instead.
 */
const CONNECTION_EVENT_TYPES = new Set(["STATE", "SESSION"]);

function InfoRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between border-b border-border py-3 last:border-0">
      <span className="text-sm text-muted-foreground">{label}</span>
      <span className="text-sm">{children}</span>
    </div>
  );
}

export function DevicePage({ onNavigate }: { onNavigate: (p: any) => void }) {
  const {
    deviceName,
    connection,
    battery,
    charging,
    reconnect,
    disconnect,
    snapshot,
    events,
    connectedTo,
  } = useApp();
  const [showId, setShowId] = useState(false);

  const timeline = events.filter((e) => CONNECTION_EVENT_TYPES.has(e.type)).slice(0, 8);

  return (
    <div className="grid grid-cols-1 gap-5 lg:grid-cols-3">
      <div className="flex flex-col gap-5 lg:col-span-2">
        <Panel className="flex items-center gap-6 p-6">
          <DeviceVisual size={180} active={connection === "connected"} />
          <div className="flex-1">
            <div className="flex items-center gap-3">
              <h2>{deviceName}</h2>
              <ConnectionBadge state={connection} />
            </div>
            {/* Only describe hardware the device actually told us about.
                The upstream "Over-ear • Bluetooth 5.3 • Adaptive ANC" line
                was a fixed string that appeared even with nothing connected. */}
            <p className="mt-1 text-sm text-muted-foreground">
              {snapshot?.identity
                ? [snapshot.identity.manufacturer, snapshot.identity.modelNumber]
                    .filter(Boolean)
                    .join(" • ") || "No hardware details reported"
                : "No device connected"}
            </p>
            <div className="mt-4 flex flex-wrap gap-2">
              <Button icon={<RefreshCw className="size-4" />} onClick={reconnect}>Reconnect</Button>
              <Button variant="outline" icon={<Power className="size-4" />} onClick={disconnect}>Disconnect</Button>
              <Button variant="ghost" icon={<Activity className="size-4" />} onClick={() => onNavigate("diagnostics")}>
                Run Diagnostics
              </Button>
            </div>
          </div>
        </Panel>

        <Panel className="p-6">
          <SectionLabel>Device Information</SectionLabel>
          <div className="mt-3">
            <InfoRow label="Connection"><ConnectionBadge state={connection} /></InfoRow>
            {/* This machine's own name, shown only while the headphones are
                connected to it. Anything else they may be connected to at
                the same time is not something they have been asked. */}
            <InfoRow label="Connected to">
              {connectedTo ?? <span className="text-muted-foreground">—</span>}
            </InfoRow>
            <InfoRow label="Transport">
              {snapshot && snapshot.transport !== "none" ? (
                <span className="inline-flex items-center gap-1.5">
                  <Bluetooth className="size-4 text-primary" /> {snapshot.transport}
                </span>
              ) : (
                <span className="text-muted-foreground">None</span>
              )}
            </InfoRow>
            <InfoRow label="Battery"><BatteryIndicator level={battery} charging={charging} /></InfoRow>
            <InfoRow label="Device name">{deviceName}</InfoRow>
            <InfoRow label="Device ID">
              {snapshot?.identity ? (
                <span className="selectable inline-flex items-center gap-2 font-mono">
                  {showId ? snapshot.identity.id : "••••••••"}
                  <button
                    onClick={() => setShowId((s) => !s)}
                    aria-label={showId ? "Hide device ID" : "Show device ID"}
                    className="text-muted-foreground hover:text-foreground"
                  >
                    {showId ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
                  </button>
                </span>
              ) : (
                <span className="text-muted-foreground">—</span>
              )}
            </InfoRow>
            <InfoRow label="Firmware">
              <span className={snapshot?.identity?.firmwareVersion ? undefined : "text-muted-foreground"}>
                {snapshot?.identity?.firmwareVersion ?? "Not exposed to Windows"}
              </span>
            </InfoRow>
          </div>
        </Panel>
      </div>

      <Panel className="p-6">
        <SectionLabel>Connection Timeline</SectionLabel>
        {timeline.length === 0 ? (
          <p className="mt-4 text-sm text-muted-foreground">
            No connection events recorded yet this session. Events appear here as they happen —
            nothing is carried over from previous runs.
          </p>
        ) : (
          <ol className="mt-4 space-y-0">
            {timeline.map((e, i) => (
              <li key={e.id} className="relative flex gap-3 pb-6 last:pb-0">
                <div className="flex flex-col items-center">
                  <span
                    className={cn(
                      "mt-1 size-2.5 rounded-full",
                      e.status === "success"
                        ? "bg-success"
                        : e.status === "warning"
                          ? "bg-warning"
                          : "bg-muted-foreground",
                    )}
                  />
                  {i < timeline.length - 1 && <span className="w-px flex-1 bg-border" />}
                </div>
                <div className="-mt-0.5">
                  <div className="text-sm">{e.description}</div>
                  <div className="text-xs tabular-nums text-muted-foreground">{e.time}</div>
                </div>
              </li>
            ))}
          </ol>
        )}
      </Panel>
    </div>
  );
}
