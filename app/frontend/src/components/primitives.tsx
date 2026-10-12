import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  BatteryFull,
  BatteryLow,
  BatteryWarning,
  Bluetooth,
  Check,
  CircleHelp,
  FlaskConical,
  Loader2,
  MinusCircle,
  Zap,
} from "lucide-react";
import { motion, useMotionValue, useSpring } from "motion/react";
import { cn } from "./ui/utils";
import { capabilityMeta, type Capability, type ConnectionState } from "../store";

/* ---------------------------------------------------------------
 * AnimatedNumber — spring-counts to its target value
 * --------------------------------------------------------------- */
export function AnimatedNumber({ value, suffix = "" }: { value: number; suffix?: string }) {
  const mv = useMotionValue(value);
  const spring = useSpring(mv, { stiffness: 120, damping: 20 });
  const [display, setDisplay] = useState(value);
  useEffect(() => {
    mv.set(value);
  }, [value, mv]);
  useEffect(() => spring.on("change", (v) => setDisplay(Math.round(v))), [spring]);
  return (
    <span className="tabular-nums">
      {display}
      {suffix}
    </span>
  );
}

/* ---------------------------------------------------------------
 * Card / Panel — animated surface used app-wide (entrance + hover lift)
 * --------------------------------------------------------------- */
export function Panel({
  className,
  children,
  ...props
}: React.HTMLAttributes<HTMLDivElement>) {
  // Arrives and lifts by CSS. A panel is most of a page, and one that waits
  // on script to become visible is a page that stays blank whenever script
  // is busy, which on a phone it can be for a noticeable moment.
  return (
    <div
      className={cn(
        "panel-enter rounded-xl border border-border bg-card shadow-[var(--shadow-sm)]",
        "transition-[translate,box-shadow] duration-200 hover:-translate-y-1 hover:shadow-[var(--shadow-lg)]",
        className,
      )}
      {...props}
    >
      {children}
    </div>
  );
}

export function SectionLabel({ children }: { children: ReactNode }) {
  return (
    <div className="text-xs uppercase tracking-wider text-muted-foreground font-medium">
      {children}
    </div>
  );
}

/* ---------------------------------------------------------------
 * CapabilityBadge — never relies on color alone (icon + text)
 * --------------------------------------------------------------- */
const toneStyles: Record<string, string> = {
  success: "bg-success-subtle text-success",
  info: "bg-info-subtle text-info",
  neutral: "bg-muted text-muted-foreground",
  warning: "bg-warning-subtle text-warning",
  muted: "bg-muted/60 text-muted-foreground/70",
};

const capIcon: Record<Capability, ReactNode> = {
  verified: <Check className="size-3" />,
  supported: <Check className="size-3" />,
  unknown: <CircleHelp className="size-3" />,
  experimental: <FlaskConical className="size-3" />,
  unsupported: <MinusCircle className="size-3" />,
};

export function CapabilityBadge({ cap, className }: { cap: Capability; className?: string }) {
  const meta = capabilityMeta[cap];
  return (
    <motion.span
      key={cap}
      title={meta.note}
      initial={{ opacity: 0, scale: 0.7 }}
      animate={{ opacity: 1, scale: 1 }}
      whileHover={{ scale: 1.08 }}
      transition={{ type: "spring", stiffness: 500, damping: 22 }}
      className={cn(
        "inline-flex items-center gap-1 rounded-md px-2 py-0.5 text-xs font-medium",
        toneStyles[meta.tone],
        className,
      )}
    >
      <motion.span
        initial={{ rotate: -90, opacity: 0 }}
        animate={{ rotate: 0, opacity: 1 }}
        transition={{ delay: 0.05 }}
        className="inline-flex"
      >
        {capIcon[cap]}
      </motion.span>
      {meta.label}
    </motion.span>
  );
}

/* ---------------------------------------------------------------
 * ConnectionBadge
 * --------------------------------------------------------------- */
const connMeta: Record<
  ConnectionState,
  { label: string; tone: string; pulse?: boolean }
> = {
  connected: { label: "Connected", tone: "success" },
  disconnected: { label: "Disconnected", tone: "neutral" },
  connecting: { label: "Connecting", tone: "info", pulse: true },
  discovering: { label: "Discovering", tone: "info", pulse: true },
  reconnecting: { label: "Reconnecting", tone: "warning", pulse: true },
  error: { label: "Error", tone: "muted" },
  "bluetooth-disabled": { label: "Bluetooth Off", tone: "warning" },
  "device-unavailable": { label: "Unavailable", tone: "warning" },
  simulated: { label: "Simulated", tone: "warning" },
};

const dotColor: Record<string, string> = {
  success: "bg-success",
  info: "bg-info",
  neutral: "bg-muted-foreground",
  warning: "bg-warning",
  muted: "bg-error",
};

export function ConnectionBadge({ state, className }: { state: ConnectionState; className?: string }) {
  const m = connMeta[state];
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-md px-2 py-0.5 text-xs font-medium",
        toneStyles[m.tone],
        className,
      )}
    >
      <span className="relative flex size-1.5">
        {m.pulse && (
          <span className={cn("absolute inline-flex size-full animate-ping rounded-full opacity-75", dotColor[m.tone])} />
        )}
        <span
          className={cn("pulse-grow relative inline-flex size-1.5 rounded-full", dotColor[m.tone])}
        />
      </span>
      {m.label}
    </span>
  );
}

/* ---------------------------------------------------------------
 * BatteryIndicator
 * --------------------------------------------------------------- */
export function BatteryIndicator({
  level,
  charging,
  showLabel = true,
  size = "sm",
}: {
  /** `null` means the device has not reported a level — not zero. */
  level: number | null;
  charging?: boolean;
  showLabel?: boolean;
  size?: "sm" | "lg";
}) {
  // A missing reading is rendered as missing. Showing 0% would be a factual
  // claim about the battery that we have no basis for.
  if (level === null) {
    return (
      <span
        className="inline-flex items-center gap-1.5 text-muted-foreground"
        title="This device has not reported a battery level to Windows."
      >
        <BatteryWarning className={size === "lg" ? "size-5" : "size-4"} />
        {showLabel && (
          <span className={size === "lg" ? "text-base" : "text-sm"}>No reading</span>
        )}
      </span>
    );
  }

  const tone = level <= 10 ? "error" : level <= 20 ? "warning" : "success";
  const colorClass =
    tone === "error" ? "text-error" : tone === "warning" ? "text-warning" : "text-success";
  const Icon = charging ? Zap : level <= 10 ? BatteryWarning : level <= 20 ? BatteryLow : BatteryFull;
  return (
    <span className={cn("inline-flex items-center gap-1.5", colorClass, size === "lg" && "gap-2")}>
      <span
        className={cn(
          "inline-flex",
          charging ? "pulse-grow-slight" : level <= 20 ? "pulse-fade" : undefined,
        )}
      >
        <Icon className={size === "lg" ? "size-5" : "size-4"} />
      </span>
      {showLabel && (
        <span className={cn(size === "lg" ? "text-base font-medium" : "text-sm")}>
          <AnimatedNumber value={level} suffix="%" />
        </span>
      )}
    </span>
  );
}

/* ---------------------------------------------------------------
 * SegmentedControl — premium pill segmented control
 * --------------------------------------------------------------- */
export function SegmentedControl<T extends string>({
  options,
  value,
  onChange,
  disabled,
  className,
}: {
  options: {
    value: T;
    label: string;
    icon?: ReactNode;
    /**
     * Disables this option alone. It can still be shown as the active one —
     * the device may report a state the application cannot request.
     */
    disabled?: boolean;
    /** Why the option is disabled; shown as a tooltip. */
    hint?: string;
  }[];
  /**
   * `null` means the device has not told us which option is active, so none
   * is shown as selected. Defaulting to the first option would assert a
   * device state we have not observed.
   */
  value: T | null;
  onChange: (v: T) => void;
  disabled?: boolean;
  className?: string;
}) {
  const idRef = useRef(Math.random().toString(36).slice(2));
  const id = idRef.current;
  return (
    <div
      role="tablist"
      aria-disabled={disabled}
      className={cn(
        "inline-flex items-center gap-1 rounded-lg border border-border bg-surface-2 p-1",
        disabled && "opacity-50",
        className,
      )}
    >
      {options.map((o) => {
        const active = o.value === value;
        const off = disabled || !!o.disabled;
        return (
          <motion.button
            key={o.value}
            role="tab"
            aria-selected={active}
            disabled={off}
            title={o.disabled ? o.hint : undefined}
            onClick={() => !off && onChange(o.value)}
            whileTap={off ? undefined : { scale: 0.94 }}
            className={cn(
              "relative inline-flex items-center justify-center gap-2 rounded-md px-4 py-1.5 text-sm outline-none",
              "focus-visible:ring-2 focus-visible:ring-ring",
              active ? "text-foreground" : "text-muted-foreground",
              !off && !active && "hover:text-foreground",
              !off && "cursor-pointer",
              // Dim a single disabled option only when the whole control is
              // not already dimmed.
              o.disabled && !disabled && "cursor-not-allowed opacity-45",
            )}
          >
            {active && (
              <motion.span
                layoutId={`seg-${id}`}
                className="absolute inset-0 rounded-md bg-card shadow-[var(--shadow-sm)]"
                transition={{ type: "spring", stiffness: 400, damping: 32 }}
              />
            )}
            <span className="relative inline-flex items-center gap-2">
              {o.icon}
              {o.label}
            </span>
          </motion.button>
        );
      })}
    </div>
  );
}

/* ---------------------------------------------------------------
 * Range — accessible themed slider with labels
 * --------------------------------------------------------------- */
export function Range({
  label,
  value,
  min = 0,
  max = 100,
  step = 1,
  onChange,
  disabled,
  suffix,
  displayValue,
  leftLabel,
  rightLabel,
  continuous = false,
}: {
  label?: string;
  value: number;
  min?: number;
  max?: number;
  step?: number;
  onChange: (v: number) => void;
  disabled?: boolean;
  suffix?: string;
  displayValue?: string;
  leftLabel?: string;
  rightLabel?: string;
  /**
   * Fire `onChange` on every intermediate value instead of on release.
   *
   * Off by default. These sliders are bound to hardware commands, and the
   * upstream behaviour issued one command per input event — dragging the
   * volume slider across its range produced a burst of IPC calls and a wall
   * of log entries, each racing the others' state read-back. The slider now
   * tracks the drag locally and commits once, when the user lets go.
   */
  continuous?: boolean;
}) {
  const [draft, setDraft] = useState(value);
  const [dragging, setDragging] = useState(false);

  // The device remains the source of truth: whenever it reports a new value
  // and the user is not mid-drag, the slider snaps to what the device says.
  useEffect(() => {
    if (!dragging) setDraft(value);
  }, [value, dragging]);

  const shown = dragging ? draft : value;

  // Letting go can arrive more than once — as a pointer event and again as
  // a touch event on a phone. These sliders drive hardware commands, and a
  // command must go out once, so the same value is not committed twice in
  // quick succession.
  const lastCommit = useRef<{ value: number; at: number } | null>(null);

  const commit = (v: number) => {
    setDragging(false);
    if (v === value) return;
    const last = lastCommit.current;
    const now = Date.now();
    if (last && last.value === v && now - last.at < 800) return;
    lastCommit.current = { value: v, at: now };
    onChange(v);
  };

  const pct = ((shown - min) / (max - min)) * 100;
  return (
    <div className={cn("w-full", disabled && "opacity-50")}>
      {label && (
        <div className="mb-2 flex items-center justify-between">
          <span className="text-sm text-foreground">{label}</span>
          <span className="text-sm tabular-nums text-muted-foreground">
            {dragging ? `${draft}${suffix ?? ""}` : (displayValue ?? `${value}${suffix ?? ""}`)}
          </span>
        </div>
      )}
      <div className="flex items-center gap-3">
        {leftLabel && <span className="w-8 text-right text-xs tabular-nums text-muted-foreground">{leftLabel}</span>}
        <div className="relative flex-1">
          <div className="h-1.5 w-full rounded-full bg-surface-2" />
          {/* `initial={false}`: a slider appears showing its value. It does
              not sweep up to it from zero, which reads as the value changing. */}
          <motion.div
            className="absolute top-1/2 h-1.5 -translate-y-1/2 rounded-full bg-primary"
            initial={false}
            animate={{ width: `${pct}%` }}
            transition={{ type: "spring", stiffness: 320, damping: 30 }}
          />
          {/* The halo follows the thumb with a spring, and breathes by
              itself in CSS: a pulse that never ends is not something to
              drive from script on every frame, for every slider on screen. */}
          <motion.div
            className="pointer-events-none absolute top-1/2 size-4 -translate-x-1/2 -translate-y-1/2"
            initial={false}
            animate={{ left: `${pct}%` }}
            transition={{ type: "spring", stiffness: 320, damping: 30 }}
          >
            <span className="thumb-halo block size-full rounded-full bg-primary/25" />
          </motion.div>
          <input
            type="range"
            aria-label={label}
            min={min}
            max={max}
            step={step}
            value={shown}
            disabled={disabled}
            onChange={(e) => {
              const v = Number(e.target.value);
              if (continuous) {
                onChange(v);
                return;
              }
              setDragging(true);
              setDraft(v);
            }}
            onPointerUp={(e) => !continuous && commit(Number((e.target as HTMLInputElement).value))}
            // A finger: some browsers end a drag on a slider with a touch
            // event and no pointer-up at all.
            onTouchEnd={(e) => !continuous && commit(Number((e.target as HTMLInputElement).value))}
            onKeyUp={(e) => !continuous && commit(Number((e.target as HTMLInputElement).value))}
            onBlur={(e) => !continuous && dragging && commit(Number(e.target.value))}
            className="absolute inset-0 top-1/2 h-4 w-full -translate-y-1/2 cursor-pointer touch-none appearance-none bg-transparent outline-none disabled:cursor-not-allowed
              [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:size-4 [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:border-2 [&::-webkit-slider-thumb]:border-primary [&::-webkit-slider-thumb]:bg-card [&::-webkit-slider-thumb]:shadow-[var(--shadow-sm)] [&::-webkit-slider-thumb]:transition-transform [&::-webkit-slider-thumb]:hover:scale-110
              [&::-moz-range-thumb]:size-4 [&::-moz-range-thumb]:rounded-full [&::-moz-range-thumb]:border-2 [&::-moz-range-thumb]:border-primary [&::-moz-range-thumb]:bg-card"
          />
        </div>
        {rightLabel && <span className="w-8 text-xs tabular-nums text-muted-foreground">{rightLabel}</span>}
      </div>
    </div>
  );
}

/* ---------------------------------------------------------------
 * Toggle switch
 * --------------------------------------------------------------- */
export function Toggle({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean;
  onChange: (b: boolean) => void;
  disabled?: boolean;
  label?: string;
}) {
  return (
    <motion.button
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      whileTap={{ scale: 0.9 }}
      className={cn(
        "relative inline-flex h-6 w-11 shrink-0 items-center rounded-full px-0.5 transition-colors outline-none focus-visible:ring-2 focus-visible:ring-ring",
        checked ? "justify-end bg-primary" : "justify-start bg-switch-background",
        disabled && "opacity-50 cursor-not-allowed",
      )}
    >
      <motion.span
        layout
        transition={{ type: "spring", stiffness: 700, damping: 30 }}
        className="block size-5 rounded-full bg-white shadow"
      />
    </motion.button>
  );
}

/* ---------------------------------------------------------------
 * Button
 * --------------------------------------------------------------- */
export function Button({
  variant = "primary",
  size = "md",
  loading,
  icon,
  className,
  children,
  ...props
}: {
  variant?: "primary" | "secondary" | "ghost" | "danger" | "outline";
  size?: "sm" | "md";
  loading?: boolean;
  icon?: ReactNode;
} & React.ButtonHTMLAttributes<HTMLButtonElement>) {
  const variants: Record<string, string> = {
    primary: "bg-primary text-primary-foreground hover:brightness-110 active:brightness-95",
    secondary: "bg-secondary text-secondary-foreground hover:bg-accent",
    ghost: "text-muted-foreground hover:bg-accent hover:text-foreground",
    danger: "bg-error text-white hover:brightness-110",
    outline: "border border-border text-foreground hover:bg-accent",
  };
  return (
    <motion.button
      whileHover={{ scale: 1.04 }}
      whileTap={{ scale: 0.95 }}
      transition={{ type: "spring", stiffness: 500, damping: 25 }}
      className={cn(
        "inline-flex items-center justify-center gap-2 rounded-lg font-medium outline-none",
        "focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50 disabled:pointer-events-none",
        size === "sm" ? "h-8 px-3 text-sm" : "h-9 px-4 text-sm",
        variants[variant],
        className,
      )}
      disabled={loading || props.disabled}
      {...(props as any)}
    >
      {loading ? <Loader2 className="size-4 animate-spin" /> : icon}
      {children}
    </motion.button>
  );
}

export function BtIcon({ className }: { className?: string }) {
  return <Bluetooth className={className} />;
}
