# Architecture

## Layering

```
              React + TypeScript UI
                       │
            renders from capability model
                       │
                       ▼
              Typed command (no bytes)
                       │
                       ▼
        ┌──────── Tauri IPC boundary ────────┐
                       │
                  validate()
                       │
                       ▼
                  BoseDevice trait
                  /             \
                 ▼               ▼
          MockBoseDevice    RealBoseDevice
                             /          \
                            ▼            ▼
              Windows PnP / CfgMgr32    BMAP control link
              identity, connection,     noise-control mode
              battery (read-only)       and custom level
                                             │
                                             ▼
                                  RFCOMM socket (Winsock)
```

The frontend never touches Bluetooth. It cannot: no command variant carries a
UUID, handle or payload, so an arbitrary write is not expressible in the type
system rather than merely discouraged.

## Key decisions

### Capability status is enforced, not documented

`CapabilityStatus::Verified` is unreachable without a `HardwareProof`. That type
has private fields and three constructors:

* `HardwareProof::observed(key, before, after, expected)` — returns `None` if
  `before == after`, or if `after != expected`. So "we set a local variable and
  read it back" and "the device changed to something we did not ask for" both
  fail to produce proof.
* `HardwareProof::observed_at(…, observed_at)` — the same rules, for a change
  the device confirmed at a known earlier time. Capability sets are rebuilt on
  every snapshot; without this, old evidence would be re-stamped with the
  current time and read as a fresh test.
* `HardwareProof::observed_passively(key, detail)` — for read-only capabilities
  where the device volunteered a value.

The mock backend cannot construct one at all, which is why it reports
`Experimental` and never `Verified`.

### Commands return outcomes, not unit

`set_noise_control` returns `CommandOutcome`, not `()`. A backend that cannot
confirm a state change is required by the signature to say
`SentUnverified { reason }`, which the UI renders as *"Command sent. State could
not be verified."* — deliberately not styled as success.

### The control link is layered so each claim has one owner

Noise control runs over Bose's BMAP protocol on an RFCOMM channel. The protocol
itself — packet codec, operators, error codes, per-model addresses — is not
implemented in this repository. It is the `bose-bmap` crate from
[aaronsb/bosectl](https://github.com/aaronsb/bosectl), vendored under
`vendor/bose-bmap` with a build-configuration-only patch so it compiles on
Windows. What this project adds sits in `bose/bmap/`:

| Module | Owns |
| --- | --- |
| `winsock` | An RFCOMM byte stream. The only platform-specific code. |
| `stream` | Reassembling frames from that stream: a read may hold part of a frame or several. Implements upstream's `Transport`. |
| `endpoint` | Which channel to use, and proof that it speaks BMAP before it is used. |
| `noise` | A mode change as three separate facts: sent, acknowledged, confirmed by read-back. |
| `custom` | The user-configurable modes: listing them, and changing the active one's level without touching anything else in it. |
| `eq` | The three-band hardware equalizer: a strict read, and a write confirmed by read-back. |
| `link` | One connection at a time; reconnect, backoff, and the last state the device confirmed. |

Three rules hold across them:

* **A successful write proves nothing.** Neither does an open socket, nor an
  acknowledgement. A route is trusted only after the device answers a
  read-only firmware request correctly; a mode change is reported as applied
  only when the device then reports that mode.
* **Nothing is sent blind.** The mode is read before any change, and a reply
  that does not match the documented framing leaves the session read-only.
* **A write is never repeated.** A read may be retried on a fresh connection.
  A mode change that hits a dead channel is reported as failed-and-unknown,
  not replayed.

Only models on an explicit allowlist get a link at all. Upstream supports more
models than this application has been run against, and "upstream supports it"
is not the same claim as "tested here".

### The store always re-reads after a command

`deviceStore.run()` calls `refresh()` after every command. The UI never assumes
a command took effect and never optimistically updates device state; it re-reads
and renders what came back. The EQ sliders keep a local draft purely so dragging
feels responsive, and that draft is re-seeded from device state.

The same holds when a command fails outright: a failure says nothing about the
device's state, so it is asked. And while real headphones are connected the
snapshot is re-read every few seconds, because the mode can be changed from the
headphones' own button or from a phone.

### Why CfgMgr32 rather than WinRT for PnP

WinRT's `DeviceInformation` exposes the same properties, but requesting extra
properties needs an `IIterable<HSTRING>`, which is awkward to construct from
Rust for no benefit at this layer. CfgMgr32 is synchronous, needs no COM
apartment, and reads the properties directly. WinRT remains the right choice for
GATT and RFCOMM when that work begins.

### Mock imperfection is intentional

The mock returns `SentUnverified` on every 7th mutation and can be configured to
fail reads. Without this, the UI's caution and error paths would never execute
until they met real hardware, which is the worst time to discover they are
wrong.

### Device identifiers are hashed

`util::stable_id()` returns a salted SHA-256 prefix. The salt is per
installation, so the same headphones produce different ids on different
machines and a shared diagnostics report cannot be correlated back to hardware.
The raw instance id is deliberately not included in the snapshot handed to the
UI, so it cannot leak into a screenshot or an export.

### Settings live natively, and one of them lives in Windows

The Settings page switches are stored by the native layer in
`settings.json` under the application's config folder, not in the webview.
"Start minimized" and "minimize to tray" are acted on before any page has
loaded, and the background watcher reads the notification switches while the
window may be hidden.

"Start with Windows" is the exception: it is not stored at all. It is on
exactly when the current user's startup list has this application's entry
pointing at the running executable, and is read from there every time.

### Attached is not the same as connected

Windows decides whether the headphones are connected. Separately, the
application is either *attached* to them or not. Detached, it reports itself
disconnected and opens no control link even with the headphones right there.
That is all "auto-connect" and "auto reconnect" are: whether the application
starts attached, and whether it lets go when the headphones leave. The
application never forces a Bluetooth connection.

### A native watcher notices the headphones come and go

The window stops refreshing once the headphones are gone, so it could never
notice them return, and it may be hidden when a notification is due. A small
native task looks at Windows' device properties every few seconds — it sends
nothing to the headphones — and tells the window to refresh, lets go of the
headphones if "auto reconnect" is off, and raises notifications. Simulated
devices are not watched: a simulation raising Windows notifications would be
passing itself off as real.

The one thing it does ask the headphones is their mode, and only while spoken
mode announcements are switched on: the window stops reading the mode when it
is hidden, and a change made on the headphones with the application in the
tray would otherwise never be noticed. It is the same read the window makes.

### Speech is told what happened; it is never asked to decide

Voice feedback sits beside the device layer, not inside it. Nothing in the
device layer calls it. It is handed two things after the fact — snapshots of
the device, and what the watcher saw — and decides from those alone whether
there is anything to say. There is no entry point for "the user asked for
Quiet", so an announcement cannot run ahead of the headphones.

Everything slow happens on one worker thread of its own: loading the model,
synthesis, playback. The calls that feed it take a short lock and return, so
speech cannot hold up a command, a refresh or the Bluetooth link, and a
failure in speech is a status on the Settings page and nothing more.

The model and the audio output sit behind one trait. The tests of scheduling
and policy substitute it; the tests that claim real speech do not. See
[voice-feedback.md](voice-feedback.md).

It is handed one more thing, also after the fact: when a change of mode was
asked for, the mode the headphones reported at the end of it. That is still
their report and never the request — it is the read-back the change is
verified by — and it means the announcement does not wait for the next
refresh of the screen.

### The model is the exception, not the rule

A loaded model is a quarter of a gigabyte, which on a phone is most of the
application. So the worker is built to need it as seldom as possible: a
sentence is kept on disk the first time it is synthesized, and read back
from then on. The sentences this application says are few and fixed, so
after its first run the model is loaded only when the voice is changed. The
files are checked before they are played and thrown away if they do not
check out; nothing but the model can put speech there.

### The interface animates without script

Everything on screen that moves continuously — the headphone visual, the
glows behind each page, the pulses on a badge or a slider — is a CSS
animation of `transform` or `opacity` on a layer that is otherwise left
alone. The browser's compositor carries those by itself: nothing is redrawn
and no script runs, so they do not stutter when the page is busy re-reading
the device, and they cost nothing while the window is hidden. Script-driven
animation is kept for things that happen once, in response to something.
Pages and panels arrive by CSS too, so that none of them can be left
invisible waiting for script.

### The widget is a window, not a program

The desktop widget could have been a small program of its own. It is a
second window of this one instead, for one reason that decides it: the
headphones accept a single control connection from a computer. Two programs
would have to take turns with it or talk to each other; two windows of one
program simply share the link, the settings and the voice service that are
already there.

It loads the same page as the main window and is told apart by the name of
the window it was loaded into, so there is one interface to build and one set
of typed commands. Each window re-reads the device after a change it made;
the native layer tells the other one, and tells both when a setting changes,
so neither shows what the other has since altered.

There is always a way back to the application: closing the main window with
the widget up hides the main window, and closing the widget with the main
window hidden and no tray icon brings the main window back.

### The phone's widget decides nothing

An Android home-screen widget is not part of the application's window: the
launcher draws it from a description the application hands over, and a tap
on it arrives as a message to a receiver, possibly in a process started for
that message alone. The control link, the rule that a change counts only
when the headphones confirm it, and the voice all live in the native layer,
which exists only while the application proper is running.

So the widget is kept to two things. A tap leaves a request — the name of a
mode — where the native layer picks requests up (`WidgetBus` in
`ModeWidget.kt`; one thread in `android/widget.rs` waits on it). And the
widget shows what the native layer last told it the headphones report. It
never marks the tapped mode as the current one.

That the native layer is waiting for requests is also how the widget knows
there is one. If nothing is — the application is closed, or is off screen
without the service that keeps it running, where Android stops it mid-step —
the widget shows no mode at all, says so, and a tap opens the application
with the request attached. A widget that went on showing "Quiet" with
nothing watching the headphones would be the application pretending.

### One instance

Two copies of the application would fight over the headphones' single control
connection, and "minimize to tray" makes a second launch likely. Starting the
application again brings the running one forward instead.

## Stack

| Layer | Choice | Note |
| --- | --- | --- |
| Shell | Tauri 2 | Small binary, real installer, native tray |
| UI | React 19 + TypeScript | Strict mode, no `any` in device types |
| Styling | Tailwind CSS 4 | CSS-first config in `styles.css` |
| Native | Rust | `windows` crate 0.58 |
| State | Zustand | Small, no boilerplate |
| Storage | SQLite (`rusqlite`, bundled) | Not yet wired |

## Not yet implemented

Named honestly rather than stubbed with fake controls:

- Windows Core Audio (volume, mute, endpoints)
- Windows media session transport controls
- SQLite persistence (settings are a small JSON file; profiles live in the
  webview's local storage)
- The standalone discovery tool in `tools/bose-discovery/`
- Any vendor-protocol feature other than noise control and the hardware
  equalizer — wind block, creating or renaming custom modes, multipoint,
  sidetone, voice prompts, rename

The Settings and Profiles pages state plainly which controls are unavailable
rather than showing switches that do nothing.
