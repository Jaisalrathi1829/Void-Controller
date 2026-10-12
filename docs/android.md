# Android

The same application, built for an Android phone. The interface and the whole
of the control logic are shared with the desktop build; only the part that
touches the operating system is different.

> **Status.** Two kinds of evidence, kept apart throughout this page.
>
> *Measured here*, on the Android emulator (Android 16): the application
> starts, asks for its permissions, unpacks and verifies the voice model,
> synthesizes speech with Kokoro, plays it, holds its background service,
> reads and sets the media volume, and gives back the voice model's memory.
> The emulator has no Bluetooth device to pair, so nothing about the
> headphones can be measured there.
>
> *Reported by the owner*, from a Galaxy S24 Ultra with the headphones
> (2026-10-12, the build of 2026-10-11): the application runs, connects,
> switches modes including the custom one, and speaks its announcements,
> including with the application in the background. That report is also where
> the problems fixed on 2026-10-12 came from — see
> [What the first run on a phone turned up](#what-the-first-run-on-a-phone-turned-up).
> Nothing on the phone has been instrumented, and the build with those fixes
> has not yet been run on it.
>
> Everything below the bridge — the control protocol, verification, the
> announcement rules — is the code that is hardware-verified on Windows. See
> [What has and has not been verified](#what-has-and-has-not-been-verified).

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
| Volume slider | The Windows volume of the default output | The phone's media volume, in the phone's own steps |
| What "connected to" names | The computer's name | The phone's Bluetooth name |
| How long the voice model stays loaded | 90 s after it was last needed | 8 s, and its memory is then handed back |
| Media keys | Not implemented | Not implemented |

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

### The home-screen widget

A widget for the home screen with the three noise modes — **Quiet**,
**Aware**, and the custom mode under the headphones' own name for it
("Focus") — and under them the equalizer: bass, mid and treble, each with the
level the headphones report and a step down and a step up. Long-press an
empty part of the home screen → **Widgets** → **Bose QC Control Center** →
**Modes and equalizer**.

It is four cells wide and two tall to begin with. Made one cell tall it is
the modes alone, and made taller again the equalizer comes back: which of the
two is drawn follows the room it has (the equalizer needs about 150 dp of
height and 220 dp of width). A widget placed before the equalizer was added
stays one cell tall, and so stays as it was, until it is stretched.

What a tap does depends on whether the application is being run at that
moment:

| The application is | The widget shows | A tap on a mode | A tap on a step |
| --- | --- | --- | --- |
| On screen, or off screen with **Keep running in the background** on | The headphones' name, "Connected" or "Not connected", the reported mode highlighted, the reported levels | Switches the mode there and then. The application does not come up | Moves that band one decibel there and then |
| Off screen with **Keep running in the background** off | "Tap to open the app", no mode, a dash for each level | Opens the application and asks it for that mode | Opens the application. Nothing is changed |
| Closed | "Tap to open the app", no mode, a dash for each level | Opens the application and asks it for that mode | Opens the application. Nothing is changed |
| On its simulated device | "Simulated device in use", no mode, a dash for each level | Nothing that shows: the simulation's modes are not the headphones' | Nothing |

A tap on the widget's top line opens the application.

The rules it keeps are the application's:

- It highlights the mode the headphones **report**. After a tap the button
  is outlined and the line reads "Switching…"; the highlight moves only when
  the headphones confirm. If they do not, the line says how it went — "Not
  confirmed", "Refused by the headphones", "Not available", "Could not reach
  them" — and the highlight stays where they say it is.
- It shows no mode it cannot vouch for. Android stops an application that is
  off screen where it stands unless a service is keeping it going; from then
  on nothing would notice the mode being changed from the headphones' own
  button. So with the background switch off, the mode is taken off the widget
  the moment the application leaves the screen.
- A change made from the widget is announced like any other, and a change
  made from the headphones' button or the application shows on the widget.
- A level on the widget is what the headphones last reported, and a dash
  when they have reported none. A step is a direction, never an amount: the
  native layer reads the equalizer from the headphones at that moment, moves
  the one band by one decibel, sends the other two back exactly as it just
  read them, and counts the change only when the headphones then report it.
  So a step is always from what the headphones have, not from what the
  widget happened to be showing, and several quick taps add up. A step that
  would pass the end of the range (−10 to +10) sends nothing, and the line
  says "Already at the limit".
- A step is not carried into an application that is not running. A mode is —
  "Quiet" means the same whenever it arrives — but "one more" only means
  something beside the level it was tapped on, and with the application
  closed the widget shows none.
- Nothing on the headphones changes the equalizer, but another phone or
  computer they are connected to can. While a widget with the equalizer on
  it is on a home screen, the native layer reads the equalizer again when
  what it has is five minutes old — one question over the link that is
  already open. With no such widget it reads nothing on the widget's account.

How it works: the widget (`ModeWidget.kt`) decides nothing. A tap leaves a
name for the native layer — a mode, or a band and a direction — which has one
thread waiting for such requests (`android/widget.rs`) and puts each through
the same verified change as the application's own controls. That the native
layer is waiting is how the widget knows it is there. The widget's receiver
is not exported, and a request is one of three modes or one of three bands up
or down — anything else is dropped.

Taps are taken one after another as fast as they come. (In the first version
of the widget each tap held Android's delivery of the next for two and a half
seconds; that was found and removed when the equalizer was added.)

For the widget to work by itself, then: **Keep running in the background**
on, and the application left open in the recent-apps list.

### The volume slider

It is the phone's media volume — the one the volume keys change while music
is playing, and so the level the headphones play at while they are the
phone's media output. It is Android's, not a value read from the headphones,
and the slider is labelled with what media is playing through at that moment
("Aurora", "Phone speaker").

Android's volume moves in steps, fifteen on most phones. The slider sets the
nearest step, and counts the change as applied when Android then reports that
step; what it shows afterwards is the step, so asking for 50% shows 53%. No
volume panel is brought up, and nothing else is changed: speech volume is
still applied to the speech alone.

### Memory

The voice model is most of what the application can weigh. Loaded, it takes
about a quarter of a gigabyte; and as first built, the application loaded it
at every start, to make its sentences ready, and kept it for 90 seconds after
each use.

It is now needed only for a sentence that has never been said in the chosen
voice: every sentence is kept as audio the first time it is made (see
[voice-feedback.md](voice-feedback.md#the-models-lifetime)), and read back
from then on. When the model is needed, it is released 8 seconds after its
last use, and the allocator is told to return the freed memory to Android.

Measured on the emulator (x86-64, Android 16) with `dumpsys meminfo`, voice
feedback on. PSS is the figure Android's own memory screens are built from.

| | Before | After |
| --- | --- | --- |
| Application process, a few seconds after starting | 300–355 MB (model loaded to prepare sentences) | 115–128 MB (model never loaded) |
| …of which native heap | 190–240 MB | 14–26 MB |
| Application process, in the background for 25 s | 355 MB | 119 MB |
| Application process, first start after installing | the same | 316–356 MB for about 25 s while the sentences are made for the first time, then 129 MB |
| The web view's own process, counted with the application | 91–96 MB | 80–106 MB |

Roughly 450 MB before and 210–230 MB after, between them. The web view's share
is its own code and is not something this application can reduce short of
tearing the interface down whenever it is off screen, which it does not do.

These are emulator figures. On a phone the proportions should hold — the
model is the same size everywhere — but the numbers have not been taken
there. `adb logcat -s Aurora` shows the model coming and going, with the
native heap at each point:

```
voice model loaded: native heap 214 MB
voice model released: native heap 39 MB
```

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
Setups\Bose-QC-Control-Center-0.1.0-android-arm64.apk
```

That is a copy, put at the top of the project so that it is easy to find; the
build's own output is
`app\src-tauri\gen\android\app\build\outputs\apk\arm64\release\app-arm64-release.apk`.

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
adb install -r Setups\Bose-QC-Control-Center-0.1.0-android-arm64.apk
```

Before first use:

- Pair and connect the headphones to the phone in Android's Bluetooth
  settings.
- Close the Bose app if it is running. The headphones accept one control
  connection from a phone, and whichever application opens it first has it.
- The first time voice feedback is used, the model is unpacked from the
  package (about 150 MB), which takes a few seconds.
- For the [home-screen widget](#the-home-screen-widget): open the application
  once, then add "Modes and equalizer" from the home screen's widget list.

## What has and has not been verified

| | Status |
| --- | --- |
| Control protocol, verification, announcement rules | Verified on the test headphones, on Windows |
| Android-specific logic that is plain code (addresses, service recognition, product-id parsing, which routes are tried, picking the device) | Unit-tested |
| The package builds for phones (`aarch64`) and holds the native libraries and the model | Yes |
| Starts; native layer and Kotlin bridge talk to each other | Verified on the emulator (x86-64 build, Android 16) |
| Media volume: read, set to a step, read back | Verified on the emulator: the slider moved Android's own media volume from step 5 to step 12 of 15 (`cmd media_session volume --get`), and then showed 80% |
| Sentences kept on disk and said without the model | Verified on the emulator: on the second start the log has no "voice model loaded" line at all |
| The model's memory given back after use | Verified on the emulator (figures under [Memory](#memory)) |
| The new dashboard animation and page changes draw correctly in Android's web view | Verified on the emulator, by eye from screenshots. Smoothness cannot be judged there: the emulator draws in software |
| Bluetooth permission: refused, said so, asked again from the application, granted, carried on by itself | Verified on the emulator |
| Voice model unpacked from the package and its checksums verified | Verified on the emulator |
| Kokoro synthesis and playback (Test Voice) | Verified on the emulator: Android's audio log shows the application playing a speech track |
| Refusing to speak through the phone when the headphones are absent and no other output is allowed | Verified on the emulator |
| Background service | Verified on the emulator: running in the foreground, type `connectedDevice`, with its notification |
| Home-screen widget: offered by the launcher, placed, drawn | Verified on the emulator: listed as "Modes and equalizer 4 × 2", dragged to the home screen |
| Widget drawn with the equalizer at two cells tall, and as the modes alone at one | Verified on the emulator, by dragging its resize handle both ways |
| Widget equalizer steps with the application kept running in the background | Verified on the emulator as far as the Bluetooth link: all six buttons, each `asked for` and `taken up` by name |
| Quick taps on the widget | Verified on the emulator: nine taps in 0.45 s, each received within about 40 ms and all nine taken up in order. Before the fix they were received one every 2.5 s |
| An equalizer step with the application's process gone | Verified on the emulator: the application was opened and no step was taken up |
| The widget's equalizer **against the headphones**: levels shown, a step changing a band | **Not verified** — the emulator has no headphones. The read and the write it calls are the ones the application's own equalizer uses |
| Widget tap with the application kept running in the background | Verified on the emulator as far as the Bluetooth link: `widget: aware asked for`, `widget: aware taken up` 1 ms later, the widget told the outcome 6 ms after that, the application still off screen |
| Widget with the application off screen and not kept running | Verified on the emulator: "Tap to open the app", and a tap on a mode opened the application, which took the request up |
| Widget tap with the application's process gone | Verified on the emulator: the process was ended, the tap started a new one, which opened the application; the request was taken up 0.8 s after the tap |
| Widget on the simulated device | Verified on the emulator: "Simulated device in use", no mode |
| The widget on the phone, with the headphones | Reported working by the owner (S24 Ultra, build of 2026-10-12 22:30): "it works … perfectly fine". Not instrumented, and not gone through state by state — which of the states in the table above were tried is not known |
| The phone build (`aarch64`) starting on a phone | Reported working by the owner (S24 Ultra, build of 2026-10-11). Not instrumented |
| Bluetooth socket to the headphones; identifying the model; reading and changing modes | Reported working by the owner: modes, including the custom one, were switched and announced. Not instrumented |
| Announcements in the background on a phone | Reported working by the owner |
| Speech routed to the headphones' own output | Announcements were heard; which output Android used was not checked |
| Custom-mode level and equalizer from Android | **Not verified** — not mentioned either way |
| Battery level from Android | **Not verified** |
| The volume slider moving the *headphones'* volume | **Not verified** — true only if they are the phone's media output, which the emulator cannot be |
| "Connected to" showing the phone's name; the spoken "connected to …" | **Not verified** on a phone. The name lookup itself runs on the emulator |
| The build of 2026-10-12 (everything on this page marked as fixed) on a phone | **Not verified** |

## What the first run on a phone turned up

Five things, reported on 2026-10-12. What was found for each, and what was
done:

| Reported | Found | Done |
| --- | --- | --- |
| About 500 MB of memory with the application in the background | The voice model: loaded at every start and kept for 90 s after each use. Measured on the emulator at 300–355 MB for the application plus ~95 MB for its web view | Sentences kept on disk so the model is rarely loaded; released after 8 s; freed memory returned to Android. [Memory](#memory) |
| The revolving headphones on the front screen glitching | Several things at once. Every layer was driven from script on each frame, so it stuttered whenever the page was busy — which it was every five seconds, re-reading the headphones. A `filter` was animated, a layer was blended with `mix-blend-mode`, and a rounded `overflow: hidden` did the trimming. Behind every page two glows a third of the screen wide were blurred by 120 px on each frame, under a frosted header. And any change in connection state rebuilt the page, restarting the animation — with a transition that could be left stuck fully transparent, and panels that stayed invisible until script got round to fading them in (on the emulator, a blank dashboard for seconds after connecting) | All of it is now CSS animation of `transform` and `opacity` on layers that are otherwise left alone; no animated filters, no blending, nothing that needs trimming; the glows are plain gradients, and on a phone they stay still rather than each taking a screen-sized layer to drift on; the header is opaque on a phone; a page is only rebuilt when it is replaced by another, and pages and panels appear by CSS, so none can be left waiting on script |
| The volume slider said it was for Windows only | It was | [The volume slider](#the-volume-slider) |
| Say which device the headphones are connected to | — | The phone's name is shown under the headphones' on the dashboard and on the Device page, and spoken: "Aurora connected to …". On Windows it is the computer's name |
| Voice prompt late on the custom mode, and sometimes on others | Reading the list of modes held the control link for half a second each time, and was done on every refresh spent in the custom mode and before every switch into it; and the custom mode's sentence was never made ready ahead | [voice-feedback.md](voice-feedback.md#what-used-to-make-the-custom-mode-late) |

The interface also no longer re-reads the headphones while it is off screen:
the native layer goes on watching and announcing by itself, and the screen
catches up when it is shown again.

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
- Asking the headphones for their mode every 150 ms keeps the Bluetooth link
  busy for as long as mode announcements are on, on screen or off. That is
  what makes a press of their button audible at once, and it is not free:
  its effect on the phone's and the headphones' batteries has not been
  measured. Switching mode announcements off stops it.
- Only the phone itself is named as what the headphones are connected to.
  They can be connected to a second device at the same time, and they have
  not been asked about it.
- To see what the application is doing on a connected phone:
  `adb logcat -s Aurora`. It writes a line when it identifies the headphones'
  model, when a route to them opens, when one fails, when the voice model is
  loaded or released (with the native heap at that moment), and where it took
  the phone's name from — not the name itself.
