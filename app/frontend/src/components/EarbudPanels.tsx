import { MapPin } from "lucide-react";
import { Button, CapabilityBadge, Panel, SectionLabel, SegmentedControl, Toggle } from "./primitives";
import { isShown, isUnavailable, useApp } from "../store";
import { hostPlatform } from "@/services/ipc";

/**
 * The touch shortcut and in-ear detection, for earbuds only. Each part shows
 * only where the earbuds answer for it, and every switch moves only when the
 * earbuds report the change.
 */
export function EarbudControlsPanel() {
  const {
    kind,
    connection,
    capabilities,
    shortcut,
    setShortcut,
    wear,
    setAutoPause,
    setAutoAnswer,
    busy,
  } = useApp();
  if (kind !== "earbuds") return null;

  const showTouch = isShown(capabilities.touchControls);
  const showPause = isShown(capabilities.inEarDetection);
  const showAnswer = isShown(capabilities.autoAnswer);
  if (!showTouch && !showPause && !showAnswer) return null;

  const connected = connection === "connected";
  const changing =
    busy === "setShortcut" || busy === "setAutoPause" || busy === "setAutoAnswer";
  const notYet = connected ? "Not read from the earbuds yet." : "Connect the earbuds to see this.";

  return (
    <Panel className="p-6" data-testid="earbud-controls">
      {showTouch && (
        <section>
          <div className="flex items-center justify-between gap-3">
            <h3 className="text-base font-semibold">Touch controls</h3>
            <CapabilityBadge cap={capabilities.touchControls} />
          </div>
          {shortcut ? (
            <>
              <p className="mt-2 text-sm text-muted-foreground">
                {shortcut.control} · {shortcut.gesture}:{" "}
                <span className="font-medium text-foreground">{shortcut.actionName}</span>
              </p>
              {shortcut.options.length > 0 ? (
                <SegmentedControl
                  options={shortcut.options.map((o) => ({ value: String(o.action), label: o.name }))}
                  value={String(shortcut.action)}
                  onChange={(v) => {
                    const action = Number(v);
                    if (action !== shortcut.action) setShortcut(action);
                  }}
                  disabled={!connected || changing || isUnavailable(capabilities.touchControls)}
                  className="mt-3 w-full flex-wrap [&>button]:flex-1 [&>button]:basis-[30%]"
                />
              ) : (
                <p className="mt-2 text-xs text-muted-foreground">
                  The earbuds do not list what else it can be set to, so it is not changed from
                  here.
                </p>
              )}
            </>
          ) : (
            <p className="mt-2 text-sm text-muted-foreground">{notYet}</p>
          )}
        </section>
      )}

      {(showPause || showAnswer) && (
        <section className={showTouch ? "mt-5 border-t border-border pt-4" : undefined}>
          <h3 className="text-base font-semibold">In-ear detection</h3>
          {showPause && (
            <div className="mt-3 flex items-center justify-between gap-3">
              <div>
                <div className="text-sm">Pause when taken out</div>
                <div className="text-xs text-muted-foreground">
                  {wear?.autoPause == null
                    ? notYet
                    : "Playback pauses when an earbud comes out, and plays again when it goes back in."}
                </div>
              </div>
              <Toggle
                label="Pause when taken out"
                checked={wear?.autoPause === true}
                disabled={!connected || wear?.autoPause == null || changing}
                onChange={setAutoPause}
              />
            </div>
          )}
          {showAnswer && (
            <div className="mt-3 flex items-center justify-between gap-3">
              <div>
                <div className="text-sm">Answer calls when put in</div>
                <div className="text-xs text-muted-foreground">
                  {wear?.autoAnswer == null ? notYet : "An incoming call is answered as an earbud goes in."}
                </div>
              </div>
              <Toggle
                label="Answer calls when put in"
                checked={wear?.autoAnswer === true}
                disabled={!connected || wear?.autoAnswer == null || changing}
                onChange={setAutoAnswer}
              />
            </div>
          )}
        </section>
      )}
    </Panel>
  );
}

/** A noted time, in the viewer's own terms. */
function when(iso: string): string {
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? iso : date.toLocaleString();
}

/**
 * "Find my buds": when, and on a phone where, the earbuds were last
 * connected. Off until switched on; kept on this device only; the map is the
 * phone's own maps application.
 */
export function FindMyBudsPanel() {
  const { kind, settings, toggleSetting, lastSeen, openLastSeenMap } = useApp();
  if (kind !== "earbuds") return null;

  const onPhone = hostPlatform() === "android";
  const on = settings.rememberLastSeen;
  const blocked = settings.unavailable.rememberLastSeen;
  const place = lastSeen?.place ?? null;

  return (
    <Panel className="p-6" data-testid="find-my-buds">
      <div className="flex items-center justify-between gap-3">
        <SectionLabel>Find my buds</SectionLabel>
        <Toggle
          label="Note where they were last connected"
          checked={on}
          disabled={!!blocked}
          onChange={() => toggleSetting("rememberLastSeen")}
        />
      </div>
      <p className="mt-2 text-sm text-muted-foreground">
        {onPhone
          ? "Notes when the earbuds were last connected to this phone, and where the phone was then. Kept on this phone only."
          : "Notes when the earbuds were last connected to this computer. Where they were is noted by the phone application."}
      </p>

      {on && (
        <div className="mt-4 rounded-lg border border-border bg-surface-2/50 p-4" data-testid="last-seen">
          {lastSeen ? (
            <>
              <div className="text-sm">
                <span className="text-muted-foreground">Last connected: </span>
                <span className="font-medium">{when(lastSeen.at)}</span>
              </div>
              {place ? (
                <>
                  <div className="mt-1 text-sm text-muted-foreground">
                    Near {(place.latitudeE6 / 1e6).toFixed(5)}, {(place.longitudeE6 / 1e6).toFixed(5)}
                    {place.accuracyM != null && ` (within about ${place.accuracyM} m)`}
                  </div>
                  {onPhone && (
                    <Button
                      size="sm"
                      variant="outline"
                      className="mt-3"
                      icon={<MapPin className="size-3.5" />}
                      onClick={openLastSeenMap}
                    >
                      Open in Maps
                    </Button>
                  )}
                </>
              ) : (
                onPhone && (
                  <div className="mt-1 text-xs text-muted-foreground">
                    No place noted: the phone had no location to give, or the permission was not
                    given.
                  </div>
                )
              )}
            </>
          ) : (
            <p className="text-sm text-muted-foreground">Nothing noted yet.</p>
          )}
        </div>
      )}
    </Panel>
  );
}
