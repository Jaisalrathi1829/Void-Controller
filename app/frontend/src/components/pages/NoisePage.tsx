import { Info, TriangleAlert, Waves, Wind } from "lucide-react";
import {
  CapabilityBadge,
  Panel,
  Range,
  SectionLabel,
  SegmentedControl,
  Toggle,
} from "../primitives";
import { useApp, type NoiseMode, capabilityMeta, isUnavailable } from "../../store";

const descriptions: Record<NoiseMode, string> = {
  quiet: "Maximum noise cancellation for focus and quiet environments.",
  aware: "Let ambient sound through so you stay aware of your surroundings.",
  custom: "Dial in the exact level of noise cancellation you prefer.",
};

/** Local wall-clock time for a device-read timestamp, or null if unparseable. */
function clockTime(iso: string | null): string | null {
  if (!iso) return null;
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? null : date.toLocaleTimeString([], { hour12: false });
}

export function NoisePage() {
  const {
    noise,
    setNoise,
    noiseReadAt,
    noiseLabel,
    deviceNotice,
    ancLevel,
    setAncLevel,
    windBlock,
    setWindBlock,
    capabilities,
    snapshot,
    simulated,
    busy,
  } = useApp();
  const cap = capabilities.noiseControl;
  const customCap = capabilities.customAnc;
  const disabled = isUnavailable(cap);
  const customUnavailable = isUnavailable(customCap);
  const switching = busy === "setNoiseControlMode" || busy === "setNoiseControl";
  const adjusting = busy === "setNoiseControlLevel";

  // Why each capability is in its current state, in the backend's own words.
  const evidence = snapshot?.capabilities.noiseControl.evidence;
  const customEvidence = snapshot?.capabilities.customNoiseControl.evidence;

  const noiseOptions: {
    value: NoiseMode;
    label: string;
    icon: React.ReactNode;
    disabled?: boolean;
    hint?: string;
  }[] = [
    { value: "quiet", label: "Quiet", icon: <Waves className="size-4" /> },
    { value: "aware", label: "Aware", icon: <Info className="size-4" /> },
    {
      value: "custom",
      label: "Custom",
      icon: <Wind className="size-4" />,
      // The headphones can be *in* a custom mode even when one cannot be
      // requested from here, and that is still shown as active.
      disabled: customUnavailable,
      hint: customEvidence ?? "Custom modes cannot be set from here.",
    },
  ];

  const readAt = clockTime(noiseReadAt);
  const inCustom = noise === "custom" && !disabled;

  return (
    <div className="mx-auto max-w-3xl space-y-5">
      <Panel className="p-6">
        <div className="mb-4 flex items-center justify-between">
          <SectionLabel>Noise Mode</SectionLabel>
          <CapabilityBadge cap={cap} />
        </div>

        <SegmentedControl
          options={noiseOptions}
          value={noise}
          onChange={setNoise}
          disabled={disabled || switching || adjusting}
          className="w-full [&>button]:flex-1"
        />

        <p className="mt-3 text-sm text-muted-foreground">
          {disabled
            ? capabilityMeta[cap].note
            : noise
              ? descriptions[noise]
              : "The device has not reported its current mode, so none is shown as active."}
        </p>

        {!disabled && noise && readAt && !simulated && (
          <p className="mt-1 text-xs text-muted-foreground" data-testid="noise-read-at">
            Reported by the headphones at {readAt}.
          </p>
        )}

        {deviceNotice && (
          <div
            role="alert"
            className="mt-4 flex items-start gap-2 rounded-lg border border-border bg-warning-subtle/40 p-3 text-sm"
          >
            <TriangleAlert className="mt-0.5 size-4 shrink-0 text-warning" />
            <span className="text-muted-foreground">
              {deviceNotice}
              {noise && " The mode shown is the last one the headphones confirmed."}
            </span>
          </div>
        )}

        {disabled && (
          <div className="mt-4 flex items-start gap-2 rounded-lg border border-border bg-warning-subtle/40 p-3 text-sm">
            <Info className="mt-0.5 size-4 shrink-0 text-warning" />
            <span className="text-muted-foreground">
              {evidence ??
                "This control is not yet verified on your headphones, so changes may not take effect on the device."}
            </span>
          </div>
        )}

        {!disabled && customUnavailable && customEvidence && (
          <p className="mt-3 text-xs text-muted-foreground" data-testid="custom-unavailable">
            Custom: {customEvidence}
          </p>
        )}
      </Panel>

      {inCustom && (
        <Panel className="space-y-6 p-6" data-testid="custom-panel">
          <div>
            <div className="flex items-center justify-between">
              <SectionLabel>Custom Noise Cancellation</SectionLabel>
              <CapabilityBadge cap={customCap} />
            </div>

            {noiseLabel && (
              <p className="mt-2 text-sm text-muted-foreground">
                Adjusting <span className="font-medium text-foreground">{noiseLabel}</span>
                {simulated ? "." : ". The level is saved to that mode on your headphones."}
              </p>
            )}

            <div className="mt-4">
              {ancLevel === null ? (
                <p className="text-sm text-muted-foreground" data-testid="level-unknown">
                  The headphones have not reported this mode&apos;s level, so there is nothing to
                  show or adjust yet.
                </p>
              ) : (
                <>
                  <Range
                    label="Noise Cancellation"
                    value={ancLevel}
                    min={0}
                    max={10}
                    leftLabel="0"
                    rightLabel="10"
                    displayValue={`${ancLevel}`}
                    onChange={setAncLevel}
                    disabled={customUnavailable || adjusting || switching}
                  />
                  <p className="mt-2 text-xs text-muted-foreground">
                    0 lets the most sound through; 10 cancels the most.
                    {adjusting && " Applying, then reading the headphones back…"}
                  </p>
                </>
              )}
            </div>

          </div>

          {/* Wind block has no device interface in this build. It is offered
              only on the simulated device, where nothing claims to be real. */}
          {simulated && (
            <div className="flex items-center justify-between border-t border-border pt-4">
              <div>
                <div className="text-sm">Wind Block</div>
                <div className="text-xs text-muted-foreground">Reduces wind noise in outdoor conditions.</div>
              </div>
              <Toggle checked={windBlock} onChange={setWindBlock} label="Wind Block" />
            </div>
          )}
        </Panel>
      )}
    </div>
  );
}
