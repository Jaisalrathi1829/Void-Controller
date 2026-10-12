import { useEffect, useRef, useState } from "react";
import { Toaster } from "./components/ui/sonner";
import { AppProvider, useApp, type ConnectionState } from "./store";
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

/**
 * Whether `page` is shown as itself, rather than replaced by the connection
 * screen. Device and Settings stay reachable while disconnected — Device is
 * where you reconnect from, and Settings is where you switch device source,
 * so gating either behind a connection would strand the user.
 */
function showsPage(page: PageId, connection: ConnectionState): boolean {
  return connection === "connected" || page === "settings" || page === "device";
}

/**
 * Names the screen `Content` puts up. It changes when what is on screen is
 * replaced by something else — another page, or the connection screen in
 * another of its states — and at no other time. In particular it does not
 * change when the connection state changes underneath a page that goes on
 * being shown, which is what used to rebuild the whole dashboard, restart its
 * animation from the beginning, and now and then leave it blank.
 */
export function screenKey(page: PageId, connection: ConnectionState): string {
  return showsPage(page, connection) ? page : `offline:${connection}`;
}

function Content({ page, onNavigate }: { page: PageId; onNavigate: (p: PageId) => void }) {
  const { connection } = useApp();

  if (!showsPage(page, connection)) {
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
  const screen = screenKey(page, connection);

  // A new screen starts at its top, not wherever the last one was left.
  const main = useRef<HTMLElement>(null);
  useEffect(() => {
    if (main.current) main.current.scrollTop = 0;
  }, [screen]);

  return (
    <div className="relative flex h-dvh w-full overflow-hidden bg-background text-foreground">
      {/* Ambient drifting glows. Each is a gradient that already fades to
          nothing, drifting as a layer: no blur filter. Blurring a layer a
          third of the screen wide by more than a hundred pixels, on every
          frame, behind every page, was the most expensive thing on screen —
          and on a phone it showed, as stutter everywhere else. */}
      <div
        aria-hidden
        className="ambient-glow ambient-glow-a pointer-events-none absolute -left-72 -top-72 size-[44rem] rounded-full"
        style={{
          background:
            "radial-gradient(circle, color-mix(in oklch, var(--primary) 13%, transparent) 0%, color-mix(in oklch, var(--primary) 6%, transparent) 35%, transparent 68%)",
        }}
      />
      <div
        aria-hidden
        className="ambient-glow ambient-glow-b pointer-events-none absolute -bottom-72 -right-40 size-[48rem] rounded-full"
        style={{
          background:
            "radial-gradient(circle, color-mix(in oklch, var(--chart-4) 11%, transparent) 0%, color-mix(in oklch, var(--chart-4) 5%, transparent) 35%, transparent 68%)",
        }}
      />
      <Sidebar page={page} onNavigate={setPage} />
      <div className="flex min-w-0 flex-1 flex-col">
        <TopBar page={page} />
        {/* Keyed off the device source, not the connection state: a simulated
            device can be connected, disconnected or reconnecting, and the
            warning must be visible in all of those cases. */}
        {simulated && <SimulatedBanner />}
        <main ref={main} className="flex-1 overflow-auto">
          {/* A new screen is a new element, and fades in by a CSS animation
              that starts when it appears. It is not held back for the old
              screen to fade out first: that arrangement waited on script to
              finish one animation before starting the next, and when the
              screen changed again in the meantime the new one could be left
              waiting at fully transparent — a blank page. */}
          <div key={screen} className="page-enter min-h-full p-4 md:p-8">
            <Content page={page} onNavigate={setPage} />
          </div>
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
