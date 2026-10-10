import { Cpu, MonitorSpeaker } from "lucide-react";
import {
  Button,
  CapabilityBadge,
  Panel,
  Range,
  SectionLabel,
} from "../primitives";
import { useApp, type EqValues } from "../../store";
import { cn } from "../ui/utils";

const presets: { name: string; eq: EqValues }[] = [
  { name: "Flat", eq: { bass: 0, mid: 0, treble: 0 } },
  { name: "Music", eq: { bass: 4, mid: 0, treble: 2 } },
  { name: "Bass Boost", eq: { bass: 8, mid: -1, treble: 0 } },
  { name: "Podcast", eq: { bass: -3, mid: 5, treble: 1 } },
  { name: "Gaming", eq: { bass: 6, mid: -2, treble: 4 } },
];

function EqControls({ value, onChange, disabled }: { value: EqValues; onChange: (e: EqValues) => void; disabled?: boolean }) {
  const fmt = (v: number) => (v > 0 ? `+${v}` : `${v}`);
  return (
    <div className="space-y-5">
      {(["bass", "mid", "treble"] as const).map((band) => (
        <Range
          key={band}
          label={band.charAt(0).toUpperCase() + band.slice(1)}
          value={value[band]}
          min={-10}
          max={10}
          leftLabel="−10"
          rightLabel="+10"
          displayValue={fmt(value[band])}
          disabled={disabled}
          onChange={(v) => onChange({ ...value, [band]: v })}
        />
      ))}
    </div>
  );
}

function matchesPreset(eq: EqValues, p: EqValues) {
  return eq.bass === p.bass && eq.mid === p.mid && eq.treble === p.treble;
}

export function EqualizerPage() {
  const { hardwareEq, setHardwareEq, softwareEq, setSoftwareEq, capabilities, snapshot, busy } = useApp();
  const hwDisabled = capabilities.eq === "unknown" || capabilities.eq === "unsupported";
  const hwApplying = busy === "setEqualizer";
  // Why the capability is in its current state, in the backend's own words.
  const hwEvidence = snapshot?.capabilities.equalizer.evidence;

  return (
    <div className="grid grid-cols-1 gap-5 lg:grid-cols-2">
      {/* Hardware EQ */}
      <Panel className="p-6">
        <div className="mb-1 flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Cpu className="size-4 text-primary" />
            <SectionLabel>Hardware EQ</SectionLabel>
          </div>
          <CapabilityBadge cap={capabilities.eq} />
        </div>
        <p className="mb-5 text-sm text-muted-foreground">
          Adjusts equalization on the headphone hardware itself.
        </p>
        <EqControls value={hardwareEq} onChange={setHardwareEq} disabled={hwDisabled || hwApplying} />
        {hwDisabled && (
          <p className="mt-4 text-xs text-muted-foreground" data-testid="eq-unavailable">
            {hwEvidence ?? "Hardware EQ is not fully verified on this device. Changes may not apply."}
          </p>
        )}
        {capabilities.eq === "experimental" && hwEvidence && (
          <p className="mt-4 text-xs text-muted-foreground" data-testid="eq-experimental">
            <span className="font-medium text-foreground">Experimental. </span>
            {hwEvidence}
          </p>
        )}
        <div className="mt-5 flex flex-wrap gap-2 border-t border-border pt-4">
          {presets.map((p) => (
            <button
              key={p.name}
              disabled={hwDisabled || hwApplying}
              onClick={() => setHardwareEq(p.eq)}
              className={cn(
                "rounded-md border px-2.5 py-1 text-xs outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-40",
                matchesPreset(hardwareEq, p.eq)
                  ? "border-primary/50 bg-accent text-accent-foreground"
                  : "border-border text-muted-foreground hover:bg-accent hover:text-foreground",
              )}
            >
              {p.name}
            </button>
          ))}
        </div>
      </Panel>

      {/* Software EQ */}
      <Panel className="p-6">
        <div className="mb-1 flex items-center justify-between">
          <div className="flex items-center gap-2">
            <MonitorSpeaker className="size-4 text-chart-4" />
            <SectionLabel>Software EQ</SectionLabel>
          </div>
          <span className="rounded-md bg-info-subtle px-2 py-0.5 text-xs font-medium text-info">Windows Audio</span>
        </div>
        <p className="mb-5 text-sm text-muted-foreground">
          Applies processing through the Windows audio pipeline — independent of the device.
        </p>
        <EqControls value={softwareEq} onChange={setSoftwareEq} />
        <div className="mt-5 flex flex-wrap gap-2 border-t border-border pt-4">
          {presets.map((p) => (
            <button
              key={p.name}
              onClick={() => setSoftwareEq(p.eq)}
              className={cn(
                "rounded-md border px-2.5 py-1 text-xs outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring",
                matchesPreset(softwareEq, p.eq)
                  ? "border-primary/50 bg-accent text-accent-foreground"
                  : "border-border text-muted-foreground hover:bg-accent hover:text-foreground",
              )}
            >
              {p.name}
            </button>
          ))}
          <Button variant="ghost" size="sm" className="ml-auto" onClick={() => setSoftwareEq({ bass: 0, mid: 0, treble: 0 })}>
            Reset
          </Button>
        </div>
      </Panel>
    </div>
  );
}
