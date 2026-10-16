import { LogOut, Power, RefreshCw, Settings, Wifi } from "lucide-react";
import { motion, type Variants } from "motion/react";
import { BatteryIndicator } from "./primitives";
import { useApp } from "../store";
import { cn } from "./ui/utils";

const container: Variants = {
  hidden: {},
  show: { transition: { staggerChildren: 0.045, delayChildren: 0.05 } },
};
const item: Variants = {
  hidden: { opacity: 0, x: -10 },
  show: { opacity: 1, x: 0, transition: { type: "spring", stiffness: 400, damping: 26 } },
};

/** Windows-style system tray popup. */
export function TrayMenu({
  onOpen,
  onClose,
}: {
  onOpen: () => void;
  onClose: () => void;
}) {
  const { connection, battery, charging, noise, volume, reconnect, disconnect } = useApp();
  const connected = connection === "connected" || connection === "simulated";

  const Row = ({ label, value }: { label: string; value: React.ReactNode }) => (
    <motion.div variants={item} className="flex items-center justify-between py-1.5">
      <span className="text-xs text-muted-foreground">{label}</span>
      <span className="text-sm">{value}</span>
    </motion.div>
  );

  const Action = ({
    icon: Icon,
    label,
    onClick,
    danger,
  }: {
    icon: React.ElementType;
    label: string;
    onClick: () => void;
    danger?: boolean;
  }) => (
    <motion.button
      variants={item}
      onClick={onClick}
      whileHover={{ x: 4 }}
      whileTap={{ scale: 0.97 }}
      className={cn(
        "flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-sm outline-none transition-colors hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring",
        danger && "text-error hover:bg-error-subtle",
      )}
    >
      <motion.span whileHover={{ rotate: 12 }} className="inline-flex">
        <Icon className="size-4" />
      </motion.span>
      {label}
    </motion.button>
  );

  return (
    <motion.div
      variants={container}
      initial="hidden"
      animate="show"
      className="w-72 overflow-hidden rounded-xl border border-border bg-popover shadow-[var(--shadow-lg)]"
    >
      <motion.div variants={item} className="flex items-center gap-2.5 border-b border-border px-4 py-3">
        <motion.div
          className="flex size-8 items-center justify-center rounded-md bg-primary/15 text-primary"
          animate={connected ? { scale: [1, 1.12, 1] } : {}}
          transition={{ duration: 1.8, repeat: Infinity, ease: "easeInOut" }}
        >
          <Wifi className="size-4" />
        </motion.div>
        <div>
          <div className="text-sm font-medium">Bose QC</div>
          <div className={cn("text-xs", connected ? "text-success" : "text-muted-foreground")}>
            {connected ? "● Connected" : "○ Disconnected"}
          </div>
        </div>
      </motion.div>

      <div className="px-4 py-2">
        <Row label="Battery" value={<BatteryIndicator level={battery} charging={charging} />} />
        <Row label="Noise Control" value={<span className="capitalize">{noise}</span>} />
        <Row label="Volume" value={<span className="tabular-nums">{volume}%</span>} />
      </div>

      <div className="border-t border-border p-1.5">
        <Action icon={Settings} label="Open Void" onClick={onOpen} />
        <Action icon={RefreshCw} label="Reconnect" onClick={reconnect} />
        <Action icon={Power} label="Disconnect" onClick={disconnect} danger />
      </div>

      <div className="border-t border-border p-1.5">
        <Action icon={Settings} label="Settings" onClick={onOpen} />
        <Action icon={LogOut} label="Exit" onClick={onClose} danger />
      </div>
    </motion.div>
  );
}
