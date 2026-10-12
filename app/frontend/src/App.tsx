import { useEffect, useState } from "react";
import { AnimatePresence, motion } from "motion/react";
import { Toaster } from "./components/ui/sonner";
import { AppProvider, useApp } from "./store";
import { type PageId } from "./pages";
import { BottomNav, Sidebar } from "./components/Sidebar";
import { TopBar } from "./components/TopBar";
import { SimulatedBanner } from "./components/SimulatedBanner";
import { ConnectionScreen } from "./components/ConnectionScreens";
import { DashboardPage } from "./components/pages/DashboardPage";
import { DevicePage } from "./components/pages/DevicePage";
import { NoisePage } from "./components/pages/NoisePage";
import { EqualizerPage } from "./components/pages/EqualizerPage";
import { ProfilesPage } from "./components/pages/ProfilesPage";
import { DiagnosticsPage } from "./components/pages/DiagnosticsPage";
import { SettingsPage } from "./components/pages/SettingsPage";

function Content({ page, onNavigate }: { page: PageId; onNavigate: (p: PageId) => void }) {
  const { connection } = useApp();

  // Device and Settings stay reachable while disconnected — Device is where
  // you reconnect from, and Settings is where you switch device source, so
  // gating either behind a connection would strand the user.
  const online = connection === "connected";
  const alwaysAvailable = page === "settings" || page === "device";

  if (!online && !alwaysAvailable) {
    return <ConnectionScreen state={connection} />;
  }

  switch (page) {
    case "dashboard": return <DashboardPage onNavigate={onNavigate} />;
    case "device": return <DevicePage onNavigate={onNavigate} />;
    case "noise": return <NoisePage />;
    case "equalizer": return <EqualizerPage />;
    case "profiles": return <ProfilesPage />;
    case "diagnostics": return <DiagnosticsPage />;
    case "settings": return <SettingsPage />;
  }
}

/** Whether the window is phone-width: one column, navigation at the bottom. */
function useNarrow(): boolean {
  const query = "(max-width: 767px)";
  const [narrow, setNarrow] = useState(
    () => typeof window !== "undefined" && !!window.matchMedia && window.matchMedia(query).matches,
  );
  useEffect(() => {
    if (!window.matchMedia) return;
    const mq = window.matchMedia(query);
    const update = () => setNarrow(mq.matches);
    update();
    mq.addEventListener("change", update);
    return () => mq.removeEventListener("change", update);
  }, []);
  return narrow;
}

function Shell() {
  const [page, setPage] = useState<PageId>("dashboard");
  const { connection, simulated } = useApp();

  return (
    <div className="relative flex h-dvh w-full overflow-hidden bg-background text-foreground">
      {/* ambient drifting gradient blobs */}
      <motion.div
        aria-hidden
        className="pointer-events-none absolute -left-40 -top-40 size-96 rounded-full blur-[120px]"
        style={{ background: "radial-gradient(circle, var(--primary) 0%, transparent 70%)", opacity: 0.12 }}
        animate={{ x: [0, 120, 0], y: [0, 80, 0], scale: [1, 1.2, 1] }}
        transition={{ duration: 22, repeat: Infinity, ease: "easeInOut" }}
      />
      <motion.div
        aria-hidden
        className="pointer-events-none absolute -bottom-40 right-0 size-[28rem] rounded-full blur-[130px]"
        style={{ background: "radial-gradient(circle, var(--chart-4) 0%, transparent 70%)", opacity: 0.1 }}
        animate={{ x: [0, -100, 0], y: [0, -60, 0], scale: [1.1, 1, 1.1] }}
        transition={{ duration: 26, repeat: Infinity, ease: "easeInOut" }}
      />
      <Sidebar page={page} onNavigate={setPage} />
      <div className="flex min-w-0 flex-1 flex-col">
        <TopBar page={page} />
        {/* Keyed off the device source, not the connection state: a simulated
            device can be connected, disconnected or reconnecting, and the
            warning must be visible in all of those cases. */}
        {simulated && <SimulatedBanner />}
        <main className="flex-1 overflow-auto">
          <AnimatePresence mode="wait">
            <motion.div
              key={page + connection}
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={{ duration: 0.18 }}
              className="min-h-full p-4 md:p-8"
            >
              <Content page={page} onNavigate={setPage} />
            </motion.div>
          </AnimatePresence>
        </main>
        <BottomNav page={page} onNavigate={setPage} />
      </div>
    </div>
  );
}

export default function App() {
  // On a phone the bottom edge belongs to the navigation bar.
  const narrow = useNarrow();
  return (
    <AppProvider>
      <Shell />
      <Toaster position={narrow ? "top-center" : "bottom-right"} richColors closeButton />
    </AppProvider>
  );
}
