import type { CSSProperties } from "react";
import { cn } from "./ui/utils";

/**
 * Headphone illustration, drawn locally.
 *
 * The Figma export used an Unsplash photo. That is replaced here because this
 * application makes no network requests by design — the CSP in index.html and
 * tauri.conf.json restricts `img-src` to `'self' data:`, so the remote image
 * was blocked outright and rendered as a broken fallback. Drawing it inline
 * also means the visual works with no network at all, which matters for an
 * offline-first desktop utility.
 *
 * It is a generic over-ear illustration, deliberately not a depiction of any
 * specific manufacturer's product.
 */
function HeadphoneMark({ active }: { active: boolean }) {
  return (
    <svg
      viewBox="0 0 200 200"
      className={cn("size-full transition-opacity duration-700", !active && "opacity-60")}
      role="img"
      aria-label="Over-ear headphones"
    >
      <defs>
        <linearGradient id="dv-cup" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="var(--chart-4)" stopOpacity="0.95" />
          <stop offset="100%" stopColor="var(--primary)" stopOpacity="0.75" />
        </linearGradient>
        <linearGradient id="dv-band" x1="0" y1="0" x2="1" y2="0">
          <stop offset="0%" stopColor="var(--primary)" stopOpacity="0.5" />
          <stop offset="50%" stopColor="var(--chart-4)" stopOpacity="0.95" />
          <stop offset="100%" stopColor="var(--primary)" stopOpacity="0.5" />
        </linearGradient>
      </defs>

      {/* headband */}
      <path
        d="M46 116 V96a54 54 0 0 1 108 0v20"
        fill="none"
        stroke="url(#dv-band)"
        strokeWidth="13"
        strokeLinecap="round"
      />
      <path
        d="M46 112 V96a54 54 0 0 1 108 0v16"
        fill="none"
        stroke="white"
        strokeOpacity="0.18"
        strokeWidth="3"
        strokeLinecap="round"
      />

      {/* earcups */}
      {[36, 136].map((x) => (
        <g key={x}>
          <rect x={x} y="104" width="28" height="52" rx="14" fill="url(#dv-cup)" />
          <rect
            x={x + 5}
            y="111"
            width="18"
            height="38"
            rx="9"
            fill="black"
            fillOpacity="0.22"
          />
          <rect
            x={x + 3}
            y="107"
            width="22"
            height="16"
            rx="8"
            fill="white"
            fillOpacity="0.16"
          />
        </g>
      ))}
    </svg>
  );
}

/** A pair of earbuds, drawn the same way: generic, not any one product. */
function EarbudsMark({ active }: { active: boolean }) {
  return (
    <svg
      viewBox="0 0 200 200"
      className={cn("size-full transition-opacity duration-700", !active && "opacity-60")}
      role="img"
      aria-label="Earbuds"
    >
      <defs>
        <linearGradient id="dv-bud" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="var(--chart-4)" stopOpacity="0.95" />
          <stop offset="100%" stopColor="var(--primary)" stopOpacity="0.75" />
        </linearGradient>
      </defs>
      {[
        { x: 60, turn: -18, tip: 22 },
        { x: 140, turn: 18, tip: -22 },
      ].map(({ x, turn, tip }) => (
        <g key={x} transform={`rotate(${turn} ${x} 110)`}>
          {/* ear tip */}
          <ellipse cx={x + tip} cy="72" rx="17" ry="13" fill="url(#dv-bud)" fillOpacity="0.7" />
          {/* body */}
          <ellipse cx={x} cy="112" rx="35" ry="42" fill="url(#dv-bud)" />
          <ellipse cx={x} cy="121" rx="22" ry="26" fill="black" fillOpacity="0.2" />
          <ellipse cx={x - 8} cy="92" rx="14" ry="10" fill="white" fillOpacity="0.16" />
        </g>
      ))}
    </svg>
  );
}

/** Headphones or earbuds, whichever the device is. */
export function DeviceMark({
  kind = "headphones",
  active = true,
}: {
  kind?: "headphones" | "earbuds";
  active?: boolean;
}) {
  return kind === "earbuds" ? <EarbudsMark active={active} /> : <HeadphoneMark active={active} />;
}

/** The arc of light that travels round the ring: dark, then warm, then white. */
const ARC =
  "conic-gradient(from 0deg, transparent 0deg, transparent 250deg, rgba(255,138,76,0.55) 320deg, #ffffff 355deg, rgba(180,120,255,0.55) 360deg)";

/** Cuts a conic gradient down to a band at its rim, `width` pixels wide. */
function rim(width: number, soft = false): CSSProperties {
  const band = soft
    ? `radial-gradient(farthest-side, transparent calc(100% - ${width}px), rgba(0,0,0,0.55) calc(100% - ${width / 2}px), transparent 100%)`
    : `radial-gradient(farthest-side, transparent calc(100% - ${width + 1}px), #000 calc(100% - ${width}px))`;
  return { WebkitMask: band, mask: band };
}

/**
 * DeviceVisual — the headphones, with light moving round them.
 *
 * Everything that moves here does so by rotating, sliding, scaling or fading
 * a layer that is otherwise left alone: the animations are CSS keyframes on
 * `transform` and `opacity` (see `styles.css`), which the browser runs on its
 * compositor without redrawing anything and without waiting for script.
 *
 * It was not always so. The first version drove each layer from script every
 * frame, animated a `filter`, blended a rotating layer onto the picture with
 * `mix-blend-mode`, and relied on a rounded `overflow: hidden` to trim it.
 * On a phone that meant redrawing filtered, masked layers sixty times a
 * second, stuttering whenever the page was busy with anything else, and the
 * rounded trim giving way for a frame now and then — a square of light
 * flashing where the circle should have cut it off. So: no animated filters,
 * no blending, and nothing that needs trimming, because every layer is
 * already the shape it should be.
 */
export function DeviceVisual({
  size = 320,
  active = true,
  kind = "headphones",
  className,
}: {
  size?: number;
  active?: boolean;
  /** What is drawn in the middle. */
  kind?: "headphones" | "earbuds";
  className?: string;
}) {
  const ring = size * 0.8; // picture diameter
  const orbit = ring * 1.12;

  return (
    <div
      className={cn("relative flex items-center justify-center", className)}
      style={{ width: size, height: size }}
    >
      {/* ambient breathing glow — a gradient that is already soft, so it
          needs no blur */}
      <div
        className={cn("dv-layer absolute rounded-full", active ? "dv-breathe" : "opacity-[0.08]")}
        style={{
          width: size * 1.1,
          height: size * 1.1,
          background:
            "radial-gradient(circle, color-mix(in oklch, var(--primary) 70%, transparent) 0%, color-mix(in oklch, var(--primary) 25%, transparent) 38%, transparent 68%)",
        }}
      />

      {/* REVOLVING LIGHT RING — a bright arc that sweeps round the circle,
          with a wider, fainter copy behind it for the glow. One layer turns;
          the two bands are drawn into it once. */}
      {active && (
        <div
          className="dv-layer dv-spin absolute"
          style={{ width: orbit + 20, height: orbit + 20, animationDuration: "9s" }}
        >
          <div className="absolute inset-0 rounded-full" style={{ background: ARC, ...rim(22, true) }} />
          <div
            className="absolute rounded-full"
            style={{ inset: 10, background: ARC, ...rim(6) }}
          />
        </div>
      )}

      {/* expanding sound rings, a third of the way apart */}
      {active &&
        [0, 1, 2].map((i) => (
          <span
            key={i}
            className="dv-layer dv-ripple absolute rounded-full border-2 border-primary/40"
            style={{ width: ring * 1.05, height: ring * 1.05, animationDelay: `${-i * 2}s` }}
          />
        ))}

      {/* slowly counter-rotating dashed halo */}
      {active && (
        <svg
          className="dv-layer dv-spin-back absolute"
          width={size * 0.98}
          height={size * 0.98}
          viewBox="0 0 100 100"
          style={{ animationDuration: "40s" }}
        >
          <circle
            cx="50"
            cy="50"
            r="47"
            fill="none"
            stroke="var(--primary)"
            strokeOpacity="0.3"
            strokeWidth="0.6"
            strokeDasharray="1 6"
            strokeLinecap="round"
          />
        </svg>
      )}

      {/* ORBITING LIGHT DOT — a glowing bead that circles the visual */}
      {active && (
        <div
          className="dv-layer dv-spin absolute"
          style={{ width: orbit, height: orbit, animationDuration: "12s" }}
        >
          <span
            className="dv-layer dv-pulse absolute left-1/2 top-0 -ml-1.5 -mt-1.5 size-3 rounded-full bg-white"
            style={{ boxShadow: "0 0 12px 4px rgba(255,150,90,0.9), 0 0 4px 1px #fff" }}
          />
        </div>
      )}

      {/* floating headphone picture. Nothing inside it reaches past its
          edge, so there is nothing to trim. */}
      <div
        className={cn(
          "dv-layer relative rounded-full border border-white/20 shadow-[var(--shadow-lg)]",
          active && "dv-float",
        )}
        style={{ width: ring, height: ring }}
      >
        <div className="absolute inset-0 rounded-full bg-gradient-to-br from-surface-2 to-card" />
        <div className="absolute inset-0 p-[14%]">
          <DeviceMark kind={kind} active={active} />
        </div>
        {/* subtle inner sheen */}
        <div className="pointer-events-none absolute inset-0 rounded-full bg-gradient-to-tr from-transparent via-transparent to-white/15" />

        {/* a highlight sweeping across the picture: plain white at low
            opacity on a round layer of its own, fading out toward the
            middle so that it reads as light catching the rim rather than
            as a beam from the centre */}
        {active && (
          <div
            className="dv-layer dv-spin pointer-events-none absolute inset-0 rounded-full"
            style={{
              animationDuration: "10s",
              background:
                "conic-gradient(from 0deg, transparent 0deg, transparent 285deg, rgba(255,255,255,0.05) 315deg, rgba(255,255,255,0.26) 348deg, rgba(255,255,255,0.05) 357deg, transparent 360deg)",
              WebkitMask: "radial-gradient(circle, transparent 18%, #000 78%)",
              mask: "radial-gradient(circle, transparent 18%, #000 78%)",
            }}
          />
        )}
      </div>

      {/* soft contact shadow beneath — again a gradient rather than a blur */}
      <div
        className={cn("dv-layer absolute rounded-full", active ? "dv-shadow" : "opacity-30")}
        style={{
          width: size * 0.46,
          height: size * 0.08,
          bottom: size * 0.05,
          background: "radial-gradient(closest-side, rgba(0,0,0,0.5) 0%, rgba(0,0,0,0.25) 45%, transparent 100%)",
        }}
      />
    </div>
  );
}
