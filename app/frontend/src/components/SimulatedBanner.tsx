import { FlaskConical } from "lucide-react";
import { useApp } from "../store";

/**
 * Unmistakable banner shown whenever the active backend is the mock.
 *
 * Deliberately not dismissible. The upstream version had a close button that
 * called `setConnection("disconnected")` — which did not leave simulation, it
 * just made the simulated device look disconnected while every value on
 * screen stayed fabricated. Leaving simulation means switching to the real
 * backend, so that is what the action does now.
 */
export function SimulatedBanner() {
  const { setSimulated, nativeAvailable, busy } = useApp();

  return (
    <div className="flex items-center justify-between gap-3 border-b border-warning/30 bg-warning-subtle px-4 py-2 text-sm md:px-8">
      <div className="flex items-center gap-2 text-warning">
        <FlaskConical className="size-4 shrink-0" />
        <span className="font-medium">Simulated Device</span>
        <span className="hidden text-muted-foreground sm:inline">
          Every value shown is fabricated. Nothing here reflects real hardware.
        </span>
        <span className="text-muted-foreground sm:hidden">Fabricated values.</span>
      </div>

      {nativeAvailable ? (
        <button
          onClick={() => void setSimulated(false)}
          disabled={busy === "switch-source"}
          className="shrink-0 rounded-md border border-warning/40 px-2 py-1 text-xs text-warning outline-none transition-colors hover:bg-warning/10 disabled:opacity-50"
        >
          Use real hardware
        </button>
      ) : (
        <span className="shrink-0 text-xs text-muted-foreground">
          Browser preview — no hardware access
        </span>
      )}
    </div>
  );
}
