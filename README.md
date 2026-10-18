# Void

A fully local control center for Bose headphones and earbuds, for Windows and
Android. No cloud services, no accounts, no telemetry, no network requests.
Called Bose QC Control Center until 2026-10-16.

> **Current state: noise control works on real hardware.** On a Bose
> QuietComfort Headphones (product `0x4075`, firmware `1.0.6`), battery,
> device identity, connection state, Windows volume, firmware version and
> noise control are **verified** against the physical device. The application
> switches between Quiet, Aware and the headphones' custom mode, and adjusts
> that custom mode's noise-cancellation level with a slider. It also shows
> the headphones' three-band hardware EQ and changes it. Every change is
> confirmed by reading it back from the headphones. Everything else behind
> the vendor protocol is not driven and remains `UNKNOWN`. See [docs/protocol-notes.md](docs/protocol-notes.md) for the
> experiment log, including what earlier attempts got wrong.
> Nothing is presented as working that has not been demonstrated.

## The guiding rule

> Build what can be proven. Simulate only what is explicitly simulated. Never pretend.

Every hardware-facing feature carries a capability status — `unknown`,
`supported`, `ready`, `verified` or `unsupported` — and the UI renders
itself from that model. A control is never shown as functional because a
component exists to draw it.

`verified` means one thing only: the physical device was tested and demonstrated
the behaviour. It is unreachable in code without a `HardwareProof`, which can
only be constructed from an observed *change* in device-reported state. A moved
slider, an accepted command, or a changed local variable cannot produce one.

## Requirements

- Windows 10 1809+ or Windows 11
- WebView2 Runtime (ships with Windows 11)
- Node.js 20+
- Rust stable with the MSVC toolchain
- VS Build Tools with the C++ workload and a Windows SDK

## Getting started

Install dependencies:

```bash
npm install
```

Run the desktop application:

```bash
npm run tauri:dev
```

Void opens by asking **Headphones or Earbuds?** It then looks at the Bose
devices paired with the computer, finds the one of that kind that is
connected, works out which model it is from the product id it was paired
with, and opens on it — showing only what that model has. With more than one
connected it asks which; with none it says so, lists the paired ones, and
points to a device of the other kind if one is connected. **Change device**
(at the foot of the sidebar, and on the Device page) asks again. Looking
sends nothing to any device.

If no Bose device is paired at all, the application can be tried on a
simulated device instead, labelled as such.

### Models

Every Bose headphone and earbud model in the catalogue is recognized (the
catalogue is upstream's, in `vendor/bose-bmap`). What can be done with one
depends on how much is known about it, and the Device page says which of
these it is:

| | Models | What Void does |
| --- | --- | --- |
| **Tested** | QuietComfort Headphones (`0x4075`) | Everything in the capability matrix, each proven on the physical headphones |
| **Documented** | QuietComfort Ultra Headphones (2nd Gen), QuietComfort Headphones (2nd Gen), QuietComfort Ultra Earbuds (2nd Gen), QuietComfort 45, QuietComfort Earbuds, Ultra Open Earbuds | Follows upstream's description of the model. Never run here. A custom mode's level is written only where its layout was checked on the model: the QC Ultra 2nd Gen headphones and earbuds. The QuietComfort Headphones (2nd Gen) are given the QC Ultra's description upstream, marked unconfirmed, so their modes are read and switched but never written into |
| **Recognized** | Every other headphone and earbud model — QuietComfort Ultra (1st Gen), QuietComfort Ultra Earbuds (1st Gen), QuietComfort Earbuds II, 700, Sport Earbuds and the rest | Asks the questions every documented model answers the same way — name, equalizer, multipoint, its list of modes — and offers what it answers. Nothing is written into its modes |
| Documented, by level | QuietComfort 35 and 35 II | Their noise cancellation is a level, not a mode: Void offers the levels the headphones list — High, Low, Off — and sets one by upstream's description of the QC35, confirming it by reading it back. No equalizer, Aware mode, custom level or profiles: they have none. The QC35 II has been used by the owner, on the phone |

Whatever the model, a change counts only when the device reports it, so a
model that does not behave as described shows a change as unconfirmed, never
as made. A feature is not shown at all when the model has none, or when the
device answers that it has no such function. The modes are the device's own
list — a QC Ultra's Immersion and Cinema included — under its own names.

**On connection, every device is asked rather than assumed:**

- A device that stays silent at first is asked for its protocol version and
  then again (the QC35 generation needs this; any other model that does is
  handled the same way).
- Its noise control is found out: modes (`[31.3]`); if it answers it has
  none, a noise-cancellation level (`[1.6]`, as the QC35); if it has neither
  — sport earbuds, Frames — no noise control is shown, and everything else
  still is.
- Its name, equalizer, multi-device switch and paired devices are each shown
  if it answers for them in the documented shape, and hidden if it answers
  that it has no such function.
- A Bose product the catalogue does not have — a model newer than it — is
  connected the same cautious way as a recognized one, under the name it
  gives itself.

**Earbuds only**, on the Device page:

- **Touch controls** — what a touch shortcut does (`[1.9]`), offered only
  from the actions the earbuds themselves list for it.
- **In-ear detection** — pause when taken out (`[1.24]`) and answer calls
  when put in (`[1.27]`), each shown only where the earbuds answer for it.
- **Find my buds** — off until switched on. Notes when the earbuds were last
  connected, and on the phone where the phone was then (as Android last knew
  it, with the location permission), and opens that place in the phone's
  maps application. Kept on the device; nothing sent anywhere.

None of these has been run on any earbuds. The ear-tip fit test is not
offered: no function for it is documented, and finding one would mean
sending undocumented requests to the earbuds.

**Per model**, as the application's own rules give it (`cargo test --lib
print_model_table -- --ignored --nocapture` prints this table). "Described":
upstream's description of the model; "Recognized": known to be the model,
nothing described, so asked on connection. Multi-device, the name and the
paired devices are asked on every model and shown if it answers.

| Model | Product id | Kind | Tried | Noise control | Custom mode's level | Equalizer | Battery |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Hearphones | `0x4015` | Headphones | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level |
| Hearphones II | `0x402B` | Headphones | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level |
| Noise Cancelling Headphones 700 | `0x4024` | Headphones | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level |
| ProFlight | `0x4021` | Headphones | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level |
| QuietComfort 35 | `0x400C` | Headphones | Described | Levels: High, Low, Off | No | No | Level |
| QuietComfort 35 II | `0x4020` | Headphones | Described | Levels: High, Low, Off | No | No | Level |
| QuietComfort 45 | `0x4039` | Headphones | Described | Modes: Quiet, Aware, your custom modes | No | Yes | Level |
| QuietComfort Headphones | `0x4075` | Headphones | Tested | Modes: Quiet, Aware, your custom modes | Yes | Yes | Level |
| QuietComfort Headphones (2nd Gen) | `0x4083` | Headphones | Described | Modes: Quiet, Aware, Immersion, Cinema, your custom modes | No | Yes | Level |
| QuietComfort Ultra Headphones | `0x4066` | Headphones | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level |
| QuietComfort Ultra Headphones (2nd Gen) | `0x4082` | Headphones | Described | Modes: Quiet, Aware, Immersion, Cinema, your custom modes | Yes | Yes | Level |
| Frames | `0x402C` | Earbuds | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level, and Right, Left, Case if it reports them |
| Frames | `0x4061` | Earbuds | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level, and Right, Left, Case if it reports them |
| Frames (2nd Gen) | `0x404C` | Earbuds | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level, and Right, Left, Case if it reports them |
| Frames Tempo | `0x4060` | Earbuds | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level, and Right, Left, Case if it reports them |
| QuietComfort Earbuds | `0x402F` | Earbuds | Described | Modes: Quiet, Aware | No | Yes | Level, and Right, Left, Case if it reports them |
| QuietComfort Earbuds II | `0x4064` | Earbuds | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level, and Right, Left, Case if it reports them |
| QuietComfort Ultra Earbuds | `0x4072` | Earbuds | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level, and Right, Left, Case if it reports them |
| QuietComfort Ultra Earbuds (2nd Gen) | `0x4062` | Earbuds | Described | Modes: Quiet, Aware, Immersion, Cinema, your custom modes | Yes | Yes | Level, and Right, Left, Case |
| QuietControl 30 | `0x4014` | Earbuds | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level, and Right, Left, Case if it reports them |
| SoundSport | `0x4012` | Earbuds | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level, and Right, Left, Case if it reports them |
| SoundSport Free | `0x4018` | Earbuds | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level, and Right, Left, Case if it reports them |
| SoundSport Pulse | `0x4013` | Earbuds | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level, and Right, Left, Case if it reports them |
| Sport Earbuds | `0x402D` | Earbuds | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level, and Right, Left, Case if it reports them |
| Sport Open Earbuds | `0x403A` | Earbuds | Recognized | Asked on connection: modes, levels, or none | No | If it answers | Level, and Right, Left, Case if it reports them |
| Ultra Open Earbuds | `0x4068` | Earbuds | Described | Modes, from its own list | No | Yes | Level, and Right, Left, Case if it reports them |

The Settings page controls starting with Windows, starting minimized, a
system-tray icon, whether the application attaches to the headphones at launch
and when they return, and notifications for connecting, disconnecting and low
battery. "Start with Windows" needs an installed build (`npm run tauri:build`).

Build installers with `npm run tauri:build` rather than a bare `tauri build`:
the script adds `tauri.bundle.conf.json`, which packages the two speech-runtime
DLLs the executable cannot start without.

Every build ends by copying what it made to **`Setups\`** at the top of the
project — the Windows installer and the Android phone package, each under one
name that the next build replaces, with a `README.txt` saying when each was
built. That folder is the place to look for something to install; it is not
part of the repository. `npm run setups` does the copying by itself.

### Now playing

What is playing on the computer — any application that reports to Windows'
media sessions, the same ones the volume flyout shows: a music app, a browser
tab, a video player — with its artwork, the application, what it is playing
on (the headphones' own name when they are the output), a position bar that
moves smoothly and can be dragged to seek, and the controls the application
allows: previous, play/pause, next, and shuffle and repeat where it has them.
On the dashboard, and as a compact row on the desktop widget. Read once a
second while on screen; a control shows as done only when the application
reports it.

**Playing on another device.** When the headphones are playing from another
device they are connected to — a phone, say — the card shows that device by
the name the headphones give it ("Jaisal's S24 Ultra"), whether it is
playing or paused, and previous, play/pause and next, passed on to it
through the headphones. The phone application shows the same for the
computer. The song's name, artwork and position are not shown for another
device: the headphones keep the track for the Bose app's signed-in
connection, and Void does not get around that (protocol-notes,
Experiment 17). Play and pause show as done when the headphones then report
it; a skip cannot be confirmed.

### Desktop widget (optional)

A small window of its own with the device's noise modes and the three
equalizer bands, so that the main window need not be opened to change them. Switch it
on in Settings → General → **Desktop widget**, or from the tray icon's menu.

- Drag it by its header. It comes back where it was left.
- The pin keeps it above other windows, or lets them cover it.
- The arrows open the main window; the cross hides the widget.
- With the widget showing, closing the main window leaves the widget running.
  With **Start with Windows** and **Start minimized** on as well, signing in
  brings up the widget alone.

It is part of the same application — it has to be, because the headphones
accept one control connection from a computer — so the application must be
running for it to be there. Everything on it shows what the headphones
report, and a change counts only when they confirm it, exactly as in the main
window.

### Multi-device connection and renaming

On the Device page, for both the desktop and the phone:

- **Multi-device connection** — whether the headphones stay connected to two
  devices at once. Switching it off is asked about first: the headphones then
  keep one connection and drop the other, and they choose which. On the
  tested pair it was the device that had asked that was dropped.
- **Devices** — under it, each device the headphones are paired with, by
  name, with a switch that is on while they are connected to it. Switching
  one off asks the headphones to disconnect from that device; switching it on
  asks them to connect. **This one has not yet been run against the
  headphones** (see [protocol-notes.md](docs/protocol-notes.md), Experiment
  13): the switch moves only when the headphones report the device
  differently, so it cannot show a change that did not happen, but whether
  this model takes the request at all is not known.
- **Device name** — the name shown is the one the headphones themselves
  report, whatever they are called, and the pencil beside it renames them (up
  to 31 bytes). The new name is shown once they report it back. Other devices
  pick it up when they next connect.

### Home-screen widget (Android)

The phone build has a widget for the home screen with the three noise modes
— **Quiet**, **Aware**, and the custom mode under the name the headphones
give it — and under them the equalizer: bass, mid and treble, each with its
level and a step down and a step up. Long-press the home screen →
**Widgets** → **Void** → **Modes and equalizer**. It comes
two rows tall; made one row tall it is the modes alone.

- With **Keep running in the background** on (Settings → General), a tap
  switches the mode there and then, without the application coming up.
- With it off, or with the application closed, the widget says "Tap to open
  the app", and a tap on a mode opens the application and asks it for that
  mode. A tap on an equalizer step opens the application and changes
  nothing.

It highlights the mode and shows the levels the headphones report, never
what was tapped, and shows neither when the application is not there to
vouch for them.
Details: [docs/android.md](docs/android.md#the-home-screen-widget).

### Voice feedback (optional)

The application can speak confirmed changes — "Quiet mode activated.",
"Aurora connected.", a low-battery warning — through the headphones, using the
Kokoro text-to-speech model running on this computer. It is off until switched
on, and it needs the voice model, which is installed once by an explicit step
(a 98 MB download; nothing is ever fetched by the application itself):

```powershell
powershell -ExecutionPolicy Bypass -File tools\install-voice-model.ps1
```

Then open Settings → Voice Feedback. See
[docs/voice-feedback.md](docs/voice-feedback.md) for what is announced and
when, where speech is played, and what has been measured.

### Android

The same application builds for an Android phone, with the voice model inside
the package:

```powershell
powershell -ExecutionPolicy Bypass -File tools\build-android.ps1
```

The interface and all of the control logic are shared; a small Kotlin bridge
stands in for the Windows APIs. What is different on a phone, how to install
the result, and what has and has not been verified there are in
[docs/android.md](docs/android.md).

Run the frontend alone in a browser (simulated device, no native layer):

```bash
npm run dev
```

## Tests

```bash
npm run rust:test
```

```bash
npm run test:run
```

The vendored protocol crate has its own suite:

```bash
cargo test --manifest-path vendor/bose-bmap/Cargo.toml
```

### Hardware tests

These talk to real headphones and are skipped by default. Pair and connect a
supported model first, and run them one at a time — the headphones accept one
control connection.

Read-only (endpoint validation, mode read, mode list, reconnection):

```bash
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 hw_a hw_b0 hw_b2 hw_b3 hw_b4
```

Switches the noise-control mode and switches it back:

```bash
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 hw_b1
```

Times a switch into the custom mode against a switch between the presets
(switches the mode six times and restores it):

```bash
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 hw_b8
```

Changes one equalizer band by one step and restores it:

```bash
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 hw_b7
```

Switches mode by slot from the headphones' own list, the way every model's
modes are switched, and back; checks the model is named (`hw_b13` only lists
what the computer has paired, and sends nothing):

```bash
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 hw_b12 hw_b13
```

Switches to the custom mode, changes its level, and restores both (needs a
custom mode set up on the headphones):

```bash
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 hw_b5
```

With the headphones switched off (the one test that needs them *not*
connected):

```bash
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 hw_c1
```

Voice feedback has real-model and real-headphones tests of its own, listed in
[docs/voice-feedback.md](docs/voice-feedback.md#tests).

## Project layout

```
app/
  frontend/       React + TypeScript + Tailwind UI
  src-tauri/      Rust native layer
    autostart.rs  "Start with Windows" (the user's startup list)
    settings.rs   Settings file
    tray.rs       System-tray icon
    widget.rs     The desktop widget's window
    watcher.rs    Background presence watcher and notifications
    host.rs       What this computer or phone is called ("connected to ...")
    voice/        Voice feedback: announcement rules, queue, Kokoro, worker,
                  and the sentences already synthesized, kept on disk
    android/      Android's Bluetooth, device list and audio, via the Kotlin bridge
    gen/android/  The Android project (Kotlin bridge, background service, manifest)
    audio/        Windows audio: endpoints, and playback of synthesized speech
    bluetooth/    Windows PnP + radio access (read-only)
    bose/         Mock and real device backends
      bmap/       Windows transport and verification for the control protocol
    device/       Capability engine, typed commands, state
vendor/
  bose-bmap/          Bose BMAP protocol implementation, vendored (MIT)
tools/
  bose-discovery/     Read-only device/GATT discovery, exports a shareable report
  bose-rfcomm-listen/ Listen-only RFCOMM capture — never transmits
  bose-btsnoop-parse/ Offline parser for Android Bluetooth HCI snoop logs
  install-voice-model.ps1  One-time download of the Kokoro voice model
  build-android.ps1        Builds the Android APK, voice model included
  collect-setups.ps1       Copies the latest installer and APK to Setups\
docs/                 Architecture, capability matrix, protocol notes
```

None of the tools transmits. The only code that writes to the headphones is the
application's control link, and what it can send is described in
[docs/security.md](docs/security.md).

## Third-party code

The Bose BMAP protocol implementation in `vendor/bose-bmap` is the `bose-bmap`
crate from [aaronsb/bosectl](https://github.com/aaronsb/bosectl), Copyright (c)
2026 Aaron Bockelie, used under the MIT licence. Its licence file is kept
alongside it, the exact upstream commit and the one build-configuration patch
are recorded in `vendor/bose-bmap/VENDORED.md`, and the notice is reproduced in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

Voice feedback uses sherpa-onnx (Apache-2.0), ONNX Runtime (MIT) and the Kokoro
model (Apache-2.0), none of which is copied into this repository. The
sherpa-onnx library contains eSpeak NG, which is GPL-3.0-or-later; what that
means for anyone distributing a build is set out in the notices file.

## Security posture

The frontend has no Bluetooth surface. It cannot express an arbitrary write:
there is no command variant carrying a UUID, characteristic handle, or byte
array, and a test asserts that the serialized command surface contains none.
All hardware operations pass through a typed, validated, allowlisted native
layer. Noise control is requested by mode name — `quiet`, `aware` or `custom` —
and a level from 0 to 10; the request types accept nothing else, and "custom"
carries no slot number.

Bluetooth addresses never leave the native layer. Anything user-visible or
exportable carries a per-installation salted hash instead, so a screenshot or
a shared log cannot be traced back to specific hardware.

See [docs/security.md](docs/security.md).

## Documentation

| Document | Contents |
| --- | --- |
| [environment.md](docs/environment.md) | Measured state of the development machine |
| [architecture.md](docs/architecture.md) | Layering and design decisions |
| [capability-matrix.md](docs/capability-matrix.md) | What actually works, and what does not |
| [protocol-notes.md](docs/protocol-notes.md) | Vendor protocol investigation status |
| [security.md](docs/security.md) | Threat model and constraints |
| [voice-feedback.md](docs/voice-feedback.md) | Spoken announcements: setup, rules, runtime, measurements |
| [android.md](docs/android.md) | The Android build: what differs, building, installing, what is verified |
| [troubleshooting.md](docs/troubleshooting.md) | Common problems |
