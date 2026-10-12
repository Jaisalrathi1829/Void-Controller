# Android

The same application, built for an Android phone. The interface and the whole
of the control logic are shared with the desktop build; only the part that
touches the operating system is different.

> **Status.** The application has been run on the Android emulator (Android
> 16), where it starts, asks for its permissions, unpacks and verifies the
> voice model, synthesizes speech with Kokoro, plays it, and holds its
> background service. It has **not** been run against real headphones from a
> phone: the emulator has no Bluetooth device to pair, so the Bluetooth socket,
> the choice of the headphones' audio output, and the battery level are
> unverified on Android. Everything below the bridge — the control protocol,
> verification, the announcement rules — is the code that is hardware-verified
> on Windows. See [What has and has not been verified](#what-has-and-has-not-been-verified).

## What is the same

- Reading and switching Quiet, Aware and the custom mode; the custom mode's
  level; the three-band equalizer. Every change is still confirmed by reading
  it back from the headphones, and still reported as unverified when it is
  not.
- Voice feedback, with the Kokoro model inside the package. Nothing is
  downloaded by the application, on first run or ever.
- Connection, disconnection and low-battery notifications.
- The rule that the interface cannot send arbitrary Bluetooth bytes. The web
  view is granted no access to the Android bridge; it reaches hardware only
  through the same typed commands as on the desktop.

## What is different

| | Windows | Android |
| --- | --- | --- |
| Carrying bytes to the headphones | Winsock RFCOMM | `BluetoothSocket`, in `AuroraPlugin.kt` |
| Is it paired, is it connected | PnP device properties | The paired-device list and the media / hands-free profiles |
| Which model it is | Product id from PnP | Asked of the headphones: `GET [0.3]` |
| Battery level | PnP property | The level Android holds; failing that, asked of the headphones: `GET [2.2]` |
| Speech output | WASAPI stream on the headphones' endpoint | `AudioTrack` with the headphones' output preferred |
| Staying alive off screen | System tray | A foreground service, with the notification Android requires |
| Start with the system, start minimized | Yes | Not offered |
| System volume, media keys | Windows Core Audio | Not implemented |

Two of those rows rest on measurements made on the test headphones on
2026-10-11, on Windows, where they could be compared with what the system
reports:

- `GET [0.3]` answered `40 75 0D`: product 0x4075, the id Windows gives.
- `GET [2.2]` answered `0A FF FF 00`: 10 percent, with Windows showing 10.

### Finding the headphones

Android gives applications neither a vendor id nor a product id for a paired
device. A device is a candidate if Android's record of it lists Bose's control
service — the same test as on the desktop — and only failing that if its name
says Bose. Its model is then whatever it answers to the product-id request,
and only a model on the tested list gets a control connection. A name alone
never opens one.

### Keeping the application running

Android freezes an application that is not on screen. With **Keep running in
the background** on (Settings → General), a foreground service holds the
process, and the watcher goes on noticing mode changes, connections and the
battery, and announcing them. Android shows a notification for as long as
that is on. Swiping the application away from the recent-apps list stops it.

With the switch off, announcements happen only while the application is on
screen.

### Permissions

| Permission | Why |
| --- | --- |
| Nearby devices (`BLUETOOTH_CONNECT`) | Seeing paired devices and talking to the headphones. Asked for at first start. |
| Notifications (`POST_NOTIFICATIONS`) | Connection and low-battery notifications, and the background notification. |
| `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_CONNECTED_DEVICE` | The background service. |
| `INTERNET` | The web view loads the interface from inside the package through a local address, and Android will not let it do that without this permission. The application contacts no server. |

If Bluetooth access is refused, the application says so and offers to ask
again; it does not show "no headphones connected", which would be untrue.

## Building

Needs, beyond the desktop requirements: Android Studio (for its JDK and the
Android SDK), the Android NDK, and the Rust target `aarch64-linux-android`.

```powershell
powershell -ExecutionPolicy Bypass -File tools\build-android.ps1
```

or `npm run android:build`. The script finds the toolchain, packs the
installed voice model into the project (run `tools\install-voice-model.ps1`
first; nothing is fetched by the build script itself), and builds an APK for
64-bit ARM phones:

```
app\src-tauri\gen\android\app\build\outputs\apk\arm64\release\app-arm64-release.apk
```

It is signed with the Android debug key, which is enough to install it on your
own phone. Pass `-NoVoice` to build without the model.

To run it on the Android emulator on a PC, build for the emulator's own
processor instead:

```powershell
powershell -ExecutionPolicy Bypass -File tools\build-android.ps1 -Target x86_64
```

The phone build will install on an emulator, which can translate ARM code, but
it will not start there: ONNX Runtime's processor detection crashes under that
translation, as the application is being loaded. This is a property of the
emulator, not of phones.

The first build downloads Gradle, the Android build plugins and their
libraries, and the sherpa-onnx Android runtime (the same 1.13.8 release as the
Windows one).

### Windows without Developer Mode

The Tauri CLI places the compiled library in the Android project with a
symbolic link, which Windows only lets an ordinary user create with Developer
Mode on. The build script does not need it: when the link is refused, it
copies the library instead and runs Gradle directly.

## Installing

Copy the APK to the phone and open it (Android will ask to allow installing
from that source), or with USB debugging on:

```powershell
adb install -r app\src-tauri\gen\android\app\build\outputs\apk\arm64\release\app-arm64-release.apk
```

Before first use:

- Pair and connect the headphones to the phone in Android's Bluetooth
  settings.
- Close the Bose app if it is running. The headphones accept one control
  connection from a phone, and whichever application opens it first has it.
- The first time voice feedback is used, the model is unpacked from the
  package (about 150 MB), which takes a few seconds.

## What has and has not been verified

| | Status |
| --- | --- |
| Control protocol, verification, announcement rules | Verified on the test headphones, on Windows |
| Android-specific logic that is plain code (addresses, service recognition, product-id parsing, which routes are tried, picking the device) | Unit-tested |
| The package builds for phones (`aarch64`) and holds the native libraries and the model | Yes |
| Starts; native layer and Kotlin bridge talk to each other | Verified on the emulator (x86-64 build, Android 16) |
| Bluetooth permission: refused, said so, asked again from the application, granted, carried on by itself | Verified on the emulator |
| Voice model unpacked from the package and its checksums verified | Verified on the emulator |
| Kokoro synthesis and playback (Test Voice) | Verified on the emulator: Android's audio log shows the application playing a speech track |
| Refusing to speak through the phone when the headphones are absent and no other output is allowed | Verified on the emulator |
| Background service | Verified on the emulator: running in the foreground, type `connectedDevice`, with its notification |
| The phone build (`aarch64`) starting on a phone | **Not verified** — it is the same code built for another processor |
| Bluetooth socket to the headphones from Android | **Not verified** |
| Identifying the model, reading and changing modes, level and equalizer from Android | **Not verified** |
| Speech routed to the headphones' own output | **Not verified** |
| Battery level from Android | **Not verified** |
| Announcements in the background on a phone | **Not verified** |

## Known limits

- One model is supported, as on the desktop: QuietComfort Headphones
  (product 0x4075).
- Whether the headphones accept a control connection from the phone while
  also connected to a computer running the desktop application has not been
  tried.
- The package is large (about 130 MB), almost all of it the voice model, and
  the unpacked model takes a further 150 MB on the phone.
- Tauri's Android runtime includes an HTTP client that it uses only to reach
  a development server in development builds. This application's own code
  makes no network request, and its interface is forbidden to by its content
  security policy; but the statement made for the desktop build — that no
  HTTP client is among the dependencies — does not hold on Android.
- Speech output has no level meter on Android, so the "meter moved" evidence
  the desktop tests rely on is not available there.
- To see what the application is doing on a connected phone:
  `adb logcat -s Aurora`. It writes a line when it identifies the headphones'
  model, when a route to them opens, and when one fails.
