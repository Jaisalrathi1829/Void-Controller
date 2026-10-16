import {
  AudioWaveform,
  Clapperboard,
  Ear,
  EarOff,
  SlidersHorizontal,
  Sparkles,
  Waves,
  Wind,
} from "lucide-react";
import { SegmentedControl } from "./primitives";
import { cn } from "./ui/utils";
import { isShown, isUnavailable, useApp, type NoiseMode } from "../store";
import type { NoiseModeOption } from "@/types/device";

/** The icon for a mode, by its name where it is one of the built-in ones. */
function iconFor(mode: NoiseModeOption) {
  const name = mode.name.trim().toLowerCase();
  // The QuietComfort 35 generation's levels go by these names too.
  const byName: Record<string, typeof Waves> = {
    quiet: Waves,
    aware: Ear,
    immersion: Sparkles,
    cinema: Clapperboard,
    high: Waves,
    low: AudioWaveform,
    wind: Wind,
    off: EarOff,
  };
  const Icon = byName[name] ?? (mode.preset ? Waves : SlidersHorizontal);
  return <Icon className="size-4" />;
}

/** What the active mode is, in a sentence. */
export function describeMode(mode: NoiseModeOption): string {
  const name = mode.name.trim().toLowerCase();
  if (name === "quiet") return "Maximum noise cancellation for focus and quiet environments.";
  if (name === "aware") return "Let ambient sound through so you stay aware of your surroundings.";
  // The levels of the QuietComfort 35 generation.
  if (name === "high") return "Full noise cancellation.";
  if (name === "low") return "Less noise cancellation: some of your surroundings come through.";
  if (name === "wind") return "Noise cancellation that keeps wind noise down.";
  if (name === "off") return "Noise cancellation is off.";
  if (mode.preset) return "One of the modes built into your headphones.";
  return mode.adjustable
    ? "A mode you set up. Its noise-cancellation level can be changed below."
    : "A mode you set up in the Bose app.";
}

/** Whether a command that switches mode is under way. */
export function isSwitchingMode(busy: string | null): boolean {
  return busy === "setNoiseControlMode" || busy === "setNoiseControl" || busy === "selectNoiseMode";
}

/**
 * The noise modes to choose from: the device's own list where it has given
 * one — every mode it has, under its own names, and nothing it has not — and
 * otherwise Quiet, Aware and the custom mode. None is shown as active until
 * the device has said which it is in.
 */
export function NoiseModePicker({
  icons = false,
  nameCustom = false,
  customHint,
  className,
}: {
  icons?: boolean;
  /** Without a list, call the custom mode by its own name once in it. */
  nameCustom?: boolean;
  /** Why the custom mode cannot be selected, where it cannot. */
  customHint?: string;
  className?: string;
}) {
  const { noiseModes, selectNoiseMode, noise, setNoise, noiseLabel, capabilities, busy } = useApp();
  const off = isUnavailable(capabilities.noiseControl);
  const switching = isSwitchingMode(busy) || busy === "setNoiseControlLevel";

  if (noiseModes && noiseModes.length > 0) {
    const active = noiseModes.find((m) => m.active);
    const many = noiseModes.length > 3;
    return (
      <SegmentedControl
        options={noiseModes.map((m) => ({
          value: String(m.index),
          label: m.name,
          icon: icons ? iconFor(m) : undefined,
        }))}
        value={active ? String(active.index) : null}
        onChange={(v) => {
          const mode = noiseModes.find((m) => String(m.index) === v);
          if (mode && !mode.active) selectNoiseMode(mode);
        }}
        disabled={off || switching}
        className={cn(
          "w-full [&>button]:flex-1",
          many && "flex-wrap [&>button]:basis-[30%]",
          className,
        )}
      />
    );
  }

  // No list: the three that can be asked for by name. A custom mode is left
  // out where the device has none that can be selected — unless it is in
  // one, which is still shown as what it is in.
  const customShown = isShown(capabilities.customAnc) || noise === "custom";
  const options: {
    value: NoiseMode;
    label: string;
    icon?: React.ReactNode;
    disabled?: boolean;
    hint?: string;
  }[] = [
    { value: "quiet", label: "Quiet", icon: icons ? <Waves className="size-4" /> : undefined },
    { value: "aware", label: "Aware", icon: icons ? <Ear className="size-4" /> : undefined },
  ];
  if (customShown) {
    options.push({
      value: "custom",
      // By the headphones' own name for it, once they have said they are in it.
      label: nameCustom && noise === "custom" && noiseLabel ? noiseLabel : "Custom",
      icon: icons ? <SlidersHorizontal className="size-4" /> : undefined,
      disabled: isUnavailable(capabilities.customAnc),
      hint: customHint ?? "These headphones have no custom mode this application can select.",
    });
  }
  return (
    <SegmentedControl
      options={options}
      value={noise}
      onChange={setNoise}
      disabled={off || switching}
      className={cn("w-full [&>button]:flex-1", className)}
    />
  );
}
