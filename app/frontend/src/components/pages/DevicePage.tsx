import { useState } from "react";
import {
  ArrowLeftRight,
  Bluetooth,
  Check,
  Eye,
  EyeOff,
  Laptop,
  Pencil,
  RefreshCw,
  Smartphone,
  X,
} from "lucide-react";
import {
  BatteryIndicator,
  BatteryParts,
  Button,
  CapabilityBadge,
  ConnectionBadge,
  Panel,
  SectionLabel,
  Toggle,
} from "../primitives";
import { Modal } from "../Modal";
import { isShown, isUnavailable, useApp } from "../../store";
import { cn } from "../ui/utils";
import { hostPlatform } from "@/services/ipc";
import {
  checkDeviceName,
  MAX_DEVICE_NAME_BYTES,
  supportNote,
  type PairedDevice,
} from "@/types/device";

function InfoRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-3 border-b border-border py-3 last:border-0">
      <span className="shrink-0 text-sm text-muted-foreground">{label}</span>
      <span className="min-w-0 break-words text-right text-sm">{children}</span>
    </div>
  );
}

/** What the simulated device adds to its name so it cannot pass for real. */
const SIMULATED_SUFFIX = " (SIMULATED)";

/**
 * The headphones' name, and a way to change it.
 *
 * The name shown is the one the headphones report. A new one is sent to them
 * and shown only once they report it back; until then the old one stays.
 */
function DeviceNameRow() {
  const { deviceName, capabilities, renameDevice, busy, connection, snapshot } = useApp();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const saving = busy === "setDeviceName";
  const canRename = connection === "connected" && !isUnavailable(capabilities.rename);
  const why = snapshot?.capabilities.deviceRename.evidence;

  if (!editing) {
    return (
      <InfoRow label="Device name">
        <span className="inline-flex items-center gap-2">
          {deviceName}
          {!isShown(capabilities.rename) ? null : canRename ? (
            <button
              onClick={() => {
                // The label that marks a simulated device is not part of its name.
                setDraft(
                  deviceName.endsWith(SIMULATED_SUFFIX)
                    ? deviceName.slice(0, -SIMULATED_SUFFIX.length)
                    : deviceName,
                );
                setEditing(true);
              }}
              aria-label="Rename headphones"
              className="shrink-0 rounded-md p-1 text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
            >
              <Pencil className="size-3.5" />
            </button>
          ) : (
            connection === "connected" &&
            why && <span className="sr-only" data-testid="rename-unavailable">{why}</span>
          )}
        </span>
      </InfoRow>
    );
  }

  const checked = checkDeviceName(draft);
  const bytes = new TextEncoder().encode(draft.trim()).length;
  const save = async () => {
    if (!checked.ok || saving) return;
    if (await renameDevice(checked.name)) setEditing(false);
  };

  // Being changed, the name has a line to itself: beside its label there is
  // not the width for a box to type in on a phone.
  return (
    <div className="border-b border-border py-3 last:border-0">
      <span className="text-sm text-muted-foreground">Device name</span>
      <div className="mt-2 flex items-center gap-1.5">
        <input
          autoFocus
          value={draft}
          aria-label="New name for the headphones"
          aria-invalid={!checked.ok}
          disabled={saving}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void save();
            if (e.key === "Escape") setEditing(false);
          }}
          className="min-w-0 flex-1 rounded-md border border-border bg-background px-2.5 py-1.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
        />
        <button
          onClick={() => void save()}
          disabled={!checked.ok || saving}
          aria-label="Save name"
          className="shrink-0 rounded-md p-2 text-primary outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-40"
        >
          <Check className="size-4" />
        </button>
        <button
          onClick={() => setEditing(false)}
          disabled={saving}
          aria-label="Cancel renaming"
          className="shrink-0 rounded-md p-2 text-muted-foreground outline-none hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-40"
        >
          <X className="size-4" />
        </button>
      </div>
      <p
        className={cn("mt-1.5 text-xs", checked.ok ? "text-muted-foreground" : "text-warning")}
        data-testid="rename-hint"
      >
        {saving
          ? "Sending, then reading the name back from the headphones…"
          : checked.ok
            ? `${bytes} of ${MAX_DEVICE_NAME_BYTES} bytes. Other devices show the new name once they reconnect.`
            : checked.reason}
      </p>
    </div>
  );
}

/**
 * Multipoint — the headphones staying connected to two devices at once — and
 * the devices they are paired with, each with a switch that is on while the
 * headphones are connected to it.
 *
 * All of it is what the headphones report. A device's switch moves when they
 * report it connected or not, never when it is tapped. The two changes that
 * can take the headphones away from the device in the hand are asked about
 * first: switching multipoint off, and disconnecting this device itself.
 */
function MultiDevicePanel() {
  const {
    multipoint,
    setMultipoint,
    pairedDevices,
    setDeviceConnected,
    capabilities,
    busy,
    connection,
    snapshot,
  } = useApp();
  const [confirmOff, setConfirmOff] = useState(false);
  /** A change to one device that is being asked about before it is made. */
  const [asking, setAsking] = useState<{ device: PairedDevice; on: boolean } | null>(null);
  /** The device a change is under way for. */
  const [pending, setPending] = useState<{ id: string; on: boolean } | null>(null);
  const onPhone = hostPlatform() === "android";
  const here = onPhone ? "phone" : "computer";
  const switching = busy === "setMultipoint";
  const changing = busy === "setPairedDeviceConnection";
  const unavailable = connection !== "connected" || isUnavailable(capabilities.multipoint);
  const cannotConnect = connection !== "connected" || isUnavailable(capabilities.connections);
  const why = snapshot?.capabilities.multipoint.evidence;
  const devices = pairedDevices ?? [];
  const switchShown = isShown(capabilities.multipoint);
  const devicesShown = isShown(capabilities.connections);

  const change = async (device: PairedDevice, on: boolean) => {
    setPending({ id: device.id, on });
    try {
      await setDeviceConnected(device, on);
    } finally {
      setPending(null);
    }
  };
  const flip = (device: PairedDevice, on: boolean) => {
    // Two changes are asked about first. Dropping this device ends this
    // application's own connection to the headphones. And with multipoint
    // off the headphones hold one connection, so connecting another device
    // is dropping this one.
    const dropsThis =
      (!on && device.thisDevice) || (on && multipoint === false && !device.thisDevice);
    if (dropsThis) setAsking({ device, on });
    else void change(device, on);
  };

  return (
    <Panel className="p-6" data-testid="multi-device">
      {switchShown && (
      <>
      <div className="flex items-center justify-between gap-3">
        <h3 className="text-base font-semibold">Multi-device connection</h3>
        <div className="flex shrink-0 items-center gap-3">
          <CapabilityBadge cap={capabilities.multipoint} />
          <Toggle
            label="Multi-device connection"
            checked={multipoint === true}
            disabled={unavailable || multipoint === null || switching}
            onChange={(on) => (on ? setMultipoint(true) : setConfirmOff(true))}
          />
        </div>
      </div>
      <p className="mt-2 text-sm text-muted-foreground">
        {multipoint === null
          ? connection === "connected"
            ? (why ?? "The headphones have not said whether this is on.")
            : "Connect the headphones to see this."
          : multipoint
            ? "On: the headphones stay connected to two devices at once and play from whichever is playing."
            : "Off: the headphones stay connected to one device at a time."}
        {switching && " Switching, then checking what the headphones report…"}
      </p>
      </>
      )}

      {devicesShown && (
      <div className={cn(switchShown && "mt-5 border-t border-border pt-4")}>
        <div className="text-sm font-medium text-muted-foreground">Devices</div>
        {pairedDevices === null ? (
          <p className="mt-3 text-sm text-muted-foreground" data-testid="paired-unknown">
            {connection === "connected"
              ? "The headphones have not listed the devices they are paired with."
              : "—"}
          </p>
        ) : devices.length === 0 ? (
          <p className="mt-3 text-sm text-muted-foreground">
            The headphones list no paired devices.
          </p>
        ) : (
          <ul className="mt-2 divide-y divide-border" aria-label="Devices the headphones are paired with">
            {devices.map((device) => {
              const name = device.name || "Unnamed device";
              const busyHere = changing && pending?.id === device.id;
              return (
                <li key={device.id} className="flex items-center gap-3.5 py-3.5">
                  {/* What kind of thing this one is, is known. What kind the
                      others are is not, and is not guessed at. */}
                  <span
                    className={cn(
                      "flex size-11 shrink-0 items-center justify-center rounded-xl",
                      device.connected ? "bg-accent text-primary" : "bg-muted text-muted-foreground",
                    )}
                  >
                    {!device.thisDevice ? (
                      <Bluetooth className="size-5" />
                    ) : onPhone ? (
                      <Smartphone className="size-5" />
                    ) : (
                      <Laptop className="size-5" />
                    )}
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-base font-medium">{name}</span>
                    <span className="block text-sm text-muted-foreground" data-testid="device-state">
                      {busyHere
                        ? pending?.on
                          ? "Connecting, then checking with the headphones…"
                          : "Disconnecting, then checking with the headphones…"
                        : [
                            device.thisDevice ? `This ${here}` : null,
                            device.connected ? "Connected" : "Not connected",
                          ]
                            .filter(Boolean)
                            .join(" · ")}
                    </span>
                  </span>
                  <Toggle
                    label={`${name} connected`}
                    checked={device.connected}
                    disabled={cannotConnect || changing || switching}
                    onChange={(on) => flip(device, on)}
                  />
                </li>
              );
            })}
          </ul>
        )}
      </div>
      )}

      <Modal
        open={confirmOff}
        onClose={() => setConfirmOff(false)}
        title="Turn off multi-device connection?"
        description={`The headphones will keep one connection and drop the other, and they choose which. If they drop this ${here}, they disconnect from it straight away, and this switch can only be turned back on after you reconnect them to this ${here} from Bluetooth settings.`}
        footer={
          <>
            <Button variant="outline" onClick={() => setConfirmOff(false)}>
              Keep it on
            </Button>
            <Button
              variant="danger"
              onClick={() => {
                setConfirmOff(false);
                setMultipoint(false);
              }}
            >
              Turn off
            </Button>
          </>
        }
      />

      <Modal
        open={asking !== null}
        onClose={() => setAsking(null)}
        title={
          asking?.on
            ? `Connect ${asking.device.name || "this device"}?`
            : `Disconnect from this ${here}?`
        }
        description={
          asking?.on
            ? `Multi-device connection is off, so the headphones hold one connection. Connecting them to ${asking.device.name || "that device"} disconnects them from this ${here}.`
            : `The headphones will stop playing from this ${here}, and this app will lose them. To get them back, connect them again from this ${here}'s Bluetooth settings.`
        }
        footer={
          <>
            <Button variant="outline" onClick={() => setAsking(null)}>
              Cancel
            </Button>
            <Button
              variant="danger"
              onClick={() => {
                const wanted = asking;
                setAsking(null);
                if (wanted) void change(wanted.device, wanted.on);
              }}
            >
              {asking?.on ? "Connect" : "Disconnect"}
            </Button>
          </>
        }
      />
    </Panel>
  );
}

/**
 * The upstream export shipped a hardcoded three-entry timeline with specific
 * timestamps. Those were invented, and they rendered identically whether or
 * not a device had ever connected. The timeline below is built from real
 * connection events recorded during this session instead.
 */
const CONNECTION_EVENT_TYPES = new Set(["STATE", "SESSION"]);

export function DevicePage() {
  const {
    connection,
    battery,
    charging,
    connect,
    reconnect,
    snapshot,
    events,
    connectedTo,
    busy,
    model,
    batteryParts,
    capabilities,
    changeDevice,
  } = useApp();
  const [showId, setShowId] = useState(false);
  const multiDeviceShown = isShown(capabilities.multipoint) || isShown(capabilities.connections);
  const onPhone = hostPlatform() === "android";

  const timeline = events.filter((e) => CONNECTION_EVENT_TYPES.has(e.type)).slice(0, 8);

  return (
    <div className="grid grid-cols-1 gap-5 lg:grid-cols-3">
      <div className="flex flex-col gap-5 lg:col-span-2">
        {/* The page used to open with a card of the headphones themselves:
            their picture, their name, and buttons to reconnect and
            disconnect. It was taken off as saying again what the rows below
            say. What it had that they did not — a way to reconnect — is on
            the Connection row. */}
        <Panel className="p-6">
          <div className="flex items-center justify-between gap-3">
            <SectionLabel>Device Information</SectionLabel>
            <Button
              size="sm"
              variant="ghost"
              icon={<ArrowLeftRight className="size-3.5" />}
              onClick={changeDevice}
            >
              Change device
            </Button>
          </div>
          <div className="mt-3">
            <InfoRow label="Connection">
              <span className="inline-flex flex-wrap items-center justify-end gap-2">
                <ConnectionBadge state={connection} />
                {connection !== "connected" && (
                  <Button
                    size="sm"
                    variant="outline"
                    icon={<RefreshCw className="size-3.5" />}
                    disabled={busy !== null}
                    onClick={connection === "disconnected" ? connect : reconnect}
                  >
                    Reconnect
                  </Button>
                )}
              </span>
            </InfoRow>
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
            <InfoRow label="Battery">
              <span className="inline-flex flex-col items-end gap-1">
                <BatteryIndicator level={battery} charging={charging} />
                <BatteryParts parts={batteryParts} className="justify-end text-xs" />
              </span>
            </InfoRow>
            <DeviceNameRow />
            {/* Which model it is, from Bose's catalogue, and how far Void
                has been tried with that model. */}
            <InfoRow label="Model">
              {model ? (
                <span className="inline-flex flex-col items-end gap-0.5">
                  <span>{model.name}</span>
                  <span
                    className="max-w-xs text-xs text-muted-foreground"
                    data-testid="model-support"
                  >
                    {supportNote(model.support)}
                  </span>
                </span>
              ) : (
                <span className="text-muted-foreground">Not identified</span>
              )}
            </InfoRow>
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
                {snapshot?.identity?.firmwareVersion ??
                  (onPhone ? "Not reported yet" : "Not exposed to Windows")}
              </span>
            </InfoRow>
          </div>
        </Panel>

        {multiDeviceShown && <MultiDevicePanel />}
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
