import { motion } from "motion/react";
import { Volume2, Waves, Wind } from "lucide-react";
import { DeviceVisual } from "../DeviceVisual";
import {
  BatteryIndicator,
  Button,
  CapabilityBadge,
  ConnectionBadge,
  Panel,
  Range,
  SectionLabel,
  SegmentedControl,
} from "../primitives";
import { useApp, isUnavailable, type NoiseMode } from "../../store";

export function DashboardPage({ onNavigate }: { onNavigate: (p: any) => void }) {
  const {
    deviceName,
    connection,
    battery,
    charging,
    noise,
    setNoise,
    capabilities,
    volume,
    setVolume,
    hardwareEq,
    setHardwareEq,
    profiles,
    applyProfile,
    busy,
  } = useApp();

  const noiseDisabled = isUnavailable(capabilities.noiseControl);
  const noiseSwitching = busy === "setNoiseControlMode" || busy === "setNoiseControl";
  const noiseOptions: { value: NoiseMode; label: string; disabled?: boolean; hint?: string }[] = [
    { value: "quiet", label: "Quiet" },
    { value: "aware", label: "Aware" },
    {
      value: "custom",
      label: "Custom",
      // Shown when the headphones report it; not something this build can set.
      disabled: isUnavailable(capabilities.customAnc),
      hint: "Custom modes cannot be set from here yet.",
    },
  ];

  return (
    <div className="grid grid-cols-1 gap-5 xl:grid-cols-3">
      {/* Hero device panel */}
      <Panel className="relative overflow-hidden xl:col-span-2">
        <div className="pointer-events-none absolute inset-0 bg-gradient-to-br from-primary/[0.06] to-transparent" />
        <div className="relative flex flex-col items-center gap-2 px-8 pb-4 pt-8">
          <div className="flex items-center gap-3">
            <h2>{deviceName}</h2>
            <ConnectionBadge state={connection} />
          </div>
          <BatteryIndicator level={battery} charging={charging} size="lg" />
          <motion.div initial={{ scale: 0.92, opacity: 0 }} animate={{ scale: 1, opacity: 1 }} transition={{ duration: 0.5 }}>
            <DeviceVisual size={320} active={connection === "connected" || connection === "simulated"} />
          </motion.div>
        </div>

        {/* Noise + volume inline */}
        <div className="grid grid-cols-1 gap-px bg-border sm:grid-cols-2">
          <div className="bg-card p-6">
            <div className="mb-3 flex items-center justify-between">
              <div className="flex items-center gap-2">
                <Waves className="size-4 text-muted-foreground" />
                <SectionLabel>Noise Control</SectionLabel>
              </div>
              <CapabilityBadge cap={capabilities.noiseControl} />
            </div>
            <SegmentedControl
              options={noiseOptions}
              value={noise}
              onChange={setNoise}
              disabled={noiseDisabled || noiseSwitching}
              className="w-full [&>button]:flex-1"
            />
            {noiseDisabled && (
              <p className="mt-2 text-xs text-muted-foreground">Not yet verified on your headphones.</p>
            )}
          </div>
          <div className="bg-card p-6">
            <div className="mb-3 flex items-center gap-2">
              <Volume2 className="size-4 text-muted-foreground" />
              <SectionLabel>Volume</SectionLabel>
            </div>
            <Range value={volume} onChange={setVolume} suffix="%" label="Output" />
          </div>
        </div>
      </Panel>

      {/* Right column */}
      <div className="flex flex-col gap-5">
        <Panel className="p-6">
          <div className="mb-4 flex items-center justify-between">
            <SectionLabel>Equalizer</SectionLabel>
            <CapabilityBadge cap={capabilities.eq} />
          </div>
          <div className="space-y-4">
            <Range label="Bass" value={hardwareEq.bass} min={-10} max={10} leftLabel="−10" rightLabel="+10"
              displayValue={hardwareEq.bass > 0 ? `+${hardwareEq.bass}` : `${hardwareEq.bass}`}
              onChange={(v) => setHardwareEq({ ...hardwareEq, bass: v })} />
            <Range label="Mid" value={hardwareEq.mid} min={-10} max={10} leftLabel="−10" rightLabel="+10"
              displayValue={hardwareEq.mid > 0 ? `+${hardwareEq.mid}` : `${hardwareEq.mid}`}
              onChange={(v) => setHardwareEq({ ...hardwareEq, mid: v })} />
            <Range label="Treble" value={hardwareEq.treble} min={-10} max={10} leftLabel="−10" rightLabel="+10"
              displayValue={hardwareEq.treble > 0 ? `+${hardwareEq.treble}` : `${hardwareEq.treble}`}
              onChange={(v) => setHardwareEq({ ...hardwareEq, treble: v })} />
          </div>
          <Button variant="ghost" size="sm" className="mt-4 w-full" onClick={() => onNavigate("equalizer")}>
            Open Equalizer
          </Button>
        </Panel>

        <Panel className="p-6">
          <div className="mb-4 flex items-center justify-between">
            <SectionLabel>Quick Profiles</SectionLabel>
            <button className="text-xs text-primary hover:underline" onClick={() => onNavigate("profiles")}>
              Manage
            </button>
          </div>
          <div className="grid grid-cols-3 gap-2">
            {profiles.slice(0, 3).map((p, i) => (
              <motion.button
                key={p.id}
                onClick={() => applyProfile(p)}
                initial={{ opacity: 0, y: 10 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ delay: 0.1 + i * 0.06 }}
                whileHover={{ y: -4, scale: 1.04 }}
                whileTap={{ scale: 0.96 }}
                className="flex flex-col items-center gap-1.5 rounded-lg border border-border bg-surface-2 px-2 py-3 text-sm outline-none transition-colors hover:border-primary/40 hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring"
              >
                <Wind className="size-4 text-primary" />
                {p.name}
              </motion.button>
            ))}
          </div>
        </Panel>
      </div>
    </div>
  );
}
