# Source of truth

One document that records what this project is, what was tried, what worked,
what did not, and the exact state it was left in. If something breaks, start
here: every claim below says whether it was **measured**, **tested**, or
**not verified**.

Last updated: 2026-10-13.

- Repository: `https://github.com/Jaisalrathi1829/bose-qc-control-center` (private), branch `main`
- Everything described here is committed and pushed. The commit that holds the code is `621448e` (see [§12](#12-git-state))

Deeper detail lives in the other documents; this one is the index and the
record of decisions:

| Document | Holds |
| --- | --- |
| [protocol-notes.md](protocol-notes.md) | Every protocol experiment, with wire traces |
| [capability-matrix.md](capability-matrix.md) | Status of each feature and the evidence for it |
| [architecture.md](architecture.md) | Layering and design decisions |
| [voice-feedback.md](voice-feedback.md) | Announcement rules, runtime, measurements |
| [android.md](android.md) | The Android build |
| [security.md](security.md) | Boundaries and privacy |
| [troubleshooting.md](troubleshooting.md) | Symptoms and fixes |

---

## 1. What this is

A fully local control application for Bose QuietComfort Headphones, for
Windows and Android. No cloud, no account, no telemetry.

**Governing rule:** *build what can be proven; simulate only what is labelled
as simulated; never pretend.* A change is reported as applied only when the
headphones' own read-back says so.

**Standing limits (never to be crossed):** no firmware modification, flashing
or extraction; no bypassing security or brute-forcing authentication; no
destructive fuzzing; no arbitrary commands. The interface can never send raw
Bluetooth bytes.

### The test device

| | |
| --- | --- |
| Model | Bose QuietComfort Headphones, codename `prince` |
| Name on the PC and phone | **Aurora** (renamed by the owner — a name match alone never finds it) |
| Bluetooth SIG vendor id | `0x009E` |
| Product id | `0x4075` |
| Firmware | `1.0.6-80+f5f219b` |
| Bluetooth address | `E4:58:BC:F9:F0:2E` (stays in native code; never shown or exported) |
| Custom mode set up on it | "Focus" (slot 2) |

Only product `0x4075` is on the allowlist (`SUPPORTED_PRODUCT_IDS` in
`app/src-tauri/src/bose/bmap/mod.rs`). Other models upstream supports are
refused until tested on hardware.

### Stack

Tauri 2.11 · React 19 · TypeScript · Vite 7 · Tailwind 4 · Vitest · Rust
(MSVC on Windows, NDK on Android) · `windows` crate 0.58 · vendored
`bose-bmap` 0.5.0 (aaronsb/bosectl, MIT, commit
`70570b29c1d3c9ba775b08a29eaea72385de27b5`) · sherpa-onnx 1.13.8 with
ONNX Runtime 1.28.2 · Kokoro `kokoro-int8-en-v0_19`.

---

## 2. The control protocol — established facts

Bose's BMAP protocol over Bluetooth Classic RFCOMM.

**Frame:** `[function block, function, flags|operator, length, payload…]`

**Operators:** SET 0 · GET 1 · SETGET 2 · STATUS 3 · ERROR 4 · START 5 ·
RESULT 6 · PROCESSING 7

| Address | Meaning | Verified |
| --- | --- | --- |
| `[0.3]` | Product id | Measured 2026-10-11: `GET` → `00 03 03 03 40 75 0D` = `0x4075` + variant `0x0D`, matching Windows |
| `[0.5]` | Firmware version | Used to validate every connection |
| `[1.2]` | The headphones' own name (`00`, then UTF-8) | Read and written 2026-10-13, confirmed by read-back |
| `[1.7]` | Equalizer, 3 bands, −10…+10 | Read and written, confirmed by read-back |
| `[1.10]` | Multipoint: `07` on, `06` off | Read and written 2026-10-13. Going off, the headphones dropped the computer that asked |
| `[2.2]` | Battery | Measured 2026-10-11: `GET` → `02 02 03 04 0A FF FF 00` = 10 %, Windows showed 10 |
| `[4.1]`, `[4.2]` + address | Connect to, disconnect from, a paired device | **Not run on the headphones.** An older model's requests; sent by the per-device switch added 2026-10-13 |
| `[4.4]` | Addresses of the paired devices | Read 2026-10-13. Never written |
| `[4.5]` + address | One paired device: status byte (bit 0 connected, bit 1 "this is you") and name | Read 2026-10-13. Never written |
| `[31.1]` | List all modes (START, answered by a series of `[31.6]`) | Read |
| `[31.3]` | Current mode index | Read and written |
| `[31.6]` | Mode configuration (47-byte STATUS, 39-byte SETGET on this model) | Read; level written with a minimal-edit guard |

Mode slots on the test unit: 0 "Quiet" (strength 10, fixed), 1 "Aware"
(strength 0, fixed), 2 "Focus" (editable), 3 unnamed (editable). Strength
shown to the user is `10 − raw`.

### How to reach the headphones

| Route | Result |
| --- | --- |
| **Serial Port service UUID** `00001101-…` → channel 8 | **The working route.** The device shows no Serial Port record when browsed, yet connecting by that UUID resolves to channel 8 |
| BMAP service UUID `00000000-deca-fade-deca-deafdecacaff` → channel 14 | **Does not speak BMAP.** Use it only to *recognise* a Bose device, never to connect |
| Fixed channels 2, 8, 9 | Documented fallbacks, tried in that order after the service route |
| Headset 11, Hands-Free 12, SRfcomm 25 | Other services; not used |

The headphones accept **one** control connection per host. Whoever opens it
first has it (proved by `WSAEADDRINUSE`).

### Behaviour that shaped the design

- **The channel never pushes.** Across 192 seconds with six physical mode
  changes, the device sent nothing unprompted (Experiment 2). A button press
  on the headphones can only be discovered by asking.
- **The 200 ms settle pause is unnecessary** (Experiment 8). Upstream sleeps
  200 ms after every send; this transport waits for the reply against a
  deadline instead. Removing it took a mode read from 200 ms to ~22 ms and a
  confirmed mode switch from 1.2 s to 0.19 s, with every hardware test still
  passing.
- **A socket that connects is not a BMAP endpoint.** Every route must answer
  a firmware GET correctly before it is used.
- Sending `shutdown(SD_SEND)` made the device drop the channel after 16 ms;
  not sending it keeps the channel open indefinitely.

### Verification discipline

read → write → read back. "Sent", "acknowledged" and "confirmed" are kept
separate. A write is **never** repeated; a read may be retried once on a
fresh connection. Outcomes on the wire to the UI: `applied`,
`sent-unverified`, `rejected`, `unsupported`.

---

## 3. What works, and how it was proved

### Windows — verified on the physical headphones

| Feature | Evidence |
| --- | --- |
| Find the headphones, connection state, battery | PnP properties; battery cross-checked against `[2.2]` |
| Read the noise-control mode | `hw_a1`–`hw_a3`, `hw_b0` |
| Switch Quiet ↔ Aware | `hw_b1`, both directions confirmed by read-back |
| Custom mode: select it, change its level | `hw_b5` (Focus 4 → 6 → 4) |
| Hardware EQ: read, change | `hw_b6`, `hw_b7` (bass +4 → +5 → +4) |
| Recover from a dropped connection | `hw_b3` |
| Asking for the mode already active sends nothing | `hw_b2` |
| Real Kokoro speech, all 11 voices | `kokoro_real_*` |
| Speech rendered to Aurora's own Windows output | `voice_real_*`, `voice_hw_*` — endpoint meter 0.0000 silent, ~0.24 speaking |
| A confirmed mode change is spoken; an unconfirmed or unchanged one is not | `voice_hw_mode_change_*` |
| **Confirmed by the owner by ear:** voice feedback from the app and from the headphones' own button | "it works perfectly" |
| Settings: tray, start minimized, auto-connect, notifications, single instance | Implemented; tray clicks, toasts and "Start with Windows" in an installed build were not exercised by automated tests |
| Windows installer with the voice model | Built, installed by the owner, voice working after the path fix in [§7](#7-problems-hit-and-how-they-were-solved) |

### Android — verified on the emulator (Android 16), x86-64 build

| Feature | Evidence |
| --- | --- |
| App starts; Rust layer and Kotlin bridge talk | Screens rendered from native data |
| Bluetooth permission refused → "Bluetooth access is needed" → asked again from the app → granted → continues by itself | Screenshots |
| Voice model unpacked from the APK and checksum-verified | Status "Ready", 11 voices listed |
| Kokoro synthesis and playback | Android audio log: an `AudioTrack` from the app, `USAGE_MEDIA / CONTENT_TYPE_SPEECH`, focus held ~2 s |
| Refuses to speak through the phone when headphones are absent and the fallback is off | Status "Unavailable" with the reason |
| Background service | `AuroraService isForeground=true`, type `connectedDevice`, notification shown; restored at next launch |
| Home-screen widget, in every state that needs no headphones | Log lines and screenshots — [§16](#16-home-screen-widget-on-android-2026-10-12) |

### Android — reported by the owner from a Galaxy S24 Ultra (2026-10-12)

Not instrumented; this is what the owner said of the build of 2026-10-11
while reporting its problems.

| Feature | Report |
| --- | --- |
| The phone build (arm64) runs on the phone | Yes |
| Bluetooth control: modes switched, including the custom one | Yes |
| Announcements spoken, including with the app in the background | Yes — late on the custom mode, see [§13](#13-second-round-on-android-2026-10-12) |

### Not verified anywhere yet

- The **build of 2026-10-12** on a phone — every fix in §13
- The Android home-screen widget, state by state, on a phone — the owner
  reports it working (§16); nothing was instrumented
- The equalizer on the Android widget against the headphones (§17)
- Custom-mode level and EQ from Android
- Which output Android used for speech
- Battery level on Android
- The volume slider moving the headphones' volume from a phone
- The spoken and shown "connected to …" on a phone
- Connect / disconnect announcements and the low-battery warning on real
  hardware (either platform) — logic is unit-tested only
- Whether the headphones accept a control connection from the phone while a
  PC running the desktop app is also connected

---

## 4. Architecture in one page

```
React UI ──typed commands only──► Rust native layer ──► platform
                                   │
                                   ├─ device/      capability engine, typed commands, state
                                   ├─ bose/real.rs the hardware backend
                                   ├─ bose/bmap/   link, endpoint validation, verification
                                   ├─ watcher.rs   presence, battery, fast mode poll
                                   ├─ voice/       policy → queue → worker → Kokoro → output
                                   ├─ settings.rs  settings.json
                                   └─ platform:
                                        Windows: bluetooth/pnp, bmap/winsock, audio/playback, tray, autostart
                                        Android: android/{bridge,rfcomm,directory,sound} → AuroraPlugin.kt
```

Rules that hold everywhere:

1. The UI cannot express a Bluetooth write. No command carries a UUID,
   handle or byte array.
2. One mutex-guarded control link; operations are strictly sequential.
3. Capability states: UNKNOWN / SUPPORTED / READY / VERIFIED /
   UNSUPPORTED (READY was called EXPERIMENTAL until 2026-10-13). VERIFIED needs a `HardwareProof` built from an observed
   change in device-reported state.
4. The mock device is always labelled SIMULATED and is never watched,
   notified about or announced.
5. Voice is told what happened after the fact; nothing in the device layer
   calls it, so a speech failure cannot affect control.

---

## 5. Voice feedback

**Runtime:** sherpa-onnx 1.13.8 (official Rust crate, `shared` linking).
**Model:** `kokoro-int8-en-v0_19`, 11 English voices, 24 kHz mono. Default
voice `af_bella`.

| Asset | Value |
| --- | --- |
| Model archive | `kokoro-int8-en-v0_19.tar.bz2`, 103,248,205 bytes, SHA-256 `c9f0dd393615805b0bab050c340834d5e684e732aec91c0e860cd30e982c08bd` |
| `model.int8.onnx` | 134,186,977 bytes, SHA-256 `e0530c5031c03c5dc957edfc6d72582c2246e9b4566643ff67eeb03610c11641` |
| Windows runtime archive | `sherpa-onnx-v1.13.8-win-x64-shared-MT-Release-lib.tar.bz2`, SHA-256 `b8eedf41bd6d3779218887b48367bb7a3ece5aaa7667f01f69ee823a12b0a9e7` |
| Windows model location | `%LOCALAPPDATA%\local.bosecontrolcenter.app\voice\kokoro-int8-en-v0_19`, or `voice\` beside the installed exe |
| Android model location | `voice-model.zip` in the APK, unpacked to the app's `files/voice-bundled` |

Eight model files are checked by size and SHA-256 before every load
(`voice/assets.rs`). **Why:** the native runtime aborts the whole process on a
file it cannot parse.

### Announcement rules

- **Mode:** only when two successive readings from real, connected headphones
  differ. Never on a click, a send or an acknowledgement. Not at start-up, not
  on reconnect, not when re-selecting the active mode. Stale ones are dropped.
- **Connection:** only a change between observations; two missed looks before
  "disconnected"; "connected" waits up to 8 s for the audio output;
  "disconnected" is spoken only through another output, and only if allowed.
- **Battery:** warn once at ≤ 20 %; re-arm only above 25 %. Routine
  percentage announcements are **not implemented**.
- **Texts:** "Quiet mode activated." · "Aware mode activated." ·
  "*name* mode activated." · "Aurora connected to *this machine's name*."
  (or "Aurora connected." when the system gives no name) ·
  "Aurora disconnected." · "Battery low. *N* percent." · test: "Voice
  feedback is ready."
- **Priority:** connection, mode, low battery, (reserved), test. One per
  subject waits; queue holds 8; nothing overlaps.
- **Output:** the headphones' own output, found by device identity not name.
  Never silently the speakers; the fallback is an explicit switch, off by
  default. No system volume is read or changed.

### How button presses are heard quickly

While mode announcements are on, the watcher asks for the mode every
**150 ms** (`MODE_INTERVAL` in `watcher.rs`) using one 4-byte GET on the open
link (`peek_noise_control`). Sentences are synthesized ahead of need, so the
audio starts 1–3 ms after the change is seen. Cost: more Bluetooth traffic
while that setting is on (battery effect not measured).

A change made from the application is announced from the read-back the
change itself ends with (`observe_reported_mode` in `lib.rs`), not from the
next refresh.

### Sentences are kept on disk

`voice/store.rs`. Every sentence of up to 80 characters is written to
`<cache dir>/spoken/` the first time it is synthesized and read back from
then on: 16-bit audio, at most 64 files, least recently used dropped first,
each ending in a SHA-256 of itself and naming the model, voice, speed and
sentence it is for. A file that fails its check is deleted. The model is
therefore loaded only for a sentence never said before in that voice — the
first run, and after a change of voice — and unloaded after 90 s idle on
Windows, 8 s on Android.

### Measurements (Windows, i7-14650HX, debug build, 2 threads)

| | |
| --- | --- |
| Model load incl. checksums | 0.9–1.0 s typical, 2.5 s once |
| Synthesis of a 1.56 s sentence | ~1.6 s |
| Test Voice cold, request to sound | 2.5 s |
| Same sentence again | 1 ms |
| Stop Speaking | ~33 ms |
| Memory: idle / model loaded / after 90 s idle unload | 16 / 265 / 45 MB |
| One mode read | 22 ms median |
| Mode switch, request to confirmed | 0.19 s |

---

## 6. Android port — how it is put together

Shared: the whole interface and all control logic. Different: only the
platform layer.

| Job | Windows | Android |
| --- | --- | --- |
| Bytes to the headphones | Winsock | `BluetoothSocket` in `AuroraPlugin.kt` |
| Paired / connected | PnP | Bonded list + A2DP/Headset profiles |
| Which model | PnP product id | Ask the headphones: `GET [0.3]` |
| Battery | PnP | Android's level; else `GET [2.2]` |
| Speech out | WASAPI on the endpoint | `AudioTrack`, headphones' output preferred |
| Alive off screen | Tray | Foreground service `AuroraService` |
| Modes and equalizer without opening the application | A second window (`widget.rs`), sliders | A home-screen widget (`ModeWidget.kt`, `android/widget.rs`), a step down and up for each band |
| Volume slider | Core Audio, default output | `AudioManager`, media stream, in its steps |
| This machine's name | `GetComputerNameExW` | Bluetooth adapter name, else settings, else model |

Key files:

- `app/src-tauri/src/android/` — `bridge.rs` (calls into Kotlin),
  `rfcomm.rs`, `directory.rs`, `sound.rs`, `widget.rs`
- `app/src-tauri/gen/android/app/src/main/java/local/bosecontrolcenter/app/`
  — `AuroraPlugin.kt`, `AuroraService.kt`, `MainActivity.kt`, `ModeWidget.kt`
- `app/src-tauri/capabilities/mobile.json` — grants the web view nothing of
  the bridge
- `app/src-tauri/tauri.android.conf.json` — window visible on Android
- `tools/build-android.ps1` — the build

Android specifics worth remembering:

- The bridge is a Tauri plugin called **only from Rust**
  (`run_mobile_plugin`). Bytes cross it as hexadecimal text.
- A device is a candidate if Android lists Bose's control service for it
  (either byte order), else by name; its model is then asked for, and only
  `0x4075` gets a link.
- The product-id probe tries the Serial Port service, then channels 2, 8, 9;
  it retries no sooner than every 15 s after a failure.
- Settings on a phone: "Start with Windows" and "Start minimized" are hidden;
  "Minimize to tray" becomes **Keep running in the background**.
- Permissions: `BLUETOOTH_CONNECT`, `POST_NOTIFICATIONS`,
  `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_CONNECTED_DEVICE`, `INTERNET`
  (needed by the web view to load the bundled interface; nothing in this
  app's code uses the network).
- Minimum Android version: API 28. Target: API 36.
- Debugging on a phone: `adb logcat -s Aurora`.

---

## 7. Problems hit and how they were solved

| Problem | Cause | Fix |
| --- | --- | --- |
| Explicit lookup of the Serial Port service returned "no more" | The device lists no such record when browsed | Connect by UUID and let the system resolve it; found channel 8 by exclusivity |
| UI treated an unverified command as unknown | Rust serialized `sentUnverified`, UI expected `sent-unverified` | Kebab-case on the wire, with a test |
| Kokoro output differed run to run | Synthesis is not bit-deterministic | Compare voices by pitch, not by samples |
| A corrupt model file killed the process | Native runtime aborts on unparsable input | SHA-256 pinning before every load |
| Tests loaded the wrong `onnxruntime.dll` | Windows ships an older one in `System32` | Copy the runtime DLLs beside each executable (`build.rs`) |
| Hardware tests failed while the app was open | One control connection per host | Close the app first; never kill the owner's running app |
| Voice model "Not installed" although the script succeeded | The script was run from inside the packaged Claude app, whose writes under `%LOCALAPPDATA%` are redirected to `…\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Local\` | Run the script (or copy the folder) from an ordinary PowerShell window |
| Installed app: "could not load the model in `\\?\C:\…`" | The speech runtime cannot open Windows verbatim paths | Strip the `\\?\` prefix (`plain_path` in `voice/backend.rs`) |
| Button presses announced seconds late | The channel pushes nothing; polling was every 4 s and only while visible | 150 ms native poll + removal of the 200 ms settle pause |
| Status showed "Announcement" while only preparing sentences | Preparation reused the announcement status | Separate `Purpose::Preparation`, names no sentence |
| A voice test named `…hw_a…` was matched by the "read-only" hardware filter | Substring filter | Renamed; voice tests avoid `hw_a`/`hw_b`/`hw_c` |
| `tauri android build` failed: "symbolic link not allowed" | Needs Windows Developer Mode | `build-android.ps1` copies the library and runs Gradle with the Rust task excluded; Developer Mode left off |
| Gradle: `'gradlew.bat' is not recognized` | Current directory not searched | Call `.\gradlew.bat` |
| arm64 APK crashed at launch on the emulator | ONNX Runtime's CPU detection crashes under ARM translation | Emulator-only; test with `-Target x86_64` |
| Android project was not tracked by git | Root `.gitignore` ignored all of `app/src-tauri/gen/` | Ignore only `gen/schemas/` |
| Sliders could commit twice on touch | Pointer-up and touch-end both fire | One commit per value within 800 ms; `touch-action: none` |
| Phone: ~500 MB in the background | The voice model was loaded at every start and held 90 s after each use | Sentences kept on disk; 8 s unload on Android; `mallopt(M_PURGE_ALL)` |
| Phone: announcement late on the custom mode, sometimes on the next one | Listing the modes waited 500 ms for silence, was done on every refresh in a custom mode and before every switch into one, and held the link meanwhile; the custom mode's sentence was never prepared | End the reply at the device's `RESULT` frame; list far less often; prepare custom-mode sentences; announce from the write's own read-back |
| Phone: the dashboard animation glitched | Script-driven animation on every frame, an animated `filter`, `mix-blend-mode`, trimming by rounded `overflow: hidden`, two 120 px blurs behind every page, a frosted header, and a page rebuild on every connection-state change | CSS `transform`/`opacity` animation only; none of those effects; page rebuilt only when replaced |
| A page could come up blank after the connection state changed | `AnimatePresence mode="wait"` left the incoming page at opacity 0 when the key changed again during the exit animation | Pages fade in by CSS and are not held back for an exit animation |
| A preparation interrupted by an announcement was lost | It was popped from the queue and never put back | Put back at the front when it was set aside rather than called off |

Tooling quirks of this environment: bash heredocs that contain apostrophes or
backslashes get mangled — write patch scripts to a file and run them; `cargo
build` of the app's exe fails with "Access is denied" while the dev app runs
(use `cargo test --lib`).

---

## 8. Build, run, test — exact commands

All from `D:\PROJECTS\Bose Prototype`.

### One-time

```powershell
npm install
```

Voice model (98 MB download, run from an **ordinary** PowerShell window):

```powershell
powershell -ExecutionPolicy Bypass -File tools\install-voice-model.ps1
```

### Windows

```powershell
npm run tauri:dev
```

Installer without the model (7 MB):

```powershell
npm run tauri:build
```

Installer with the model (105 MB):

```powershell
npm run tauri:build:voice
```

Never a bare `tauri build`: the result would lack the two speech DLLs the exe
cannot start without (the build prints a warning if you do).

Output: `Setups\Bose QC Control Center_0.1.0_x64-setup.exe` — a copy of
`app\src-tauri\target\release\bundle\nsis\Bose QC Control Center_0.1.0_x64-setup.exe`,
made by `tools\collect-setups.ps1` at the end of the build.

### Android

Phone (arm64):

```powershell
npm run android:build
```

Emulator (x86-64):

```powershell
powershell -ExecutionPolicy Bypass -File tools\build-android.ps1 -Target x86_64
```

Outputs:

- `Setups\Bose-QC-Control-Center-0.1.0-android-arm64.apk` (133 MB) — the
  phone build, copied there at the end of the build
- `app\src-tauri\gen\android\app\build\outputs\apk\<arm64|x86_64>\release\` —
  where Gradle leaves each one. The emulator's build stays there

### Where to find something to install

`Setups\`, at the top of the project (added 2026-10-12, at the owner's
request, so that nothing has to be dug out of the build trees). Every build
of the installer or the phone package ends by copying its result there under
a fixed name, replacing the last one, and rewriting `Setups\README.txt` with
each file's build time, size and SHA-256. `npm run setups` does the copying
on its own. The folder is ignored by git.

Toolchain the script finds by itself: JDK `C:\Program Files\Android\Android
Studio\jbr`; SDK `%LOCALAPPDATA%\Android\Sdk`; NDK `D:\Android\ndk\28.2.13676358`.

Emulator smoke test:

```powershell
emulator -avd Medium_Phone_API_36.0 -read-only -no-window -no-boot-anim -no-snapshot -gpu swiftshader_indirect
```

```powershell
adb install -r app\src-tauri\gen\android\app\build\outputs\apk\x86_64\release\app-x86_64-release.apk
```

### Tests

```powershell
npm run rust:test
```

```powershell
npm run test:run
```

```powershell
cargo test --manifest-path vendor/bose-bmap/Cargo.toml
```

Hardware tests need the headphones connected to the PC **and the app
closed**, one at a time (`--test-threads=1`). The third command is run by
itself, never in the same process as the second (why: [§15](#15-hardware-run-2026-10-12)):

```powershell
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 hw_a hw_b
```

```powershell
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 kokoro_real voice_real voice_hw
```

```powershell
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture bundled_real_model
```

Lint (two old lints in first-commit lines are allowed):

```powershell
cargo clippy --manifest-path app/src-tauri/Cargo.toml --lib --tests -- -D warnings -A clippy::redundant_closure -A clippy::collapsible_match
```

### Last known-good results

| Suite | Result | When |
| --- | --- | --- |
| Rust app | 579 passed, 0 failed, 47 ignored | 2026-10-13 |
| Frontend | 255 passed; `tsc` clean; `vite build` OK | 2026-10-13 |
| Vendored crate / tools | 103 / 16 / 27 / 32 passed | 2026-10-11 |
| Clippy | clean but for the two old lints allowed above | 2026-10-12 |
| Hardware (Windows) | `hw_a1–3`, `hw_b0–b8`, `hw_diag_mode_slots`, all `kokoro_real_*`, `voice_real_*` and `voice_hw_*`, and `bundled_real_model` by itself: all pass | 2026-10-12, after every change described in this document — see [§15](#15-hardware-run-2026-10-12) |
| Android compile | `aarch64` and `x86_64` | 2026-10-12 |

`npm run lint` has never worked: the repo has no ESLint config.

---

## 9. State the hardware was left in

Mode **Aware** · custom slot "Focus" at level **4** · EQ bass **+4**, mid
**+1**, treble **+4** · battery **10 %** when last read.

---

## 10. Licences

| Component | Licence |
| --- | --- |
| `bose-bmap` (vendored) | MIT, © 2026 Aaron Bockelie |
| sherpa-onnx | Apache-2.0 |
| ONNX Runtime | MIT |
| Kokoro model | Apache-2.0 |
| eSpeak NG (inside the sherpa library and the model's data) | **GPL-3.0-or-later** |

Using builds yourself is unaffected. Giving an installer or APK to someone
else distributes GPL code and must follow the GPL; this project has not
chosen a licence of its own. Details in `THIRD-PARTY-NOTICES.md`.

The Android APK is signed with the Android **debug** key — fine for your own
phone, not for distribution.

---

## 11. If something goes wrong

| Symptom | First thing to check |
| --- | --- |
| Windows app shows SIMULATED | Is a Bose device paired with this PC? |
| Controls greyed, "could not be reached" | Is another program (a second copy, a test, the Bose app) holding the control channel? |
| Voice "Not installed" on Windows | Is the model in the **real** `%LOCALAPPDATA%\local.bosecontrolcenter.app\voice`? (see the redirection row in §7) |
| Installed app will not start, missing `sherpa-onnx-c-api.dll` | Built with bare `tauri build`; use `npm run tauri:build` |
| Voice "Unavailable" | Headphones' output not active and the fallback switch is off — by design |
| No announcement on a mode change | Voice feedback on? Mode announcements on? Real device? Did the mode actually *change*? |
| Android: "Bluetooth access is needed" | Grant "Nearby devices" |
| Android: connected but nothing controllable | Close the Bose app; it may hold the control channel |
| Android: announcements stop off screen | Turn on "Keep running in the background" |
| Android build: symbolic link error | Use `tools\build-android.ps1`, not `tauri android build` |
| Phone APK crashes on the emulator | Expected; build with `-Target x86_64` |
| Android: a lot of memory | `adb logcat -s Aurora`: is "voice model loaded" followed within seconds by "released"? |
| Android: volume slider lands elsewhere | It sets the nearest of the phone's own steps |
| An announcement is late every time | Is it the first run in that voice? Otherwise see Experiment 9 in protocol-notes.md |
| Anything on the phone | `adb logcat -s Aurora` |

To get back to a known-good state, check out `b1f2a3f` (everything, hardware-tested on Windows). If the trouble is in that commit itself, `b5c0446` is the widget and phone fixes without the last two timing fixes, `55e3684` is Windows and Android as first built, and `659446f` is the last commit before the Android port (Windows only, confirmed by ear).

---

## 12. Git state

On `origin/main`:

| Commit | Holds |
| --- | --- |
| `365b820` | Verified noise control, custom mode, hardware EQ, settings, local voice feedback |
| `659446f` | 150 ms button detection, settle pause removed, Windows installer with the voice model, verbatim-path fix |
| `55e3684` | The Android port (`src/android/`, `gen/android/`, `tools/build-android.ps1`, phone layout), battery over the control protocol, `DeviceError::NotPermitted`, `docs/android.md`, and this file |
| `b5c0446` | The fixes from the first run on a phone ([§13](#13-second-round-on-android-2026-10-12)): sentences kept on disk, the mode list read to its end marker, CSS-only animation, phone media volume, "connected to". And the desktop widget ([§14](#14-desktop-widget-2026-10-12)) |

| `b1f2a3f` | What the hardware run of 2026-10-12 led to ([§15](#15-hardware-run-2026-10-12)): the custom-mode switch and level write sent as single-reply requests, the guard on the speech runtime's pronunciation folder, `hw_b8` |

| `7bfd6e1` | The Android home-screen widget ([§16](#16-home-screen-widget-on-android-2026-10-12)), and the `Setups` folder tooling (`tools/collect-setups.ps1`) |

| `1502442` | The equalizer on the Android widget, and the widget's taps no longer waiting on one another ([§17](#17-equalizer-on-the-android-widget-2026-10-12)) |

| `621448e` | Everything of 2026-10-13 ([§18](#18-third-round-on-the-phone-app-2026-10-13)): multi-device connection, renaming, a switch for each paired device, "ready" in place of "experimental", and one interface for the phone and the desktop — no Diagnostics, no card on the Device page, no software equalizer |

Commits that touch only this document are bookkeeping; `621448e` is the one
to return to for the code.

Earlier points to go back to, if something in the latest turns out wrong:
`1502442` is the last commit with the Diagnostics page, the software-equalizer
panel and the card on the Device page, and before anything was read from or
written to the headphones' name, multipoint or paired devices;
`7bfd6e1` is the Android widget with the modes alone, as the owner reported
working on the phone — with the 2.5 s wait between taps still in it;
`b1f2a3f` is everything verified on the headphones, without the Android
widget; `b5c0446` has the desktop widget and the phone fixes but not the two
timing fixes of §15; `55e3684` is the Android port as first built, before any of that; and
`659446f` is the last commit before Android, Windows only, confirmed by ear.

Not in the repository, by design: build output (`target/`, `gen/android/app/build/`),
the compiled libraries, the installers and APKs, and the voice model. They are
rebuilt or reinstalled with the commands in [§8](#8-build-run-test--exact-commands).

---

## 13. Second round on Android (2026-10-12)

The owner ran the first phone build on a Galaxy S24 Ultra and reported five
things. This section records what each turned out to be, what was changed,
and what has and has not been checked.

### What was reported, found and done

| # | Reported | Found | Done |
| --- | --- | --- | --- |
| 1 | ~500 MB in the background | The voice model, loaded at every start to prepare sentences and held 90 s after each use. Emulator: 300–355 MB for the app process, ~95 MB for its web view | Sentences kept on disk (`voice/store.rs`); model unloaded after 8 s on Android; freed memory returned with `mallopt(M_PURGE_ALL)`; nothing re-read while the window is hidden |
| 2 | The revolving headphones glitch | Script-driven animation on every frame; an animated `filter`; `mix-blend-mode`; trimming by rounded `overflow: hidden`; two 120 px blurs behind every page; a frosted header; the page rebuilt on every connection-state change, by a transition that could stick at transparent; panels that waited on script to become visible | All animation is CSS on `transform`/`opacity`; none of those effects remain on a phone, where the background glows are also held still; a page is rebuilt only when replaced; pages and panels appear by CSS |
| 3 | Volume slider "Windows only" | True | Phone media volume through `AudioManager`, set to the nearest step and read back |
| 4 | Say which device it is connected to | — | This machine's name (`host.rs`): shown on the dashboard and Device page, spoken as "Aurora connected to …", used in the notification |
| 5 | Voice prompt late on the custom mode, sometimes on others | The mode list: each read waited 500 ms for silence with the link held; it was read on every refresh in a custom mode and before every switch into one; the custom mode's sentence was never prepared; an interrupted preparation was lost | Reply ends at the device's `RESULT` frame (Experiment 9); list read on arrival and every 10 s in a custom mode; custom-mode sentences prepared; interrupted preparations resumed; a change made from the app is announced from its own read-back |

### New and changed files

| File | What |
| --- | --- |
| `app/src-tauri/src/voice/store.rs` | **New.** Synthesized sentences on disk, checksummed, 64 files, least recently used dropped |
| `app/src-tauri/src/host.rs` | **New.** This machine's name |
| `app/src-tauri/src/voice/mod.rs` | `start_with_store`, `expect_modes`, `set_host`, resumable preparation, 8 s unload on Android |
| `app/src-tauri/src/voice/policy.rs` | `Connected { name, host }`, `speakable_name` |
| `app/src-tauri/src/voice/backend.rs` | `release_freed_memory` (Android) |
| `app/src-tauri/src/bose/bmap/stream.rs` | `ends_drained_reply` |
| `app/src-tauri/src/bose/bmap/link.rs` | `slots_age` |
| `app/src-tauri/src/bose/real.rs` | `mode_list_due`, `reported_noise_control`, `known_custom_modes` |
| `app/src-tauri/src/device/{traits,state,capability}.rs` | Two trait methods; `SystemAudioState` (was `WindowsAudioState`), `host_name`; `Mechanism::AndroidAudio` |
| `app/src-tauri/src/lib.rs`, `watcher.rs` | `attach_system`, `observe_reported_mode`, Android volume and mute, host name passed to voice and notifications |
| `app/src-tauri/src/android/{bridge,sound}.rs` | `host_name`, `note_memory`; `MediaVolume`, `percent_of`, `level_for`; playback asks Android whether it is done only once it could be |
| `…/AuroraPlugin.kt` | `volumeGet`, `volumeSet`, `muteSet`, `hostName`, `noteMemory` |
| `app/frontend/src/components/DeviceVisual.tsx`, `styles.css` | The visual, and every endless animation, in CSS |
| `app/frontend/src/App.tsx` | `screenKey`; CSS page entrance; plain gradients for the background glows |
| `app/frontend/src/components/{primitives,TopBar}.tsx` | Panels by CSS; pulses by CSS; opaque header on a phone |
| `app/frontend/src/store.tsx`, `pages/{Dashboard,Device}Page.tsx`, `pages/VoiceFeedbackPanel.tsx` | `connectedTo`, `volumeKnown`, phone wording, no re-read while hidden |

Wire change: the snapshot field `windowsAudio` is now `systemAudio`, and
there is a new `hostName`.

### Checked

| What | How | Result |
| --- | --- | --- |
| Unit tests | `cargo test --lib`, `vitest run` | 477 Rust, 196 frontend, all passing |
| Memory on the emulator (x86-64, Android 16), `dumpsys meminfo` PSS | Before / after | App process 300–355 MB → 115–129 MB; native heap 190–240 MB → 14–31 MB. Web view process ~80–105 MB either way |
| Second start loads no model | `adb logcat -s Aurora` | No "voice model loaded" line |
| First start after install | same | "loaded: native heap 214 MB" … "released: native heap 39 MB", ~21 s apart on the emulator |
| Test Voice | Android's audio log | First press: model loaded, an `AudioTrack` from the app, focus held ~2.3 s, model released 8 s after its last use. Second press: played at once, no synthesis |
| Phone volume | `cmd media_session volume --get`; Android's audio log | Slider moved the media stream from step 5 to 12 of 15; `setStreamVolume(… index:12 … oldIndex:5) from local.bosecontrolcenter.app`; slider then read 80 % |
| Phone name | log | "phone name taken from its Bluetooth name" |
| Dashboard and Device page draw | Emulator screenshots; browser preview at phone and desktop width, light and dark | Yes. In the browser: 19 CSS animations running, none script-driven, no element with a filter, backdrop filter or blend mode at phone width |

### Not checked

- **Anything on the phone.** The build with these changes has not been run
  on the S24 Ultra. Smoothness in particular cannot be judged on the
  emulator, which draws in software.
- ~~The headphones, at all, after these changes.~~ Done later the same day,
  on Windows: [§15](#15-hardware-run-2026-10-12). It found two more waits.
- Memory on a phone; the volume slider with the headphones as the phone's
  output; the spoken "connected to …".
- The Windows installer has not been rebuilt. The desktop application the
  owner is running is the build from before these changes.

### What the owner then reported from the phone

After installing the build below over the old one: **218 MB in the
background** (from about 500), which is what the emulator predicted, and the
front screen "a little smoother". Not yet reported: the Focus prompt's
timing, the volume slider against the headphones, and the spoken and shown
"connected to …". What stutter remains has not been described, so it has not
been chased.

### Builds

| | |
| --- | --- |
| Phone | `app\src-tauri\target\android\Bose-QC-Control-Center-0.1.0-android-arm64.apk`, built 2026-10-12 10:16, 139,191,458 bytes, SHA-256 `1afecbc08f9c61092bbf3f4acd108c7da2cce650b398f00dd19d060bc4c2e978`. It differs from the emulator build that was exercised by one stylesheet rule (the background glows held still at phone width), checked in a browser at that width. **This is the build the owner installed** and reported on: 218 MB in the background, "a little smoother". The Gradle output folder has since been rebuilt with the widget's changes, which do nothing on a phone |
| Emulator | `…\apk\x86_64\release\app-x86_64-release.apk`, built 2026-10-12 |
| Windows installer, with the widget and the voice model | `app\src-tauri\target\release\bundle\nsis\Bose QC Control Center_0.1.0_x64-setup.exe`, built 2026-10-12 16:52, 105,174,541 bytes, SHA-256 `d8d73931fbb016492c68f0b801c771529f4cb265a81dcf2ee6b5e6fbc6764f7e`. Installed by the owner, who reports that it works |

---

## 14. Desktop widget (2026-10-12)

Asked for by the owner: a floating panel on the Windows desktop for the modes
and the equalizer, so that the application need not be opened for them.

### What it is

A second window of the same application (`widget.rs`), 316 × 352, frameless,
transparent around a rounded card, off the taskbar, and by default above
other windows. It has the three modes, the three equalizer bands, the
battery, a line saying how the last change went, and three buttons: pin,
open the main window, hide.

| Decision | Why |
| --- | --- |
| A window, not a separate program | One control connection per computer; a second program could not have it too |
| The same page, told apart by window name | One interface and one set of typed commands. `isWidgetWindow()` in `services/ipc.ts` |
| Made only while the setting is on | A hidden second web view would cost memory for nothing |
| Made on a thread of its own, and torn down from one | On Windows, making a window from inside a command or event handler deadlocks; and a window should not be destroyed from inside a request it is waiting on |
| A capability file of its own, `capabilities/widget.json` | It needs less than the main window: events and dragging |
| Position remembered, checked against the screens present | A place on a monitor since unplugged is not used (`widget::place`) |
| Always a way back | Closing the main window with the widget up hides it; closing the widget with the main window hidden and no tray icon shows the main window |

Settings added: `desktopWidget` (off by default), `widgetOnTop` (on by
default), and the stored `widgetPosition`. Events added: `settings-changed`
to every window after any setting changes; `device-changed` is now also sent
after a command that may have changed the device, so the window that did not
make the change catches up.

Ways to switch it: Settings → General → Desktop widget; the tray menu's "Show
or hide the widget"; the cross on the widget itself.

### Checked

| What | How | Result |
| --- | --- | --- |
| Placement, including a lost monitor and negative coordinates | `widget::tests` | 7 tests pass |
| The widget's interface: what it shows, changing a mode and a band, an unconfirmed change leaving the selection alone, its three buttons, following a setting changed elsewhere | `WidgetApp.test.tsx` | 16 tests pass |
| How it looks at its real size, light and dark, and that a click on a mode works end to end against the simulated device | Browser preview at 316 × 352 with `#widget` | As intended; no console errors |
| The whole suite | | 485 Rust, 215 frontend |

### Reported by the owner

After installing the installer listed in [§13](#13-second-round-on-android-2026-10-12)
on the Windows laptop: **"it works perfectly"**. That is the only evidence
there is for the window itself — it was never opened during development,
because the owner's installed application was running and a second copy only
brings the first forward.

### Not checked

- Anything about the window by measurement or by test: transparency,
  dragging, staying on top, staying off the taskbar, coming back where it was
  left, and the two "way back" rules rest on the owner's report above, which
  did not go through them one by one.
- A second monitor, or one unplugged while the widget was on it. The
  placement rule for that is unit-tested only.
- The widget against the headphones by test. (The control path under it was:
  [§15](#15-hardware-run-2026-10-12).)

---

## 15. Hardware run (2026-10-12)

The owner closed the desktop application and asked for the hardware tests.
This is what was run on the headphones, what it found, and what was changed
because of it.

### What it found

| # | Found | By | Done |
| --- | --- | --- | --- |
| 1 | The mode list does end with `RESULT` from the address asked, as the fix of §13 assumed. The live reply also carries three status frames the fixture did not have | `hw_diag_mode_slots`, wire log | Nothing to change. Recorded in Experiment 9 |
| 2 | A switch **into the custom mode** still took about 600 ms, against 100 ms for a preset. The request was sent through a call that waits 500 ms for silence after the reply; the headphones send one `PROCESSING` and nothing more | New test `hw_b8`, then the wire log of `hw_b5` | Sent as a single-reply request (`request_slot` in `noise.rs`). Experiment 10 |
| 3 | A change of the **custom mode's level** had the same wait | Timestamps in `hw_b5` | Same fix (`set_level` in `custom.rs`) |
| 4 | Three voice tests failed when the suite was run as one command. A test that loads the model from a temporary folder ran first and deleted the folder; the speech runtime (eSpeak NG) reads pronunciation data from the first folder of the process for the rest of it, so every later model spoke 0.3 s of noise with all checksums passing | The failures, and "Can't read dictionary file" naming the deleted folder | That test renamed to `bundled_real_model_…` and run by itself. And in the application: a model is refused, with a message saying to restart, if the folder the runtime is reading from is no longer intact (`PRONUNCIATION_DATA` in `kokoro.rs`) |
| 5 | Reading the mode back every 50 ms instead of every 250 ms after a switch made no measurable difference: the new mode is there at the first read-back | `hw_b8` before and after | Kept at 50 ms; the comment says what was actually found |

Finding 2 is the part of "the voice prompt lags on Focus" that was still
there. It affected a switch made from the application or the widget, on both
Windows and the phone, and not a press of the headphones' own button.

### Measured on the headphones (Windows, debug build)

| | Before this run | After |
| --- | --- | --- |
| Listing the modes | reply + 500 ms (before §13) | 22–46 ms, median 35 ms, nine reads |
| Switch into the custom mode, asked to confirmed | 596–628 ms | 80–119 ms |
| Switch between presets, or out of the custom mode | 65–112 ms | 59–112 ms |
| Change of the custom mode's level | about 730 ms each | about 200–280 ms each |
| One mode read, 40 in a row 150 ms apart | 22 ms median (2026-10-11) | 10 ms median, 8–64 ms |
| Mode change asked to confirmed, in the voice tests | 190 ms (2026-10-11) | 114–130 ms |
| Confirmed to first sound, sentence made ready | 1–3 ms | 1–2 ms |
| Refresh straight after a switch (mode, list if due, EQ) | not measured | 38–86 ms |

### What passed, after the changes

| Command | Result |
| --- | --- |
| `hw_a hw_b` | 12 of 12: `hw_a1`–`a3`, `hw_b0`–`b8`. Quiet and Aware both ways, the custom mode and its level with the saved configuration byte for byte restored, the equalizer, three reconnections |
| `kokoro_real voice_real voice_hw` | 16 of 16, in one process. Real speech in all 11 voices; rendered to "Headphones (Aurora)" with its meter moving; a confirmed mode change spoken and an unchanged one not |
| `bundled_real_model` | 1 of 1, by itself, including the refusal once its folder is gone |
| `hw_diag_mode_slots` | Slots as before: Quiet, Aware, "Focus" at raw 6, one empty |
| Unit tests | 487 Rust, 215 frontend |

### State the hardware was left in

Mode **Aware** · "Focus" at level **4** · EQ bass **+4**, mid **+1**, treble
**+4** — as it was found.

### Not checked

- The new builds below have not been run by anyone. The installed desktop
  application and the phone are still on the builds from before findings 2
  and 3.
- The switch into the custom mode has been timed on Windows only.
- `[31.2]`, `[31.5]` and `[31.8]`, which turn up in the mode list's reply,
  have not been looked into.

### Builds made after it

| | |
| --- | --- |
| Windows installer | `app\src-tauri\target\release\bundle\nsis\Bose QC Control Center_0.1.0_x64-setup.exe`, 2026-10-12 18:09, 105,273,630 bytes, SHA-256 `92912ceb0918dadfba1bd598214eb30f78a8c01dc28653a2a00ef8d0243f81eb` |
| Phone | `app\src-tauri\target\android\Bose-QC-Control-Center-0.1.0-android-arm64.apk`, 2026-10-12 18:11, 139,191,458 bytes, SHA-256 `e086bae7949d4c937132eae88165062b6807dda02788f362ad6d3cde3e7c582b` |

These replace the ones listed in §13.

---

## 16. Home-screen widget on Android (2026-10-12)

Asked for by the owner: Android offers an "add widget" entry for
applications that have one, this one had none, and a widget was wanted for
the modes — "modes only".

Committed as `7bfd6e1`.

### What it is

One widget, "Noise modes", four cells by one: the headphones' name, a line of
status, and three buttons — Quiet, Aware, and the custom mode under the
headphones' name for it. No equalizer, no battery, no settings.

| Decision | Why |
| --- | --- |
| The widget decides nothing; a tap is a mode's name left for the native layer | The link, the confirmation rule and the voice are in the native layer. A second path to the headphones from Kotlin would be a second set of rules |
| Requests are taken by one thread that waits on Kotlin (`widgetNext`, up to 50 s a time) | Kotlin cannot call into the native layer in this application — the bridge runs one way. Waiting costs nothing; polling would |
| That the native layer is waiting is the sign it is alive | A receiver started for a tap, in a process with no native layer, can tell at once and open the application instead |
| The mode shown is the reported one; a tap only outlines the button | The same rule as the main window: asked for is not confirmed |
| No mode shown when the application is closed, or off screen without the background service | Android stops such a process where it stands. Nothing would see the mode change, and the widget would be showing a guess |
| No mode shown on the simulated device | Its modes are not the headphones'. On a home screen there is no room for a "simulated" badge on each button |
| Receiver not exported; taps are immutable pending intents; a request is read into `NoiseModeRequest` | Nothing outside the application can ask, and nothing but one of three names can be asked for |
| With no native layer, the tap opens the application and the request waits for it (20 s at most) | The alternative was a tap that does nothing |

### Files

| File | What |
| --- | --- |
| `gen/android/.../ModeWidget.kt` (new) | `WidgetBus` — the requests, and whether anything is taking them; `ModeWidget` — the provider: draws, and receives taps |
| `gen/android/.../res/layout/widget_modes.xml`, `res/drawable/widget_*.xml`, `res/values/widget.xml`, `res/values-night/widget.xml`, `res/xml/widget_modes_info.xml` (new) | Layout, button and card shapes, strings and colours for light and dark, the declaration Android reads |
| `gen/android/.../AndroidManifest.xml` | The receiver |
| `gen/android/.../AuroraPlugin.kt` | Two bridge commands: `widgetNext`, `widgetState` |
| `gen/android/.../AuroraService.kt` | Says whether it is running; tells the widget when the application is swiped away or the service stops |
| `gen/android/.../MainActivity.kt` | Says when the window is on screen; takes the request a tap carried in |
| `src/android/widget.rs` (new) | `state_from` (what the widget is told), `parse_request`, `note_for`, and on Android the request thread and `seen` / `mode_changed` |
| `src/lib.rs`, `src/watcher.rs` | Start the thread; tell the widget after a command, a detected change, and each look at the headphones |

### Checked

| What | How | Result |
| --- | --- | --- |
| What the widget is told: reported mode only, none when disconnected, none on the simulated device, the custom mode's name, notes | `android::widget::tests` | 11 tests pass |
| What it can ask for: three names, and nothing else — not "off", not an index, not bytes | same | pass |
| The launcher offers it, and it can be placed | Emulator (Android 16, Pixel launcher) | "Noise modes 4 × 1", placed |
| Application kept running in the background, tap on Aware | Emulator, `adb logcat -s Aurora` | `aware asked for` → `aware taken up` after 1 ms → widget told after 6 ms more; the application stayed off screen |
| Application off screen, background switch off | Emulator | Widget reads "Tap to open the app"; a tap on a mode opened the application, which took the request up |
| Application's process ended, then a tap | Emulator, `am crash` | A new process, `the application is not running; opening it`, the application on screen, the request taken up 0.8 s after the tap |
| Application not running at all, tap | Emulator | Opened, request taken up at start |
| Simulated device | Emulator | "Simulated device in use", no mode; a tap changes nothing shown |
| Tap on the top line | Emulator | Opens the application |
| The whole suite | | 499 Rust, 215 frontend; clippy clean |

### Reported by the owner

After installing the phone package below on the Galaxy S24 Ultra, with the
headphones: **"it works, I have tested and it's perfectly fine"**. That is
the only evidence for the widget against the headphones and on a Samsung
home screen. It was not instrumented, and the report did not go through the
states one by one.

### Not checked

Everything below was written before the owner's report, and is unverified
*by measurement or test*; the report above covers it only in general.

- **Anything against the headphones.** The emulator has none, so no tap there
  ended in a mode change, and the widget has never been seen with a mode
  highlighted, with "Connected", with the headphones' name, with "Focus" on
  its third button, or with "Switching…". The switch it calls is the one
  measured in [§15](#15-hardware-run-2026-10-12); the drawing of those states
  is untested.
- A Samsung home screen: how the widget sits in One UI's grid, its look
  there, and whether One UI lets a tap bring the application up when its
  process has gone (the Pixel launcher on Android 16 does; if One UI does
  not, the widget reads "Tap to open the app" and the next tap opens it).
- A tap that opens the application and switches the mode in one go, on a
  phone: at start-up the link to the headphones is not yet open when the
  request is taken up.
- How long Samsung leaves the application running in the background with the
  switch on. If it is ended, the widget goes on showing what it last showed
  until it is tapped.

### Build

| | |
| --- | --- |
| Phone | `Setups\Bose-QC-Control-Center-0.1.0-android-arm64.apk`, 2026-10-12 22:30, 139,249,130 bytes, SHA-256 `b1284e162d905a8cc2820b41cd0edd1a5414f3da998543995b96dd261dc67dac`. Checked to hold the widget's receiver and the `arm64-v8a` libraries. Reported working by the owner on the phone |
| Windows installer | Unchanged — the one of 18:09 listed in §15. Nothing in this section alters the Windows build |

The phone package replaces the one listed in §15. It installs over the
version already on the phone.

---

## 17. Equalizer on the Android widget (2026-10-12)

Asked for by the owner after the widget of §16 was working on the phone:
"add eq to the android widget too".

Committed as `1502442`.

### What it is

The same widget, now called "Modes and equalizer". Under the three modes,
three columns — Bass, Mid, Treble — each with the level the headphones
report and two buttons, − and +. It is placed two cells tall; made one cell
tall it is the modes alone, exactly as in §16.

| Decision | Why |
| --- | --- |
| Steps, not sliders | A home-screen widget cannot hold a slider: Android lets it have text, images and buttons |
| A request is a band and a direction — never a level or an amount | The level it ends at is worked out in the native layer. `parse_request` reads `bass`/`mid`/`treble` and `up`/`down` and drops anything else |
| The equalizer is read from the headphones before every step | The write sends all three bands. Sent from what the widget was last showing, a step on bass would put mid and treble back to whatever was last seen, undoing a change made elsewhere. Read first, the other two go back as they are at that moment (`step` in `android/widget.rs`, `stepped`) |
| Nothing is sent for a step past −10 or +10, or from a reading outside that range | There is nothing proper to send. The line says "Already at the limit" |
| The level shown is the reported one; a dash when there is none | As for the mode. New trait method `reported_equalizer`: the last reading, costing nothing, forgotten when the headphones go |
| A step is not carried into an application that is not running | "One more" means something only beside the level it was tapped on, and a closed application's widget shows none. The tap opens the application and changes nothing |
| One widget that changes with its size, not a second widget | Both forms are handed to Android, which draws whichever fits (`RemoteViews` by size, Android 12 and later; before that the widget's reported height decides). An existing one-cell widget is left exactly as it was |
| The equalizer is re-read when five minutes old, only while a widget with it is placed | Nothing on the headphones changes it, but another device they are connected to can. Kotlin tells the native layer whether such a widget exists each time it is told the state (`showsEq`); with none, nothing is read |

### A fault found on the way: taps were taken one every 2.5 seconds

Testing six equalizer buttons in a row on the emulator, the log showed each
tap arriving exactly 2.5 s after the one before, whatever the pace of the
tapping. The widget's receiver was being held open (`goAsync`) for the 2.5 s
it waits to see whether the native layer has taken a request, and Android
hands a receiver one broadcast at a time. This was in the widget of §16 as
committed and as the owner has it on the phone: there it would only show as
a second mode tap, made straight after a first, being slow to act.

The hold was there so the process would not be stopped while waiting. It was
not needed: a request is only waited on when the application counts as
alive, which it does only while on screen or kept running by the service,
and neither is stopped. Removed. Measured after: nine taps in 0.45 s, each
received within about 40 ms, all nine taken up in order.

Also changed with it: whether a tap was heard is now recorded on the request
itself rather than as "the last request taken", so several can wait their
turn; and a tap made while the native layer is busy with the one before no
longer counts as the application having gone.

### Files

| File | What changed |
| --- | --- |
| `src/android/widget.rs` | `Band`, `Request::Step`, `stepped`, the three levels in `WidgetState`, `step` and `freshen` on Android; `parse_request` takes a band and a direction |
| `src/device/traits.rs`, `src/bose/real.rs` | `reported_equalizer` |
| `src/watcher.rs` | Calls `freshen` after each look at the headphones |
| `gen/android/.../ModeWidget.kt` | Rewritten around a request with a kind; the equalizer's buttons and levels; two forms by size; no receiver hold |
| `gen/android/.../AuroraPlugin.kt` | `widgetNext` hands on a band and a direction; `widgetState` takes the levels and answers `showsEq` |
| `gen/android/.../res/layout/widget_modes.xml`, `res/values/widget.xml`, `res/xml/widget_modes_info.xml` | The equalizer's rows and words; two cells tall by default |

### Checked

| What | How | Result |
| --- | --- | --- |
| A step moves one band by one and leaves the others; none past either end; none from a reading out of range; every step from every in-range level stays in range | `android::widget::tests` | pass |
| A request is a band and a direction and nothing else — not "+1", not "10", not "reset", not bytes | same | pass |
| Levels shown only for real, connected headphones that have reported them; never the simulated device's | same | 21 tests in the module, all pass |
| Offered as "Modes and equalizer 4 × 2", placed, drawn with the equalizer | Emulator (Android 16, Pixel launcher), screenshots | As intended |
| Made one cell tall: the modes alone. Made two again: the equalizer back | Emulator, dragging the resize handle | As intended |
| All six buttons, application kept running in the background | Emulator, `adb logcat -s Aurora` | Each `asked for` and `taken up` under its own name; the application stayed off screen |
| Quick taps | Emulator, nine taps in 0.45 s | All received within about 40 ms and taken up in order |
| A step with the application's process ended | Emulator, `am crash` then a tap on bass + | The application opened; no step was taken up |
| The whole suite | | 508 Rust, 215 frontend; clippy and the type check clean |

### Not checked

- **Anything against the headphones.** No level has been seen on the widget,
  and no tap there has changed a band: the emulator has none, so every step
  ended at "not connected". The read and the write under it are the ones
  the application's equalizer uses, tested on the headphones in
  [§15](#15-hardware-run-2026-10-12) on Windows.
- How long one step takes on the phone, and so how a run of quick taps feels.
- The five-minute re-read, and the read after the widget is placed.
- How the two-cell widget sits on a Samsung home screen, and whether One UI
  swaps the two forms as the widget is resized. (It is Android's own widget
  host that does the swapping, so it should.)
- The widget on Android 11 or earlier, where the form is chosen from the
  height the launcher reports.

### Build

| | |
| --- | --- |
| Phone | `Setups\Bose-QC-Control-Center-0.1.0-android-arm64.apk`, 2026-10-12 23:25, 139,287,950 bytes, SHA-256 `a16cee7b6419e59cb9838afede129201117280dca8a9493559e7c6e78007d47f`. Checked to hold the new widget's words and the `arm64-v8a` libraries; not run by anyone |
| Windows installer | Unchanged — the one of 18:09 listed in §15. Nothing in this section alters the Windows build |

The phone package replaces the one listed in §16. It installs over the
version already on the phone; a widget already on the home screen stays one
cell tall until it is stretched.

---

## 18. Third round on the phone app (2026-10-13)

Asked for by the owner, of the mobile app: remove the Diagnostics section;
remove the word "Windows" from the equalizer; remove the term "experimental"
everywhere; add a switch for the multi-device connection that shows what is
connected; detect the headphones' name whatever it is; and add a way to
change the name.

Committed as `621448e`, with everything else in this section.

### What was done

| Asked | Done |
| --- | --- |
| Remove Diagnostics | On Android it is not in the navigation, the Device page has no button to it, and Settings has no logging section (`pagesFor` in `pages.ts`). The desktop keeps all of it |
| "Windows" in the equalizer | The software-equalizer panel is not shown on Android — it was a panel for a Windows audio pipeline that does not exist — and the page is the headphones' own equalizer. The Device page, the battery tooltip and the reasons given for a capability's state no longer say Windows on a phone either |
| "Experimental" everywhere | The status is renamed `ready` throughout — the type, both languages, the labels — and shown as "Ready". The amber boxes that began "Experimental." on the Noise and Equalizer pages are gone. What it means is unchanged, and it still turns to "Verified" on a confirmed change. This applies to the desktop too: it is one interface |
| Multi-device switch, and what is connected | New, on the Device page: Experiments 11 and 12, `bose/bmap/unit.rs` |
| Detect the name, whatever it is | It always was read rather than written in — nothing in the application knows "Aurora". Now it is the headphones' own answer over the control link that is shown and spoken, where before it was the system's record of them. Models other than the QuietComfort Headphones (`0x4075`) are still not driven: none has been tested |
| Change the name | New, on the Device page: Experiment 12 |

### Decisions

| Decision | Why |
| --- | --- |
| "Ready", not just a blank where "Experimental" was | The state is real and worth showing: the headphones are answering, and nothing has been confirmed yet this run. The old word made that sound like doubt about features that have each been proven on the headphones |
| Switching multipoint off is asked about first | It dropped the device that asked, at once, and it could then only be switched back on after reconnecting by hand. A switch that can lock you out of itself says so before it is flipped |
| A switch to "off" counts as confirmed on the headphones' answer alone if they then close the link | They answer with the new state and then go. Waiting for a read-back that cannot come would report a change that happened as unconfirmed |
| Only `06` and `07` are read as multipoint | Upstream's rule for other models reads both as "on". Two values were seen; two are accepted |
| Paired devices are shown by name, and their addresses stop in one module | They are other people's devices. The address is needed to ask for the name and for nothing else, and that exchange is kept out of the wire log |
| Nothing in the device-management block is written | True when this table was written. Later the same day connecting and disconnecting were asked for and added — see the end of this section. Removing a paired device is still not sent by anything |
| The headphones' own name replaces the system's wherever it is known | After a rename Windows keeps the old name until they are paired again; without this the application would rename them and go on using the old name |
| The name limit is 31 bytes, counted as it is sent | Upstream's figure. The headphones' own limit was not looked for — that would be probing |
| A feature that does not answer is left alone for a minute, and the link kept | Headphones that never answer one question would otherwise hold the link for a reply timeout on every refresh |

### Files

| File | What |
| --- | --- |
| `src/bose/bmap/unit.rs` (new) | `read_name`, `set_name`, `read_multipoint`, `set_multipoint`, `read_paired`; `check_name`; 31 tests |
| `src/bose/bmap/link.rs` | `read_unit`, `set_name`, `set_multipoint`; what was read and what was confirmed, in `LinkStatus` |
| `src/bose/real.rs` | The snapshot's `multipoint` and `paired_devices`; the name from the headphones; capabilities for Multipoint and DeviceRename; `reported_name` |
| `src/device/{state,command,traits}.rs` | `MultipointState`, `PairedDeviceState`, `check_device_name`; commands `SetMultipoint`, `SetDeviceName`; trait methods |
| `src/bose/mock.rs` | The simulated device has both, labelled as simulated |
| `src/lib.rs`, `src/watcher.rs`, `src/settings.rs` | `go_by_own_name`, `remember_own_name`, `reported_device_name` |
| `src/bose/bmap/hardware_tests.rs` | `hw_diag_device_settings`, `hw_diag_multipoint_toggle`, `hw_set_multipoint_on`, `hw_diag_rename_and_back` |
| `frontend/src/components/pages/DevicePage.tsx` | The multi-device panel, the rename editor; the page's header stacks on a phone |
| `frontend/src/pages.ts`, `Sidebar.tsx`, `App.tsx`, `TopBar.tsx`, `EqualizerPage.tsx`, `SettingsPage.tsx`, `primitives.tsx` | What a phone leaves out |
| `frontend/src/types/capability.ts`, `store.tsx`, `NoisePage.tsx` and the Rust `device/capability.rs` | `experimental` → `ready` |

### Checked

| What | How | Result |
| --- | --- | --- |
| Name, multipoint and paired devices read from the headphones | `hw_diag_device_settings`, `hw_b9` through the application's code | "Aurora"; on; "Jaisal's S24 Ultra" and "NEMESIS", both connected, the computer marked as the one asking. Nothing but reads sent; nothing about the other devices in the wire log |
| Rename and rename back | `hw_diag_rename_and_back`, `hw_b10` through the application's code | Each confirmed by read-back in about 75 ms; written once each; DeviceRename `VERIFIED`; the new name is the one shown. Left as "Aurora" |
| Multipoint off and on | `hw_diag_multipoint_toggle`, `hw_set_multipoint_on` | Off: `06`, and the computer was dropped. On again after reconnecting by hand: `07`, read back three times. Left on |
| Asking for multipoint as it already is | `hw_b11` | Nothing written |
| The parsing, the write-once rule, the closed-link case, refusals, addresses kept in | `unit.rs`, `link.rs`, `real.rs`, `command.rs`, `mock.rs`, `lib.rs` tests | 563 Rust tests pass |
| What the phone leaves out; the Device page's switch, list, confirmation and rename editor | `mobile.test.tsx`, `DevicePage.test.tsx` | 240 frontend tests pass |
| The pages at phone width with an Android user agent, against the simulated device | Browser preview, 375 × 812 | No Diagnostics in the navigation; the Equalizer page without the software panel or the word Windows; "Ready" on Noise; rename and switch-off with its warning both work; no sideways scroll; no console errors |

### Not checked

- **Anything on a phone.** The multi-device switch, the list and the rename
  were run on the headphones from Windows. The code between the page and the
  wire is the same on Android, and the phone build compiles, but nobody has
  used them there.
- **Multipoint switched off through the application's own code path on the
  headphones.** It was switched off once, with the diagnostic test's raw
  frames; the application's path for it is covered by unit tests using the
  bytes that were seen. Doing it again meant dropping the computer again.
- **Which device the headphones keep** when multipoint goes off, as a rule.
  Seen once.
- What a paired, not-connected device looks like on this model.
- How long a phone or a computer takes to show a new name of their own.
- The desktop application's Device page, by eye. It uses the same page.

### State the hardware was left in

Called **"Aurora"** · multipoint **on** · connected to the phone and the
computer · mode, equalizer and custom level not touched.

### Builds

| | |
| --- | --- |
| Phone | `Setups\Bose-QC-Control-Center-0.1.0-android-arm64.apk`, 2026-10-13 12:50, 139,337,102 bytes, SHA-256 `19471427b0d3cc2a5c16780fcd4375a77eca93e443f50684125ccb2da2e020d4`. Not run by anyone |
| Windows installer | `Setups\Bose QC Control Center_0.1.0_x64-setup.exe`, 2026-10-13 12:52, 105,297,172 bytes, SHA-256 `6c88793cdcc430cb8af5da7521fd58c5910748427da738d7a25c418b04f60dc4`. Not run by anyone |

These replace the ones listed in §15 and §17. Both install over the version
already there.

### Later the same day: the Device page on the phone, and a switch for each device

Asked for by the owner after installing the build above on the phone (which
is the first evidence that it runs there: their screenshot is of its Device
page). Two things: take the card at the top of the Device page off — the
headphones' picture, their name, Reconnect and Disconnect — as saying again
what is already there; and make the multi-device section bigger, with a
switch in front of each device that is on while it is connected and
disconnects it when turned off.

| Asked | Done |
| --- | --- |
| Remove the card | Not shown on Android. A Reconnect button appears on the Connection row when the headphones are not connected, since the card was the page's only way to reconnect. The desktop keeps the card |
| Bigger | The section's title, each device's name and its rows are larger, and each device has an icon tile |
| A switch for each device | Every paired device is listed with one, on while the headphones are connected to it. Moving it sends `START [4.2]` (off) or `START [4.1]` (on) with that device's address — see below |

**The switch for each device has not been run on the headphones.** The
experiment, `hw_diag_paired_disconnect_and_reconnect`, disconnects the
owner's phone for some seconds and was stopped before it ran, for want of
their explicit agreement to that. The requests are an older model's
(Experiment 13 in protocol-notes.md). Everything else about it is tested:

| What | How |
| --- | --- |
| The device is looked up on the headphones' own list by an id, read first, asked once, and read back; a device not on the list sends nothing; an address given as an id reaches nothing | `unit.rs`, `link.rs`, `command.rs` tests |
| Disconnecting the device that is asking is confirmed by the link going, and only then | same |
| The page: each device's switch follows what the headphones report; disconnecting this phone, and connecting another device with multi-device off, are asked about first | `DevicePage.test.tsx`; and by hand in a browser at phone width against the simulated device |

New since the tables above: `CapabilityKey::PairedDevices`, the command
`SetPairedDeviceConnection { device, connected }`, `PairedDeviceState.id`,
`set_paired_connection` in `unit.rs`, `link.rs` and both device backends.
Tests now: 579 Rust, 251 frontend.

Decisions:

| Decision | Why |
| --- | --- |
| A device is named to the native layer by a salted hash of its address | The interface has to say which device, and must not hold addresses. The hash is matched against the headphones' list each time, so it can only ever name something the headphones themselves just listed |
| Disconnecting this phone is asked about first | The application cannot undo it: with the phone dropped there is no link to ask over |
| Connecting another device while multi-device is off is asked about first | The headphones then hold one connection, so it drops this one |
| No request to remove a paired device | Not asked for, and not reversible from here |

Builds with this in them, replacing the two listed just above:

| | |
| --- | --- |
| Phone | `Setups\Bose-QC-Control-Center-0.1.0-android-arm64.apk`, 2026-10-13 13:15, 139,369,870 bytes, SHA-256 `8b0cf866e0b270cb2be0bb86d56946eecbbdf640a87116ae6a7dca1c74a28248`. Not run by anyone |
| Windows installer | `Setups\Bose QC Control Center_0.1.0_x64-setup.exe`, 2026-10-13 13:19, 105,335,116 bytes, SHA-256 `ae7a3574ef032cf81cf1386bcf113182dc6bc939dddab4567cb611ead90cecfd`. Not run by anyone |

### And then: the same on Windows

Asked for by the owner once the phone build had these changes: make every
change that was made for the phone in the Windows build as well — the card
off the Device page, the Diagnostics section gone.

So what had been left out on a phone is now simply not in the application:

| | Before | Now |
| --- | --- | --- |
| Diagnostics page, its button on the Device page, the logging section of Settings | Hidden on Android | Removed. `DiagnosticsPage.tsx` is deleted; `diagnostics` is no longer a page |
| The card at the top of the Device page | Hidden on Android | Removed. Reconnect is on the Connection row, on both, when the headphones are not connected |
| Software EQ panel | Hidden on Android | Removed, with the store state behind it. The Equalizer page is one panel |
| `pagesFor`, `pageTitle` (what each platform has) | Decided per platform | Gone: there is one set of pages |

What still differs by platform is wording that names the host — "This phone"
or "This computer", "Phone volume" or "Windows volume" — and the settings
that only one of them has (start with Windows, the desktop widget, keep
running in the background).

Nothing native changed for this. The event log the Diagnostics page showed
was kept in the interface only, and the part of it the Device page's
timeline uses is still there.

Checked: 255 frontend tests (the "what the application no longer has" cases
now run for a phone and for a desktop), the type check, and by eye in a
browser at desktop width against the simulated device — the sidebar has six
entries, the Device page opens at Device Information, the Equalizer page is
one panel, Settings has no logging section, no console errors. Not checked:
the installed Windows application itself.

Builds, replacing the two listed before this subsection:

| | |
| --- | --- |
| Windows installer | `Setups\Bose QC Control Center_0.1.0_x64-setup.exe`, 2026-10-13 13:57, 105,187,353 bytes, SHA-256 `20a1fc639720b9fa60f934e7dfc0985cd9412b9eb48fac68554935483fbcd92f`. Not run by anyone |
| Phone | `Setups\Bose-QC-Control-Center-0.1.0-android-arm64.apk`, 2026-10-13 13:59, 139,353,486 bytes, SHA-256 `7b040efae243b29bc040d8c7a4cfddb35c1f03bfb68f606cb4c0f79fa807c38d`. Not run by anyone |

## 19. Void, and every Bose headphone and earbud (2026-10-16)

Asked for by the owner, first on Windows, the phone to follow once they have
tried it: rename the application **Void**; support every Bose headphone and
earbud — the QuietComfort generations and the QuietComfort Ultra generations
among them; open by asking **Earbuds or Headphones**, then find the connected
device, work out which model it is, and show that model's features and not
ones it does not have; and read the name its owner gave it.

### What was done

| Asked | Done |
| --- | --- |
| Rename to Void | Product name, executable, window, tray, widget, installer (`Void_<version>_x64-setup.exe`), phone package name and label, voice-model script messages. The identifier `local.bosecontrolcenter.app` is kept, so settings carry over |
| Every Bose headphone and earbud | `bose/bmap/models.rs`: upstream's catalogue, less speakers, each model **tested**, **documented** or **recognized** (README, *Models*). Documented models drive with upstream's description; recognized ones with the addresses the current generation shares, reading their mode list and writing nothing into a mode. The QC35 generation is recognized and not driven |
| Earbuds or Headphones at start | `DeviceChooser.tsx`, on every start and on **Change device**. It lists what the system has paired (`list_bose_devices`, nothing sent to any device), opens the one connected device of that kind by itself, asks when there are several, and says so when there is none |
| Identify the model and show its features | The snapshot carries the model; a feature is hidden where the device answers that it has no such function, where the description has none (the equalizer), where a custom mode's level cannot be written, and for a model that is not driven. Pages and dashboard cards for hidden features go with them |
| Each model's modes | The modes are the device's own list, under its own names — Immersion and Cinema on a QC Ultra — switched by slot (`SelectNoiseMode`) and confirmed by read-back like any switch |
| The owner's name for the device | Unchanged in kind: the name shown is the device's own answer, for any model. The chooser lists devices by that name |
| Earbuds | Left, right and case batteries where the device reports them, and an earbuds picture rather than headphones |

### Decisions

| Decision | Why |
| --- | --- |
| Three degrees of support, shown on the Device page | "Supports every model" is true only in degrees. Saying which degree is the honest form of the claim |
| A model never run here is driven at all | The owner asked for every model. Every change is still only counted when the device reports it, so a model that does not behave as described shows "could not be verified", never a change that did not happen |
| Nothing written into a mode where its layout was not checked on that model | A level write rewrites a mode's whole configuration. A layout guessed from another model could change a different setting of the mode |
| The QuietComfort Headphones (2nd Gen) read-only for mode writes | Upstream gives them the QC Ultra (2nd Gen)'s description and marks it unconfirmed |
| Multipoint, the name and the paired list hidden only on the device's own word | Upstream's description of the tested headphones says they have no multipoint. They do |
| The chooser on every start | Asked for. It sends nothing, and with one device of the kind connected it opens it by itself |
| The QC35 generation not driven | Their noise cancellation is a different function (`[1.6]`) with nothing described of how it behaves |

### Files

| File | What |
| --- | --- |
| `src/bose/bmap/models.rs` (new) | The catalogue, the degrees, `config_for`, the generic description, `parse_mode_config_any` |
| `src/bose/bmap/custom.rs` | A layout per model (47-, 48- and 44-byte mode configurations), written only where checked; `can_adjust` |
| `src/bose/bmap/noise.rs` | `Target::Listed`, `mode_options` |
| `src/bose/bmap/link.rs` | `to_mode`; batteries per part |
| `src/bose/choice.rs` (new) | The paired Bose devices with their models; choosing one by its id |
| `src/bose/real.rs` | The snapshot's model and mode list; `select_noise_mode`; `mark_what_the_model_lacks`; models that are not driven |
| `src/device/{state,command,traits}.rs`, `src/bose/mock.rs`, `src/lib.rs` | `ModelState`, `NoiseModeOption`, `BatteryPart`; `SelectNoiseMode`; `list_bose_devices`, `use_bose_device` |
| `frontend/src/components/DeviceChooser.tsx`, `NoiseModes.tsx` (new) | The chooser; the mode picker |
| `frontend/src/store.tsx`, `App.tsx`, `Sidebar.tsx`, pages, widget, `DeviceVisual.tsx`, `mock/browserDevice.ts` | Model, modes, chooser state; hidden pages and cards; the earbuds picture; three simulated devices for the browser preview |

### Checked

| What | How | Result |
| --- | --- | --- |
| Catalogue, degrees, layouts, mode lists, hidden features, the paired list, batteries per part | Rust unit tests | 611 pass; clippy clean |
| The chooser, the mode picker, hidden pages, the Device page's model and batteries, the simulation | Frontend tests | 277 pass; type check clean |
| The whole flow against the browser preview's simulated devices | By eye at desktop and phone width | Headphones → two connected → choose the QC Ultra → five modes, Cinema with no level, Commute with one; Change device → Earbuds → opened by itself, four modes, no custom mode, Right/Left/Case batteries; no console errors |
| The model, the listed modes and a switch by slot, on the tested headphones | `hw_b12`, through the application's code | Named "QuietComfort Headphones", tested; Quiet, Aware and "Focus" listed with Quiet marked; slot 1 then slot 0 each confirmed by read-back (`START [31.3]` with the slot, nothing written into a mode); slot 9 refused with nothing sent |
| The paired list | `hw_b13`, the system's record only | "Aurora", connected, identified as the QuietComfort Headphones |
| Nothing on the tested headphones hidden by the new rules | `hw_b0`, `hw_b2`, `hw_b4`, `hw_b6`, `hw_b9`, `hw_b11` | All pass; noise control, the custom mode, the equalizer, multipoint and the name all `READY`, none `UNSUPPORTED` |

### State the hardware was left in

Called **"Aurora"** · Quiet, as it was found · multipoint **on** · equalizer
and custom level not touched.

### Not checked

- **Any model but the QuietComfort Headphones.** None was available.
- The phone. The phone's paired list has no product ids, so the chooser there
  would list every device as unrecognized until the phone side is done.
- The installed Windows application. The installer below has not been run.

### Pop-ups removed

Asked for by the owner the same day: no pop-up notices at all. The pop-up area is no longer
rendered (`App.tsx`), so nothing pops up for a change made, unconfirmed or failed. What
the headphones report is still what every control shows, and each outcome is still
recorded in the session's event log (the widget's status line shows the latest).

### The wolf as the application's icon

Asked for by the owner the same day. `icons/app-icon-source.png` is the sidebar's wolf logo
(`frontend/src/assets/wolf-logo.png`) on a 1024-pixel rounded tile, with the image's grey
edge trimmed and a small sparkle mark in its corner blended out; `npx tauri icon` made every
size from it: the Windows icons and the Android launcher icons. The installer is given
the same icon (`installerIcon`), and the tray uses the application's. After changing the
icons, `build.rs` has to be touched: the release build otherwise keeps the old icon
embedded in the executable. Checked by reading the icon out of a fresh copy of the built
executable and of the installer: both are the wolf.

### Builds

| | |
| --- | --- |
| Windows installer | `Setups\Void_0.1.0_x64-setup.exe`, 2026-10-16 15:47, 105,428,881 bytes, with the voice model, without pop-ups and with the wolf icon, SHA-256 `5ecb7e322c05a1613ff170e50ab4751813aeddfecaa8bddd93b1c6dad8191c5c`. Not run by anyone |
| Phone | None for Void yet. `Setups\Bose-QC-Control-Center-0.1.0-android-arm64.apk` is still the 2026-10-13 build of the old application |

The new name installs into a folder of its own (`%LOCALAPPDATA%\Void`) beside
the old one, which stays until it is uninstalled. The identifier is the same,
so settings, profiles and the voice model are found by both — uninstall the
old one without deleting its application data. "Start with Windows" points at
whichever copy last switched it on, so it is switched on again from Void.
