# Source of truth

One document that records what this project is, what was tried, what worked,
what did not, and the exact state it was left in. If something breaks, start
here: every claim below says whether it was **measured**, **tested**, or
**not verified**.

Last updated: 2026-10-12.

- Repository: `https://github.com/Jaisalrathi1829/bose-qc-control-center` (private), branch `main`
- Everything described here is committed and pushed. The commit that holds the code is `55e3684` (see [§12](#12-git-state))

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
| `[1.7]` | Equalizer, 3 bands, −10…+10 | Read and written, confirmed by read-back |
| `[2.2]` | Battery | Measured 2026-10-11: `GET` → `02 02 03 04 0A FF FF 00` = 10 %, Windows showed 10 |
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

### Not verified anywhere yet

- The **phone build (arm64) starting on a real phone**
- **Bluetooth control from Android**: identifying the model, reading or
  changing mode, level, EQ
- Speech routed to the headphones' own output on Android
- Battery level on Android
- Announcements in the background on a phone
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
3. Capability states: UNKNOWN / SUPPORTED / EXPERIMENTAL / VERIFIED /
   UNSUPPORTED. VERIFIED needs a `HardwareProof` built from an observed
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
  "*name* mode activated." · "Aurora connected." · "Aurora disconnected." ·
  "Battery low. *N* percent." · test: "Voice feedback is ready."
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

Key files:

- `app/src-tauri/src/android/` — `bridge.rs` (calls into Kotlin),
  `rfcomm.rs`, `directory.rs`, `sound.rs`
- `app/src-tauri/gen/android/app/src/main/java/local/bosecontrolcenter/app/`
  — `AuroraPlugin.kt`, `AuroraService.kt`, `MainActivity.kt`
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

Output: `app\src-tauri\target\release\bundle\nsis\Bose QC Control Center_0.1.0_x64-setup.exe`

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

- `app\src-tauri\gen\android\app\build\outputs\apk\arm64\release\app-arm64-release.apk`
- Copy kept at `app\src-tauri\target\android\Bose-QC-Control-Center-0.1.0-android-arm64.apk` (133 MB)

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
closed**, one at a time (`--test-threads=1`):

```powershell
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 hw_a hw_b
```

```powershell
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 kokoro_real voice_real voice_hw
```

Lint (two old lints in first-commit lines are allowed):

```powershell
cargo clippy --manifest-path app/src-tauri/Cargo.toml --lib --tests -- -D warnings -A clippy::redundant_closure -A clippy::collapsible_match
```

### Last known-good results (2026-10-11)

| Suite | Result |
| --- | --- |
| Rust app | 432 passed, 0 failed, 38 ignored |
| Frontend | 180 passed; `tsc` clean; `vite build` OK |
| Vendored crate / tools | 103 / 16 / 27 / 32 passed |
| Clippy | clean |
| Hardware (Windows) | `hw_a1–3`, `hw_b0–b7`, `voice_hw_*`, `hw_diag_battery`, `hw_diag_product_id` all pass |
| Android compile | `aarch64` and `x86_64` |

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
| Anything on the phone | `adb logcat -s Aurora` |

To get back to a known-good state, check out `55e3684` (Windows and Android), or `659446f` for the last commit before the Android port (Windows only, confirmed by ear).

---

## 12. Git state

All on `origin/main`, nothing left uncommitted:

| Commit | Holds |
| --- | --- |
| `365b820` | Verified noise control, custom mode, hardware EQ, settings, local voice feedback |
| `659446f` | 150 ms button detection, settle pause removed, Windows installer with the voice model, verbatim-path fix |
| `55e3684` | The Android port (`src/android/`, `gen/android/`, `tools/build-android.ps1`, phone layout), battery over the control protocol, `DeviceError::NotPermitted`, `docs/android.md`, and this file |

Later commits that touch only this document are bookkeeping; `55e3684` is the
one to return to for the code.

Not in the repository, by design: build output (`target/`, `gen/android/app/build/`),
the compiled libraries, the installers and APKs, and the voice model. They are
rebuilt or reinstalled with the commands in [§8](#8-build-run-test--exact-commands).
