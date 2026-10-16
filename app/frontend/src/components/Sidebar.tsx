import {
  ArrowLeftRight,
  LayoutDashboard,
  Settings,
  SlidersHorizontal,
  Speaker,
  Volume2,
  Waves,
} from "lucide-react";
import { motion } from "motion/react";
import { cn } from "./ui/utils";
import { BatteryIndicator, ConnectionBadge } from "./primitives";
import { shownPages, useApp } from "../store";
import type { PageId } from "../pages";
import wolfLogo from "@/assets/wolf-logo.png";

const nav: { id: PageId; label: string; short: string; icon: React.ElementType }[] = [
  { id: "dashboard", label: "Dashboard", short: "Home", icon: LayoutDashboard },
  { id: "device", label: "Device", short: "Device", icon: Speaker },
  { id: "noise", label: "Noise Control", short: "Noise", icon: Waves },
  { id: "equalizer", label: "Equalizer", short: "EQ", icon: SlidersHorizontal },
  { id: "profiles", label: "Profiles", short: "Profiles", icon: Volume2 },
  { id: "settings", label: "Settings", short: "Settings", icon: Settings },
];

/**
 * The same destinations as the sidebar, along the bottom edge. Shown only
 * where the window is too narrow for a sidebar — a phone.
 */
export function BottomNav({
  page,
  onNavigate,
}: {
  page: PageId;
  onNavigate: (p: PageId) => void;
}) {
  const { capabilities } = useApp();
  const shown = shownPages(capabilities);
  return (
    <nav
      aria-label="Sections"
      className="flex shrink-0 border-t border-sidebar-border bg-sidebar md:hidden"
    >
      {nav.filter((item) => shown.has(item.id)).map((item) => {
        const active = page === item.id;
        const Icon = item.icon;
        return (
          <button
            key={item.id}
            onClick={() => onNavigate(item.id)}
            aria-label={item.label}
            aria-current={active ? "page" : undefined}
            className={cn(
              "relative flex min-w-0 flex-1 flex-col items-center gap-0.5 px-0.5 pb-2 pt-2.5 text-[10px] leading-none outline-none transition-colors",
              "focus-visible:ring-2 focus-visible:ring-sidebar-ring",
              active ? "text-primary" : "text-muted-foreground",
            )}
          >
            {active && (
              <motion.span
                layoutId="bottom-nav-active"
                className="absolute top-0 h-0.5 w-8 rounded-full bg-primary"
              />
            )}
            <Icon className="size-5" />
            <span className="max-w-full truncate">{item.short}</span>
          </button>
        );
      })}
    </nav>
  );
}

export function Sidebar({
  page,
  onNavigate,
}: {
  page: PageId;
  onNavigate: (p: PageId) => void;
}) {
  const { connection, battery, charging, capabilities, changeDevice, model, deviceName } = useApp();
  const shown = nav.filter((item) => shownPages(capabilities).has(item.id));

  return (
    <aside className="hidden w-60 shrink-0 flex-col border-r border-sidebar-border bg-sidebar md:flex">
      {/* brand */}
      <div className="flex items-center gap-3 px-5 py-5">
        <motion.div
          className="flex size-14 shrink-0 items-center justify-center overflow-hidden rounded-xl bg-white ring-1 ring-border shadow-[var(--shadow-md)]"
          whileHover={{ scale: 1.08, rotate: [0, -6, 6, 0] }}
          transition={{ duration: 0.5 }}
        >
          <img src={wolfLogo} alt="VOID logo" className="size-full object-cover" />
        </motion.div>
        <div className="leading-tight">
          <div className="text-lg font-semibold tracking-[0.2em]">VOID</div>
          <div className="text-xs text-muted-foreground">Control Center</div>
        </div>
      </div>

      {/* nav */}
      <nav className="flex-1 px-3 py-2">
        <ul className="space-y-0.5">
          {shown.map((item) => {
            const active = page === item.id;
            const Icon = item.icon;
            return (
              <motion.li
                key={item.id}
                initial={{ opacity: 0, x: -16 }}
                animate={{ opacity: 1, x: 0 }}
                transition={{ delay: 0.04 * shown.indexOf(item), type: "spring", stiffness: 300, damping: 24 }}
              >
                <motion.button
                  onClick={() => onNavigate(item.id)}
                  whileHover={{ x: 4 }}
                  whileTap={{ scale: 0.97 }}
                  className={cn(
                    "group relative flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm outline-none transition-colors",
                    "focus-visible:ring-2 focus-visible:ring-sidebar-ring",
                    active
                      ? "bg-sidebar-accent text-sidebar-accent-foreground"
                      : "text-muted-foreground hover:bg-sidebar-accent/50 hover:text-sidebar-foreground",
                  )}
                >
                  {active && (
                    <motion.span
                      layoutId="nav-active"
                      className="absolute left-0 top-1/2 h-5 w-0.5 -translate-y-1/2 rounded-full bg-primary"
                    />
                  )}
                  <motion.span
                    animate={active ? { scale: [1, 1.25, 1] } : {}}
                    transition={{ duration: 0.4 }}
                    className="inline-flex"
                  >
                    <Icon className="size-4" />
                  </motion.span>
                  {item.label}
                </motion.button>
              </motion.li>
            );
          })}
        </ul>
      </nav>

      {/* status footer */}
      <div className="m-3 space-y-3 rounded-lg border border-sidebar-border bg-card/60 p-3">
        <div className="min-w-0">
          <div className="truncate text-sm font-medium">{deviceName}</div>
          {model && <div className="truncate text-xs text-muted-foreground">{model.name}</div>}
        </div>
        <div className="flex items-center justify-between">
          <span className="text-xs text-muted-foreground">Connection</span>
          <ConnectionBadge state={connection} />
        </div>
        <div className="flex items-center justify-between">
          <span className="text-xs text-muted-foreground">Battery</span>
          <BatteryIndicator level={battery} charging={charging} />
        </div>
        <button
          onClick={changeDevice}
          className="flex w-full items-center justify-center gap-2 rounded-md border border-sidebar-border px-2 py-1.5 text-xs text-muted-foreground outline-none transition-colors hover:bg-sidebar-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-sidebar-ring"
        >
          <ArrowLeftRight className="size-3.5" /> Change device
        </button>
      </div>
    </aside>
  );
}
