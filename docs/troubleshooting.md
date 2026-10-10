# Troubleshooting

## Build

### `error: Missing manifest in toolchain 'stable-x86_64-pc-windows-msvc'`

The rustup toolchain registered but did not fully download. Repair it:

```bash
rustup toolchain uninstall stable-x86_64-pc-windows-msvc
```

```bash
rustup toolchain install stable-x86_64-pc-windows-msvc --profile default
```

### `link.exe not found` / `cl.exe not found`

The VS C++ workload is missing. A Visual Studio install that contains only
`Common7`, `Licenses` and `MSBuild` cannot compile native code. Add the
workload from an **elevated** shell:

```powershell
Start-Process "C:\Program Files (x86)\Microsoft Visual Studio\Installer\vs_installer.exe" -Verb RunAs -Wait -ArgumentList 'modify --installPath "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools" --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --quiet --norestart'
```

Verify:

```powershell
Get-ChildItem "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC" -Directory
```

### `The 'tauri' dependency features on Cargo.toml does not match the allowlist`

A crate feature is enabled that the config does not permit, or vice versa. This
project disables the asset protocol, so the `protocol-asset` feature must stay
off in `Cargo.toml`.

### Exit code 87 from `vs_installer.exe`

`ERROR_INVALID_PARAMETER`. Usually a component id that is not valid for the
installed product version, or a non-elevated shell. Drop explicit component ids
and use `--includeRecommended`, and run elevated.

## Runtime

### The app shows SIMULATED and I have real headphones

The application starts on real hardware when a Bose device is paired with this
PC, and on the simulated device otherwise. If it shows SIMULATED, Windows does
not have your headphones paired — pair them, then press **Use real hardware**
in the banner.

### The app says "No Bose headphones connected" but they are connected

The application is not attached to them. That happens when **Auto-connect** is
off (it waits at launch), or when **Auto reconnect** is off and the headphones
went away and came back. Press **Connect**.

### "Start with Windows" is greyed out

You are running a development build (`npm run tauri:dev`). It cannot start on
its own, because it loads its interface from a dev server. Build the real
application and run that instead:

```powershell
npm run tauri:build
```

### I closed the window and the app is still running

**Minimize to tray** is on: closing or minimizing hides the window to the
system-tray icon. Click the icon to bring it back, or right-click it and
choose **Quit**. Starting the application again also brings it back.

### No notification appeared

Notifications are raised only for real headphones, and only on a change: the
application starting up with the headphones already connected is not a
"connected" event. Low battery is announced once per dip, at 20% or below.
Also check that Windows notifications are not silenced (Focus / Do not
disturb). In a development build, Windows may show them under a different
application name.

### Voice Feedback says "Not installed"

The voice model has not been installed, or its files are not the ones this
build expects. Run the command shown on the Settings page
(`tools\install-voice-model.ps1`), then press **Check again**. The message
names the folder it looked in and, if a file is damaged, which one.

### "Not installed", but the install script said it succeeded

The script was probably run from inside a packaged (Store/MSIX) application —
a terminal embedded in one, for instance. Windows silently redirects such a
program's writes under `%LOCALAPPDATA%` to
`%LOCALAPPDATA%\Packages\<package>\LocalCache\Local\`, so the model exists
only for programs started from that same application. Run the script again
from an ordinary PowerShell window, or copy the
`local.bosecontrolcenter.app\voice` folder from there to
`%LOCALAPPDATA%\local.bosecontrolcenter.app\voice`.

### Test Voice works but nothing is announced

Check, in order: **Voice feedback** is on; the kind of announcement is on; the
application is on real hardware (a simulated device is never announced). A mode
is announced only when it *changes* between two readings — the mode the
headphones are in when the application starts is not announced, and neither is
selecting the mode they are already in.

### Voice Feedback says "Unavailable"

There is nowhere to speak to: the headphones' audio output is not active (they
are disconnected, or Windows has them in call mode only), and **Use another
output when the headphones are unavailable** is off. That switch decides
whether the computer's speakers may be used instead; the application will not
use them on its own.

### "Aurora disconnected." is never spoken

It cannot be spoken into headphones that have just gone. Switch on **Use
another output when the headphones are unavailable** to hear it through the
Windows default output.

### The first announcement is late

The model takes about a second to load and about as long as the sentence to
synthesize it. With voice feedback on, the fixed sentences are made ready ahead
of time and then play at once; a sentence in a newly chosen voice or speed has
to be synthesized once first.

### The application will not start: `sherpa-onnx-c-api.dll` was not found

The installer was built without the speech runtime. Build installers with
`npm run tauri:build`, not a bare `tauri build`.

### Real hardware is greyed out

You are running the browser preview (`npm run dev`), which has no native layer.
Use `npm run tauri:dev`.

### My Bose headphones are not detected

Check in order:

1. Are they paired in Windows Settings → Bluetooth & devices?
2. Are they powered on and connected to *this* machine rather than a phone?
3. Does Windows itself show them?

```powershell
Get-PnpDevice -Class Bluetooth | Where-Object Status -eq OK
```

Detection matches on the Windows friendly name. A renamed device may not match
the Bose hints in `bose/mod.rs`.

### Battery shows "Not reported"

This is honest, not broken. Windows populates the battery property from the HFP
battery indication or the BLE Battery Service. Some devices report only while
audio is streaming; some never report. The application will not invent a value.

Check what Windows itself has:

```powershell
Get-PnpDevice -Class Bluetooth | ForEach-Object { (Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2" -EA SilentlyContinue).Data }
```

If Windows has no value, neither can this application.

### Noise control says "Experimental"

Expected at the start of every session. The badge reflects what this session
has seen: it turns to "Verified" once the headphones have confirmed a switch
to Aware and a switch to Quiet. Use the control once each way.

### Noise control is disabled

The Noise Control page shows the reason under the control. The usual ones:

* **"The control channel could not be validated"** — the headphones are
  connected for audio but did not answer on the control channel. Wait about
  20 seconds (failed attempts are not retried immediately) or switch the
  device source to Simulated and back to Real. If it persists, turn the
  headphones off and on.
* **"Product 0x…. is not a model this application has been tested against"** —
  only QuietComfort Headphones (`0x4075`) are enabled. Other models are
  refused on purpose, not by accident.
* **"Windows does not list Bose's control service (BMAP)"** — remove the
  headphones in Windows Bluetooth settings and pair them again, so Windows
  re-reads their service list.

### "State could not be verified" after changing mode

The request was sent, but the headphones did not report the new mode within
about two seconds. The control keeps showing the last mode they confirmed.
Nothing is retried automatically; try again, and if it repeats, check whether
the mode in fact changed on the headphones.

### Custom is greyed out

"Custom" is the custom mode saved on your headphones, and they have none set
up. Create one in the Bose app, then reopen this page. This application adjusts an existing custom mode; it does
not create one.

### The level slider takes a few seconds

Expected. A level change is written to the headphones and then read back
before the slider moves, and the slider stays where the headphones say it is.
If it springs back to its old position with a warning, the change was not
confirmed.

### The level changed my custom mode in the Bose app

Also expected. These headphones keep the level inside the custom mode itself,
so setting it here changes that saved mode. Nothing else about the mode — its
name, wind block — is touched.

### Hardware EQ is disabled

It is enabled once the headphones have reported their equalizer. If they are
connected and it stays disabled, the reason is shown under the sliders.
"Software EQ" on the same page is a separate thing and is not implemented.
