import {
  AlertTriangle,
  Bluetooth,
  BluetoothOff,
  Loader2,
  Radar,
  SearchX,
  ShieldQuestion,
} from "lucide-react";
import { motion } from "motion/react";
import { DeviceVisual } from "./DeviceVisual";
import { Button } from "./primitives";
import { useApp, type ConnectionState } from "../store";
import {
  hostPlatform,
  openBluetoothSettings as openBtSettings,
  toIpcError,
} from "@/services/ipc";
import { toast } from "sonner";

async function openBluetoothSettings() {
  try {
    await openBtSettings();
  } catch (e) {
    toast.error("Could not open Bluetooth settings.", {
      description: toIpcError(e).message,
    });
  }
}

function Center({ children }: { children: React.ReactNode }) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      className="flex h-full flex-col items-center justify-center px-2 text-center md:px-8"
    >
      {children}
    </motion.div>
  );
}

export function ConnectionScreen({ state }: { state: ConnectionState }) {
  // The upstream export drove these buttons with `setConnection("connecting")`
  // and `setConnection("simulated")`, which only moved a local state variable.
  // They now issue real commands, so the screen reflects what the backend
  // actually did rather than what we hoped it would do.
  const {
    reconnect,
    connect,
    setSimulated,
    simulated,
    nativeAvailable,
    deviceName,
    busy,
    deviceError,
    allowBluetooth,
  } = useApp();
  const onPhone = hostPlatform() === "android";

  // Nothing is wrong with the headphones or the radio: the application has
  // not been allowed to look. Said before anything else, because until it is
  // settled every other screen here would be misleading.
  if (deviceError?.kind === "not-permitted" && !simulated) {
    return (
      <Center>
        <div className="flex size-24 items-center justify-center rounded-2xl bg-warning-subtle text-warning">
          <ShieldQuestion className="size-10" />
        </div>
        <h2 className="mt-6">Bluetooth access is needed.</h2>
        <p className="mt-2 max-w-sm text-sm text-muted-foreground" data-testid="permission-reason">
          {deviceError.message}
        </p>
        <Button className="mt-5" icon={<Bluetooth className="size-4" />} onClick={allowBluetooth}>
          Allow Bluetooth access
        </Button>
      </Center>
    );
  }

  if (state === "connecting" || state === "reconnecting") {
    return (
      <Center>
        <DeviceVisual size={300} active />
        <div className="mt-6 flex items-center gap-2 text-foreground">
          <Loader2 className="size-4 animate-spin text-primary" />
          <span className="font-medium">
            {state === "connecting" ? "Connecting to" : "Reconnecting to"} {deviceName}…
          </span>
        </div>
        <p className="mt-2 max-w-sm text-sm text-muted-foreground">
          Establishing a link with your headphones. This usually takes a few seconds.
        </p>
      </Center>
    );
  }

  if (state === "discovering") {
    return (
      <Center>
        <div className="relative">
          <div className="flex size-28 items-center justify-center rounded-full bg-primary/10 text-primary">
            <Radar className="size-10" />
          </div>
        </div>
        <h2 className="mt-6">Discovering devices…</h2>
        <p className="mt-2 max-w-sm text-sm text-muted-foreground">
          Scanning for nearby Bose headphones. Make sure your device is powered on and in range.
        </p>
      </Center>
    );
  }

  if (state === "bluetooth-disabled") {
    return (
      <Center>
        <div className="flex size-24 items-center justify-center rounded-2xl bg-warning-subtle text-warning">
          <BluetoothOff className="size-10" />
        </div>
        <h2 className="mt-6">
          {onPhone ? "Bluetooth is turned off." : "Bluetooth is turned off in Windows."}
        </h2>
        <p className="mt-2 max-w-sm text-sm text-muted-foreground">
          Turn Bluetooth on to detect and connect to your headphones.
        </p>
        <Button
          className="mt-5"
          icon={<Bluetooth className="size-4" />}
          onClick={() => void openBluetoothSettings()}
        >
          Open Bluetooth Settings
        </Button>
      </Center>
    );
  }

  if (state === "device-unavailable") {
    return (
      <Center>
        <div className="flex size-24 items-center justify-center rounded-2xl bg-warning-subtle text-warning">
          <SearchX className="size-10" />
        </div>
        <h2 className="mt-6">Your headphones are out of range or powered off.</h2>
        <p className="mt-2 max-w-sm text-sm text-muted-foreground">
          Move closer to your {onPhone ? "phone" : "PC"} or power on the headphones, then try
          again.
        </p>
        <Button className="mt-5" onClick={reconnect}>
          Retry Connection
        </Button>
      </Center>
    );
  }

  if (state === "error") {
    return (
      <Center>
        <div className="flex size-24 items-center justify-center rounded-2xl bg-error-subtle text-error">
          <AlertTriangle className="size-10" />
        </div>
        <h2 className="mt-6">Unable to communicate with the headphones.</h2>
        <p className="mt-2 max-w-sm text-sm text-muted-foreground">
          Something went wrong during the last operation. You can retry the connection.
        </p>
        <Button className="mt-5" onClick={reconnect}>
          Reconnect
        </Button>
      </Center>
    );
  }

  // disconnected
  return (
    <Center>
      <DeviceVisual size={280} active={false} />
      <h2 className="mt-4">No Bose headphones connected.</h2>
      <p className="mt-2 max-w-sm text-sm text-muted-foreground">
        Connect your headphones to control noise, EQ, and profiles from here.
      </p>
      <div className="mt-5 flex flex-wrap justify-center gap-2">
        <Button
          icon={<Bluetooth className="size-4" />}
          onClick={connect}
          disabled={busy !== null}
        >
          Connect
        </Button>
        {/* Only offered when we are not already on the mock backend, and only
            where a real backend exists to switch away from. */}
        {!simulated && nativeAvailable && (
          <Button
            variant="outline"
            onClick={() => void setSimulated(true)}
            disabled={busy !== null}
          >
            Use Simulated Device
          </Button>
        )}
      </div>
      {simulated && (
        <p className="mt-4 max-w-sm text-xs text-muted-foreground">
          The simulated device is active. Connecting will produce fabricated readings, not
          hardware data.
        </p>
      )}
    </Center>
  );
}
