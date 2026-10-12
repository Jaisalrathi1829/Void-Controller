import { useEffect, useRef, useState } from "react";
import { ChevronDown, Monitor, Moon, PanelTop, Sun } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { pageTitles, type PageId } from "../pages";
import { useApp, type ConnectionState, type ThemePref } from "../store";
import { ConnectionBadge, Button } from "./primitives";
import { TrayMenu } from "./TrayMenu";
import { cn } from "./ui/utils";

const themeCycle: Record<ThemePref, ThemePref> = { dark: "light", light: "system", system: "dark" };
const themeIcon: Record<ThemePref, React.ElementType> = { dark: Moon, light: Sun, system: Monitor };

const simStates: { value: ConnectionState; label: string }[] = [
  { value: "connected", label: "Connected" },
  { value: "connecting", label: "Connecting" },
  { value: "discovering", label: "Discovering" },
  { value: "reconnecting", label: "Reconnecting" },
  { value: "disconnected", label: "Disconnected" },
  { value: "device-unavailable", label: "Device Unavailable" },
  { value: "bluetooth-disabled", label: "Bluetooth Off" },
  { value: "error", label: "Error" },
  { value: "simulated", label: "Simulated Device" },
];

export function TopBar({ page }: { page: PageId }) {
  const { connection, setConnection, theme, setTheme } = useApp();
  const [trayOpen, setTrayOpen] = useState(false);
  const [simOpen, setSimOpen] = useState(false);
  const meta = pageTitles[page];
  const ThemeIcon = themeIcon[theme];

  const trayRef = useRef<HTMLDivElement>(null);
  const simRef = useRef<HTMLDivElement>(null);

  // Close any open dropdown when clicking outside it or pressing Escape.
  // (A backdrop-filter ancestor traps `position: fixed` overlays, so we
  // detect outside clicks via refs on the document instead.)
  useEffect(() => {
    if (!trayOpen && !simOpen) return;
    const onPointerDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (trayOpen && trayRef.current && !trayRef.current.contains(t)) setTrayOpen(false);
      if (simOpen && simRef.current && !simRef.current.contains(t)) setSimOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setTrayOpen(false);
        setSimOpen(false);
      }
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [trayOpen, simOpen]);

  return (
    // Frosted on a desktop. On a phone the bar is simply opaque: blurring
    // whatever drifts behind it, every frame, is work a phone shows as
    // stutter, for an effect nobody would miss at that size.
    <header className="relative z-50 flex items-center justify-between gap-3 border-b border-border bg-background/95 px-4 py-3 md:bg-background/80 md:px-8 md:py-4 md:backdrop-blur">
      <AnimatePresence mode="wait">
        <motion.div
          key={page}
          initial={{ opacity: 0, x: -14 }}
          animate={{ opacity: 1, x: 0 }}
          exit={{ opacity: 0, x: 14 }}
          transition={{ duration: 0.22 }}
        >
          <h1>{meta.title}</h1>
          <p className="hidden text-sm text-muted-foreground sm:block">{meta.subtitle}</p>
        </motion.div>
      </AnimatePresence>

      <div className="flex items-center gap-2">
        {/* connection state simulator (dev tool) */}
        <div className="relative" ref={simRef}>
          <button
            onClick={() => setSimOpen((o) => !o)}
            className="flex items-center gap-2 rounded-lg border border-border bg-card px-2.5 py-1.5 text-sm outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring"
          >
            <ConnectionBadge state={connection} />
            <motion.span animate={{ rotate: simOpen ? 180 : 0 }} transition={{ duration: 0.2 }}>
              <ChevronDown className="size-3.5 text-muted-foreground" />
            </motion.span>
          </button>
          <AnimatePresence>
            {simOpen && (
              <motion.div
                initial={{ opacity: 0, y: -6, scale: 0.97 }}
                animate={{ opacity: 1, y: 0, scale: 1 }}
                exit={{ opacity: 0, y: -6, scale: 0.97 }}
                transition={{ type: "spring", stiffness: 400, damping: 28 }}
                className="absolute right-0 z-50 mt-2 w-56 origin-top-right rounded-lg border border-border bg-popover p-1.5 shadow-[var(--shadow-lg)]"
              >
                <div className="px-2 py-1 text-xs text-muted-foreground">Simulate connection state</div>
                {simStates.map((s, i) => (
                  <motion.button
                    key={s.value}
                    initial={{ opacity: 0, x: -8 }}
                    animate={{ opacity: 1, x: 0 }}
                    transition={{ delay: 0.02 * i }}
                    whileHover={{ x: 3 }}
                    whileTap={{ scale: 0.97 }}
                    onClick={() => {
                      setConnection(s.value);
                      setSimOpen(false);
                    }}
                    className={cn(
                      "flex w-full items-center justify-between rounded-md px-2 py-1.5 text-sm outline-none hover:bg-accent",
                      connection === s.value && "bg-accent",
                    )}
                  >
                    {s.label}
                  </motion.button>
                ))}
              </motion.div>
            )}
          </AnimatePresence>
        </div>

        <Button variant="outline" size="sm" onClick={() => setTheme(themeCycle[theme])} icon={<ThemeIcon className="size-4" />}>
          <span className="hidden capitalize sm:inline">{theme}</span>
        </Button>

        {/* tray — a desktop notion; a phone has none */}
        <div className="relative hidden md:block" ref={trayRef}>
          <Button variant="outline" size="sm" onClick={() => setTrayOpen((o) => !o)} icon={<PanelTop className="size-4" />}>
            Tray
          </Button>
          <AnimatePresence>
            {trayOpen && (
              <motion.div
                initial={{ opacity: 0, y: -6, scale: 0.97 }}
                animate={{ opacity: 1, y: 0, scale: 1 }}
                exit={{ opacity: 0, y: -6, scale: 0.97 }}
                transition={{ type: "spring", stiffness: 400, damping: 28 }}
                className="absolute right-0 z-50 mt-2 origin-top-right"
              >
                <TrayMenu onOpen={() => setTrayOpen(false)} onClose={() => setTrayOpen(false)} />
              </motion.div>
            )}
          </AnimatePresence>
        </div>
      </div>
    </header>
  );
}
