# Security

## Principles

1. The frontend is treated as untrusted. It receives the minimum capability
   needed to render.
2. Nothing leaves the machine. Ever.
3. Device identifiers are not exposed, even to the local UI.
4. The application writes to Bluetooth hardware only through the narrow,
   typed operations listed under "Hardware safety".

## No arbitrary Bluetooth writes

The command enum in `device/command.rs` is the complete set of operations the UI
can request. It contains no variant carrying a UUID, a characteristic handle, a
service identifier, or a byte array.

This is enforced by construction, and additionally pinned by a test —
`command_json_surface_contains_no_raw_byte_payloads` — which serializes command
samples and asserts the JSON contains no `uuid`, `bytes` or `payload` key. A
future contributor adding a raw-payload variant has to break that test.

Every command is validated in `DeviceCommand::validate()` before any backend
sees it, for both mock and real paths, so the mock exercises the same checks as
hardware.

## Tauri permissions

`capabilities/default.json` grants `core:default`, a few explicit window
operations, and notifications. It does not grant shell access, filesystem
access, or HTTP.

The desktop widget is a second window and has a capability file of its own,
`capabilities/widget.json`, which grants it less than the main window: the
application's own events, and being dragged. It loads the same page under
the same content security policy, and reaches the headphones through the same
typed commands; there is nothing it can ask for that the main window cannot.
The one command added for it, `show_main_window`, takes no arguments and
brings this application's own window forward.

The asset protocol is disabled in `tauri.conf.json`, and the corresponding
`protocol-asset` crate feature was removed rather than enabling the protocol to
match it.

`freezePrototype` is on. The CSP forbids everything except self, with
`object-src 'none'`, `base-uri 'none'` and `form-action 'none'`. `connect-src`
permits only Tauri's IPC — there is no origin the app could reach even if a
network call were introduced.

## Privacy

| Data | Handling |
| --- | --- |
| Bluetooth address / PnP instance id | Stays in the native layer. Never sent to the UI. The preferred device's instance id is kept in the local settings file so the same headphones are used next time; the UI is given only their name. |
| Settings | `settings.json` in the application's config folder. Local only. |
| Start with Windows | One entry in the current user's startup list (`HKCU\...\Run`), added and removed only by the switch. Per-user; no elevation. |
| Notifications | Raised locally through Windows. Nothing is sent anywhere. |
| Spoken announcements | Synthesized in this process by a local model. The text is composed natively from confirmed events, is not logged, and is not written to disk; synthesized audio is cached in memory only. |
| Voice model | Installed by an explicit script, never by the application. Checked against pinned SHA-256 values before every load. |
| Device id shown in UI and reports | Per-installation salted SHA-256 prefix |
| Diagnostic captures | Local files, git-ignored |
| Telemetry / analytics | None |
| Network requests | None |
| User accounts | None |

`util::stable_id()` is covered by `stable_id_does_not_leak_the_raw_identifier`,
which asserts the raw address does not appear in the output.

`.gitignore` excludes `device-report.json`, `device-report.txt`, `captures/`,
`reports/` and `*.btcapture` so device data cannot be committed by accident.

## Voice feedback

Speech adds a native library and a large data file to the application, so it
is held to the same constraints as everything else.

- **The window cannot make the application say arbitrary things.** The only
  speech command the UI has takes no argument and speaks one fixed sentence.
  Every other sentence is built natively from a confirmed device event, and
  the only variable parts are the headphones' name and a battery percentage.
- **Nothing reaches the headphones but audio.** Speech is played through the
  ordinary Windows audio output. It is not sent over the control link, and
  nothing about voice feedback writes to the headphones.
- **No network.** The speech runtime's DLLs (`sherpa-onnx-c-api.dll`,
  `onnxruntime.dll`) import no networking library, and no HTTP or TLS client
  is among the application's runtime dependencies. The model is fetched by
  `tools/install-voice-model.ps1`, run by hand, which verifies a pinned
  checksum before unpacking.
- **The model is verified before the native runtime sees it.** That runtime
  aborts the process on a file it cannot parse. Eight files are checked by
  size and SHA-256 before every load, and a mismatch is a message on the
  Settings page.
- **The right DLL is the one loaded.** Windows ships an older
  `onnxruntime.dll` in `System32`. The application's own copy sits beside the
  executable, which Windows searches first.
- **No system volume is touched.** Speech volume is a gain applied to the
  speech samples.

## Android

The Android build keeps the same boundary in a different place.

- **The bridge is not reachable from the interface.** Bluetooth, audio and the
  background service are reached through one Kotlin class, registered as a
  Tauri plugin so that native code can call it. The web view's capability
  file grants no permission for that plugin, so the interface cannot invoke
  it: its only route to hardware is still this crate's typed commands. The
  bridge has a "send these bytes" method — it has to, to carry the protocol —
  and that is exactly why it is native-only.
- **The model is identified by asking, never assumed.** Android supplies no
  product id, so the headphones are asked for theirs, and only a model on the
  tested list gets a control connection.
- **Permissions are the minimum that works**: nearby devices, notifications,
  the foreground-service pair, and `INTERNET` — which the web view needs in
  order to load the interface from inside the package, and which nothing in
  this application's own code uses. The interface's content security policy
  forbids it any connection but the local IPC.
- **A caveat.** Tauri's Android runtime links an HTTP client for reaching a
  development server in development builds. So "no HTTP client among the
  dependencies", true of the desktop build, is not true of the Android one.
- **Signing.** The build script signs with the Android debug key so the
  result can be installed on the builder's own phone. That key is not a
  secret and identifies nobody; a build for anyone else needs its own.

## Logging

Structured logging with explicit levels. Ordinary logs record command *names*
only — `DeviceCommand::name()` deliberately returns a static string with no
payload detail, so parameters cannot reach normal logs. Detailed diagnostic
logging is opt-in.

Never logged: credentials, tokens, secrets, raw Bluetooth addresses.

## Hardware safety

Writing to a device is the exception, and it is kept narrow.

- **Three kinds of state-changing request exist.** A mode switch — to
  Quiet, to Aware, or to one of the model's editable custom slots — a write of
  a custom mode's level, and a write of the three equalizer gains (each
  bounded to -10..+10). All are built by the vendored upstream codec. Nothing
  in the application assembles a frame from caller-supplied bytes: the IPC
  request types admit a mode name (`quiet`, `aware`, `custom`) and a level
  from 0 to 10, and an index, a slot number, a frame or a byte string does not
  deserialize. Which custom slot "custom" means is decided natively from the
  list the headphones report.
- **A level write may change one byte.** A custom mode's configuration is
  written back whole, so before it goes out it is compared against what the
  device just reported and must differ in the level and nothing else. Only
  the mode the headphones are in is edited. Modes are never created, renamed
  or deleted.
- **They go only to a model on the allowlist** (`bose/bmap/mod.rs`), that
  Windows shows advertising the BMAP service, on a channel that has answered a
  read-only firmware request correctly, after a mode read in exactly the
  documented shape.
- **Channel selection is bounded.** The channel Windows resolves, then the
  channels upstream documents for the model. No scanning.
- **No authentication is attempted or bypassed.** The protocol as used needs
  none; the link is the one pairing already established.
- Everything else behind the vendor protocol — wind block, noise control
  "off" — returns `Unsupported` with a reason and sends nothing.
- No firmware operations of any kind exist in the codebase.
- No fuzzing, brute-forcing, or authentication bypass.
