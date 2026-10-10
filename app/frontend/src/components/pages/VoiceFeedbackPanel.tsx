import { Play, RefreshCw, Square } from "lucide-react";
import { Button, Panel, Range, SectionLabel, Toggle } from "../primitives";
import { useApp } from "../../store";
import { VOICE_SPEED, type SettingKey, type VoiceState } from "@/services/ipc";
import { cn } from "../ui/utils";

const stateMeta: Record<VoiceState, { label: string; tone: string }> = {
  "not-installed": { label: "Not installed", tone: "bg-warning-subtle text-warning" },
  initializing: { label: "Initializing", tone: "bg-info-subtle text-info" },
  ready: { label: "Ready", tone: "bg-success-subtle text-success" },
  synthesizing: { label: "Synthesizing", tone: "bg-info-subtle text-info" },
  playing: { label: "Playing", tone: "bg-info-subtle text-info" },
  unavailable: { label: "Unavailable", tone: "bg-muted text-muted-foreground" },
  error: { label: "Error", tone: "bg-warning-subtle text-warning" },
};

/** The command that fetches the voice model. Shown, never run from here. */
const INSTALL_COMMAND = "powershell -ExecutionPolicy Bypass -File tools\\install-voice-model.ps1";

function Row({
  title,
  desc,
  unavailable,
  children,
}: {
  title: string;
  desc?: React.ReactNode;
  unavailable?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-4 border-b border-border py-3.5 last:border-0">
      <div className={unavailable ? "opacity-60" : undefined}>
        <div className="text-sm">{title}</div>
        {desc && <div className="text-xs text-muted-foreground">{desc}</div>}
        {unavailable && <div className="mt-0.5 text-xs text-warning">{unavailable}</div>}
      </div>
      {children}
    </div>
  );
}

/**
 * Voice feedback: spoken announcements, synthesized on this computer.
 *
 * Everything shown here is what the native layer reports. The status is the
 * speech worker's own account of what it is doing, the voices are those of
 * the model it found installed, and a control that cannot work is disabled
 * with the reason rather than left to do nothing.
 */
export function VoiceFeedbackPanel() {
  const {
    settings,
    toggleSetting,
    voice,
    setVoiceOption,
    testVoice,
    stopSpeaking,
    recheckVoice,
  } = useApp();

  const meta = stateMeta[voice.state];
  // The headphones are announced by their own name. Until one is known,
  // the sentence is described rather than made up.
  const name = settings.preferredDeviceName;
  // The reason the whole feature is out of reach — loading, or a browser tab.
  const blocked = settings.unavailable.voiceEnabled;
  const installed = voice.voices.length > 0;
  const canSpeak = !blocked && installed;
  const busy =
    voice.state === "initializing" || voice.state === "synthesizing" || voice.state === "playing";
  // The kinds of announcement only matter while voice feedback is on.
  const kindsOff = !settings.voiceEnabled;

  const toggle = (key: SettingKey, title: string, disabled = false) => (
    <Toggle
      label={title}
      disabled={!!settings.unavailable[key] || disabled}
      checked={settings[key]}
      onChange={() => toggleSetting(key)}
    />
  );

  return (
    <Panel className="p-6">
      <div className="flex items-center justify-between">
        <SectionLabel>Voice Feedback</SectionLabel>
        <span
          data-testid="voice-state"
          className={cn("rounded-md px-2 py-0.5 text-xs font-medium", meta.tone)}
        >
          {meta.label}
        </span>
      </div>

      <div className="mt-2">
        <Row
          title="Voice feedback"
          desc="Speak confirmed changes aloud. Synthesized on this computer; nothing is sent anywhere."
          unavailable={blocked}
        >
          {toggle("voiceEnabled", "Voice feedback")}
        </Row>
      </div>

      <div className="mt-3 rounded-lg border border-border bg-surface-2/40 p-3" aria-live="polite">
        <div className="text-sm" data-testid="voice-detail">
          {voice.detail}
        </div>
        {voice.speaking && (
          <div className="mt-1 text-xs text-muted-foreground" data-testid="voice-speaking">
            {voice.isTest ? "Test: " : "Announcement: "}
            <span className="text-foreground">“{voice.speaking}”</span>
          </div>
        )}
        {voice.state === "not-installed" && (
          <div className="mt-3 space-y-2 text-xs text-muted-foreground" data-testid="voice-install">
            <p>
              The voice model is a one-time download of about 98 MB and is not fetched
              automatically. From the project folder, run:
            </p>
            <code className="block select-all rounded-md bg-muted px-2 py-1.5 font-mono text-foreground">
              {INSTALL_COMMAND}
            </code>
            {voice.assetsDir && (
              <p>
                It is installed to <span className="break-all text-foreground">{voice.assetsDir}</span>
              </p>
            )}
            <Button
              variant="outline"
              size="sm"
              icon={<RefreshCw className="size-4" />}
              onClick={recheckVoice}
            >
              Check again
            </Button>
          </div>
        )}
      </div>

      <div className="mt-4 space-y-4">
        <div>
          <label htmlFor="voice-select" className="mb-2 block text-sm">
            Voice
          </label>
          <select
            id="voice-select"
            className="w-full rounded-md border border-border bg-card px-3 py-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
            disabled={!canSpeak}
            // Only voices the installed model has are offered; with no model
            // there are none to offer.
            value={installed ? settings.voiceId : ""}
            onChange={(e) => setVoiceOption({ kind: "voice", id: e.target.value })}
          >
            {!installed && <option value="">No voice model installed</option>}
            {voice.voices.map((v) => (
              <option key={v.id} value={v.id}>
                {v.label}
              </option>
            ))}
          </select>
        </div>

        <Range
          label="Speed"
          value={settings.voiceSpeedPercent}
          min={VOICE_SPEED.min}
          max={VOICE_SPEED.max}
          step={5}
          leftLabel="0.5×"
          rightLabel="2×"
          displayValue={`${(settings.voiceSpeedPercent / 100).toFixed(2)}×`}
          suffix="%"
          disabled={!canSpeak}
          onChange={(percent) => setVoiceOption({ kind: "speed", percent })}
        />

        <div>
          <Range
            label="Speech volume"
            value={settings.voiceVolumePercent}
            min={0}
            max={100}
            step={5}
            leftLabel="0"
            rightLabel="100"
            suffix="%"
            disabled={!canSpeak}
            onChange={(percent) => setVoiceOption({ kind: "volume", percent })}
          />
          <p className="mt-2 text-xs text-muted-foreground">
            Applies to speech only. The Windows and headphone volume are left alone.
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            icon={<Play className="size-4" />}
            disabled={!canSpeak}
            onClick={testVoice}
          >
            Test Voice
          </Button>
          <Button
            variant="ghost"
            size="sm"
            icon={<Square className="size-4" />}
            disabled={blocked !== undefined || !busy}
            onClick={stopSpeaking}
          >
            Stop Speaking
          </Button>
          {voice.output && (
            <span className="text-xs text-muted-foreground" data-testid="voice-output">
              Last spoken through {voice.output}
            </span>
          )}
        </div>
      </div>

      <div className="mt-5">
        <SectionLabel>Announce</SectionLabel>
        <div className="mt-2">
          <Row
            title="Quiet / Aware mode changes"
            desc="“Quiet mode activated.” or “Aware mode activated.”, once the headphones report the new mode"
            unavailable={settings.unavailable.announceModeChanges}
          >
            {toggle("announceModeChanges", "Quiet / Aware mode changes", kindsOff)}
          </Row>
          <Row
            title="Connection and disconnection"
            desc={
              name ? (
                <>
                  “{name} connected.” through the headphones. “{name} disconnected.” cannot be
                  spoken into headphones that have gone, so it needs the other output below.
                </>
              ) : (
                <>
                  The headphones’ name, then “connected.” or “disconnected.” The second cannot be
                  spoken into headphones that have gone, so it needs the other output below.
                </>
              )
            }
            unavailable={settings.unavailable.announceConnection}
          >
            {toggle("announceConnection", "Connection and disconnection", kindsOff)}
          </Row>
          <Row
            title="Low battery"
            desc="Once, when the level Windows reports falls to 20% or below"
            unavailable={settings.unavailable.announceBattery}
          >
            {toggle("announceBattery", "Low battery warning", kindsOff)}
          </Row>
          <Row
            title="Use another output when the headphones are unavailable"
            desc="Off: stay silent rather than speak through the computer’s speakers. On: use the Windows default output"
            unavailable={settings.unavailable.voiceUseDefaultOutput}
          >
            {toggle("voiceUseDefaultOutput", "Use another output when the headphones are unavailable")}
          </Row>
        </div>
      </div>
    </Panel>
  );
}
