export type PageId =
  | "dashboard"
  | "device"
  | "noise"
  | "equalizer"
  | "profiles"
  | "settings";

export const pageTitles: Record<PageId, { title: string; subtitle: string }> = {
  dashboard: { title: "Dashboard", subtitle: "Overview and quick controls" },
  device: { title: "Device", subtitle: "Connection and hardware details" },
  noise: { title: "Noise Control", subtitle: "Manage ambient noise and awareness" },
  equalizer: { title: "Equalizer", subtitle: "Bass, mid and treble on the headphones" },
  profiles: { title: "Profiles", subtitle: "Saved control presets" },
  settings: { title: "Settings", subtitle: "Application preferences" },
};
