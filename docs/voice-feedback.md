# Voice feedback

Spoken announcements of things the headphones have actually done, synthesized
on this computer with [Kokoro](https://huggingface.co/hexgrad/Kokoro-82M) and
played through the headphones' own Windows audio output.

The rule the rest of the application follows applies here too: **nothing is
announced that has not been confirmed.** "Quiet mode activated." is spoken
because the headphones reported Quiet, not because a button was pressed.

## One-time setup

The application contains no code that downloads anything, and nothing is
fetched at start-up. The voice model is installed by running one script, once:

```powershell
powershell -ExecutionPolicy Bypass -File tools\install-voice-model.ps1
```

| | |
| --- | --- |
| Fetches | `kokoro-int8-en-v0_19.tar.bz2` from the sherpa-onnx project's `tts-models` release on GitHub |
| Size | 103,248,205 bytes (98 MB) to download, about 150 MB on disk |
| Checked | SHA-256 `c9f0dd393615805b0bab050c340834d5e684e732aec91c0e860cd30e982c08bd`, before anything is unpacked |
| Installed to | `%LOCALAPPDATA%\local.bosecontrolcenter.app\voice\kokoro-int8-en-v0_19` |
| After that | No network access is needed, or made, by voice feedback |

Then open **Settings → Voice Feedback**. If the application was already
running, press **Check again**. Until the model is there the section says
*Not installed*, shows this command and the folder, and offers no voices.

To remove the model, delete that folder.

### An installer that carries the model

`npm run tauri:build:voice` builds an installer with the model inside (about
100 MB instead of 7 MB), so the installed application can speak with no
separate step. It takes the model from the copy installed above — nothing is
downloaded — and installs it beside the application, in `voice\`. A model in
the per-user folder above is used in preference to the bundled one.

That installer contains eSpeak NG's data files as well as its code; see
`THIRD-PARTY-NOTICES.md` before giving it to anyone else.

## Settings

| Control | Does |
| --- | --- |
| Voice feedback | The master switch. Off by default. Switching it off stops an announcement in progress and drops any that were waiting. |
| Voice | One of the installed model's 11 English voices. Only voices the installed model has are listed. |
| Speed | 0.5× to 2×. |
| Speech volume | 0–100, applied to the speech samples only. The Windows and headphone volume are never changed. |
| Test Voice | Speaks "Voice feedback is ready." in the chosen voice, speed and volume. Works with voice feedback off, so a voice can be chosen first. Shown as *Test* in the status, never as an announcement. |
| Stop Speaking | Stops what is being said and drops everything waiting. |
| Quiet / Aware mode changes | "Quiet mode activated." / "Aware mode activated." (and "*name* mode activated." for a custom mode). |
| Connection and disconnection | "Aurora connected." / "Aurora disconnected.", using the headphones' own name. |
| Low battery | "Battery low. 18 percent." |
| Use another output when the headphones are unavailable | Off by default. See [Where speech goes](#where-speech-goes). |

The status shown is the speech worker's own account of what it is doing:
*Not installed*, *Initializing*, *Ready*, *Synthesizing*, *Playing*,
*Unavailable* (there is nowhere to speak to) or *Error* (the last attempt
failed, with the reason).

Settings are kept in the application's `settings.json` with the others. A value
edited by hand to something impossible — a voice the model does not have, a
speed of zero — is brought back into range when it is read.

## What is announced, and when

### Mode changes

A mode announcement has exactly one source: two successive readings of the
mode, taken from real, connected headphones, that differ.

- Pressing a mode button announces nothing. Neither does the request being
  sent, nor the headphones acknowledging it. The announcement follows the read
  that shows the new mode — and after a change made from the application that
  read is the one the change itself ends with, so the announcement does not
  wait for the screen to refresh.
- Asking for the mode the headphones are already in announces nothing.
- The mode found when the application starts, or when the headphones
  reconnect, is where things stand, not a change. It is not announced.
- If the mode moves on while an announcement is waiting or being synthesized,
  that announcement is dropped. Only the mode the headphones are in is spoken.
- A simulated device is never announced.

The headphones do not announce a mode change themselves: the control channel
is strictly question and answer. So a change made with the button on the
headphones is found only by asking, and while mode announcements are on the
application asks every 150 ms — one four-byte question on the link that is
already open, answered in about 20 ms. A press of the button is therefore
spoken within roughly a fifth of a second. With mode announcements off, it
does not ask.

The price is a little more Bluetooth traffic while the setting is on, and so
possibly a little more battery use on the headphones. That has not been
measured.

A custom mode is announced by the headphones' own name for it — "Focus mode
activated." Its sentence is made ready as soon as the headphones have listed
their modes, the same as the two fixed ones, so the first switch into it is
not kept waiting for synthesis.

#### What used to make the custom mode late

Three things, all of them about the list of modes rather than about speech
(found 2026-10-12, from a report of the announcement lagging on Focus and
sometimes on the mode after it):

1. Listing the modes is the one request answered with several frames, and the
   transport took the answer to be over only after half a second of silence.
   The headphones in fact say when they are done — the captured reply ends
   with `RESULT` from the address that was asked — so the wait was for
   nothing. The reply now ends at that frame (`ends_drained_reply` in
   `bose/bmap/stream.rs`); the silence remains as the fallback for a device
   that does not send one.
2. The list was asked for on every refresh spent in a custom mode, and once
   more before every switch into one. Each asking held the link for that half
   second, and the 150 ms mode checks queued behind it — which is why a press
   of the button while in Focus could be late too. It is now asked for when
   the headphones arrive, and every ten seconds while they are in a custom
   mode.
3. "Focus mode activated." was not among the sentences made ready ahead, so
   the first time in each run it was synthesized on the spot.

The first is a change to how a reply is read and has been run against the
unit tests only, on the reply as it was captured from the headphones in
Experiment 6; it has not yet been timed on the headphones themselves.

### Connection

The watcher looks at Windows' device properties every 4 seconds.

- Only a change between two observations is announced. Whatever state the
  headphones are in when the application starts is not.
- One failed look is not a disconnection: the headphones must be missing for
  two looks running. A burst of failures followed by recovery says nothing.
- "Connected" waits up to 8 seconds for Windows to bring up the headphones'
  audio output, because it is spoken through them.
- "Disconnected" cannot be spoken into headphones that have gone. It is spoken
  only if another output has been allowed, and otherwise not at all.
- "Connected" says what they connected to: "Aurora connected to Nemesis." The
  name is this computer's own, or this phone's Bluetooth name — the machine
  the application is running on, which is the one it saw them connect to.
  Separators in a name are read as spaces ("DESKTOP-8A3F2K1" as "DESKTOP
  8A3F2K1"), a name longer than 40 characters is cut at a word, and if the
  system gives no name the sentence is simply "Aurora connected." Anything
  else the headphones may be connected to at the same time is not named: they
  have not been asked.
- A disconnection also stops anything being spoken into the headphones and
  drops waiting mode and battery announcements, which are no longer true.

### Battery

The level is the one Windows reports for the headphones. Nothing is estimated.

- A warning is spoken when a reading is at or below **20%** and the previous
  state was not already "warned".
- It is not repeated. It re-arms only after a reading above **25%**.
- A missing reading, or one from disconnected headphones, says nothing and
  does not re-arm the warning.

Routine "battery at 60 percent" announcements are not implemented.

## Where speech goes

Speech is played through the audio output that belongs to the headphones,
found by matching the output's device container to the paired headphones' —
not by name, and whether or not it is Windows' default output. The headphones'
hands-free (call) output is never used.

If that output is not available:

- with **Use another output…** off, the application stays silent and the
  status says why. It does not fall back to the computer's speakers on its
  own;
- with it on, Windows' default output is used — and for "disconnected", the
  default output unless that is the headphones.

Playback uses a shared-mode WASAPI stream of its own. No system, endpoint or
session volume is read or written.

## How it works

```
 watcher (presence, battery) ─┐
                              ├─► policy ─► queue ─► worker thread ─► Kokoro ─► WASAPI ─► headphones
 device snapshots (mode) ─────┘   what is     what     one at a       (cache)
                                  worth       waits    time
                                  saying
```

| Module | Role |
| --- | --- |
| `watcher.rs` | Sees the headphones come, go and run low. Knows nothing about speech. |
| `voice/policy.rs` | Turns confirmed observations into announcements; tracks the confirmed mode. |
| `voice/queue.rs` | What is waiting to be said. |
| `voice/mod.rs` | The service and its one worker thread: route → synthesize → play. |
| `voice/store.rs` | Sentences already synthesized, kept on disk. |
| `voice/kokoro.rs` | Kokoro inference through sherpa-onnx. |
| `voice/assets.rs` | Where the model is, and whether it is the right one. |
| `voice/backend.rs` | The real model and the real Windows output, behind one trait. |
| `audio/playback.rs` | Finding the headphones' output and playing samples through it. |

Nothing in the device layer calls the voice service. It is handed observations
after the fact, so a failure in speech cannot affect noise control, the
equalizer or the Bluetooth link.

### One at a time, in order

One thread does all the speaking, so two announcements cannot overlap.
Whatever arrives meanwhile waits:

| Priority | Announcement |
| --- | --- |
| 1 | Connected / disconnected |
| 2 | Mode change |
| 3 | Low battery |
| 4 | *(unused — reserved for routine battery levels)* |
| 5 | Test Voice |

- Among equals, first come first.
- At most one announcement per subject waits: a newer mode replaces an older
  one still waiting, and likewise for connection state and battery.
- A more important announcement does not interrupt one being spoken; it goes
  next. The exceptions are a disconnection, Stop Speaking, and switching voice
  feedback off, which stop speech into the headphones.
- The queue holds at most 8. The one-per-subject rule keeps it well short of
  that; if it were ever full, the least important item gives way, and a
  newcomer no more important than it is turned away.

### When a setting changes

- Voice feedback off, or a kind of announcement off: everything waiting that
  the switch covers is dropped at once, and one of that kind being spoken is
  stopped. A Test Voice in progress is left to finish.
- Voice or speed: applies to whatever is said next.
- Volume: read at the moment playback starts, so it applies to the very next
  sentence, including one already synthesized.

### The model's lifetime

- **Not loaded until needed.** With voice feedback off, starting the
  application loads nothing; the model is first loaded by Test Voice. With it
  on, the model is loaded only for a sentence that has never been synthesized
  in the chosen voice at the chosen speed — on the very first run, and after
  a change of voice — and released again when idle.
- **Checked before every load.** Eight files are compared by size and SHA-256
  against values pinned in the source. A model that is not the expected one is
  refused with a message; the native runtime aborts the whole process when
  handed a file it cannot parse, so it is never given the chance.
- **Loaded once, reused.** One model instance serves every announcement.
- **Released when idle.** After 90 seconds without synthesizing on Windows,
  and after 8 seconds on Android, the model is unloaded. On Android the
  allocator is then told to hand the freed memory back to the system
  (`mallopt(M_PURGE_ALL)`), without which the application goes on being
  counted at its loaded size.
- **Sentences are cached in memory.** The last 16 synthesized sentences (up
  to 80 characters each, about 150 KB apiece) are kept, keyed by text, voice
  and speed.
- **And kept on disk.** Every sentence of up to 80 characters is also written
  to the application's cache folder (`spoken/`) the first time it is made:
  16-bit audio, about 70 KB a sentence, at most 64 files, least recently used
  dropped first. From then on it is read back instead of synthesized, in this
  run and every later one. Each file names the model, voice, speed and
  sentence it was made for and ends with a SHA-256 of everything before it; a
  file that does not check out is deleted and the sentence made again. What
  is kept is exactly what Kokoro produced — nothing else can put speech
  there, and nothing is fetched. Deleting the folder costs nothing but the
  time to make the sentences again.
- **Made ready ahead.** With voice feedback on, the fixed sentences for the
  enabled kinds — and the custom modes the headphones have listed — are made
  ready while idle, so the first real announcement does not arrive seconds
  after the event. After the first run that means reading a few small files.
  A real announcement interrupts this, and the sentence that was interrupted
  is taken up again afterwards. (It used to be dropped, which left that one
  sentence to be synthesized, late, the first time it was needed.)
- **Released on exit**, with playback stopped first.

### When announcements can happen

While the application is running — on screen, minimized, or in the tray. There
is no Windows service and no separate background process: quit the
application and nothing is announced. After the computer wakes from sleep the
watcher simply resumes; a state that is the same as before sleep announces
nothing, and one that differs is announced once.

## Privacy

- Synthesis happens in this process. The speech runtime's DLLs import no
  networking library, and the application's runtime dependencies include no
  HTTP or TLS client. As a check, all 11 voices were synthesized while the
  process was sampled every 250 ms: it held no TCP connection and no UDP
  endpoint at any point. (It has not been run with the network physically
  disconnected.)
- No text is sent to the native layer for speaking. The window can ask for the
  fixed test sentence and nothing else; every other sentence is composed
  natively from a confirmed event.
- Spoken sentences are not written to logs. They are kept on disk as audio,
  in the application's own cache folder (`spoken/`), so that they need not be
  synthesized again; each file also holds the text of its sentence. Those
  sentences are the fixed ones listed above, which include the headphones'
  name, this machine's name, the names of the headphones' custom modes, and
  battery levels that were announced. Nothing else is in the folder, nothing
  reads it but this application, and deleting it loses nothing.
- Nothing is downloaded by the application, at start-up or ever.

## Runtime choice

Three ways of running Kokoro were considered. Only the first was built and
measured; the other two were assessed from their documentation.

| | sherpa-onnx (chosen) | ONNX Runtime via the `ort` crate | Reference Python package as a child process |
| --- | --- | --- | --- |
| Runs in this process | Yes | Yes | No |
| Text → phonemes | Built in (eSpeak NG) | Not included; a phonemizer matched to the model's token table would have to be written or ported | Built in |
| Rust API | Official crate, version-locked to the native library | Yes, for inference only | None; a protocol over pipes |
| What ships with the app | Two DLLs, 22 MB | ONNX Runtime plus the phonemizer | A Python runtime and its packages |
| Long-running helper process | None | None | Needed to avoid a multi-second start per sentence |
| Published, checksummable model package | Yes | Model only | Fetched by the package at run time |
| Assessed by | Building and measuring it | Documentation | Documentation |

sherpa-onnx was chosen because it is the only one of the three that turns text
into speech in-process without this project owning a phonemizer, and without a
second runtime or a helper process.

### Pinned versions

| Component | Version | Where it is pinned |
| --- | --- | --- |
| `sherpa-onnx` crate and native library | 1.13.8 | `Cargo.toml` (`=1.13.8`) |
| Native library archive | `sherpa-onnx-v1.13.8-win-x64-shared-MT-Release-lib.tar.bz2`, SHA-256 `b8eedf41bd6d3779218887b48367bb7a3ece5aaa7667f01f69ee823a12b0a9e7` | Fetched at build time by the crate's build script |
| ONNX Runtime | 1.28.2 | Inside that archive |
| Voice model | `kokoro-int8-en-v0_19` (Kokoro v0.19, 8-bit quantized, 11 voices, 24 kHz) | `tools/install-voice-model.ps1` and `voice/assets.rs` |

Updating the model means updating the checksums in `voice/assets.rs` and the
voice list beside them; a different model is refused until that is done.

The native library is fetched when the application is *built*, by the
sherpa-onnx crate. A built application fetches nothing.

## Measurements

Taken on the development machine (Intel Core i7-14650HX, 16 GB, Windows 11),
debug build, two inference threads, by the `voice_hw_*` tests. They are a
handful of runs, not benchmarks.

| | |
| --- | --- |
| Model load, including verifying eight checksums | 0.9–1.0 s in five runs; 2.5 s in one, the first load after a rebuild |
| Synthesis of "Voice feedback is ready." (1.56 s of audio) | 1.6 s — about real time |
| Test Voice, request to first sound, model not loaded | 2.5 s |
| Same sentence again, request to first sound | 1 ms (cached) |
| Stop Speaking, request to silence | 32–34 ms |
| One mode read on the open link | 22 ms median (10–50 ms over 40 reads) |
| Mode change: request until the headphones confirm it | 0.19 s (1.2 s before the transport's fixed 200 ms pause per request was removed) |
| Headphones' confirmation to first sound, sentence made ready ahead (how the application runs) | 1–3 ms |
| The same with the model loaded but the sentence new | 1.6–1.7 s |
| The same with the model not loaded and nothing made ready | 3.7–5.8 s over three runs (in the one broken down: 1.6 s load, 2.1 s synthesis) |
| Process working set before loading the model | 16 MB |
| Process working set with the model loaded | 265 MB |
| Process working set after the idle unload | 45 MB |

## Tests

The ordinary suite substitutes the model and the audio output, and so proves
scheduling and policy only:

```powershell
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- voice:: watcher::
```

Real Kokoro inference, no audio (needs the model installed):

```powershell
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 kokoro_real
```

Real inference played through the headphones (needs them connected; makes
sound):

```powershell
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 voice_real voice_hw_test_voice voice_hw_stop voice_hw_measurements
```

A real mode change, confirmed by the headphones, being spoken. These switch
the noise-control mode and put it back, and need the application closed — the
headphones accept one control connection:

```powershell
cargo test --manifest-path app/src-tauri/Cargo.toml --lib -- --ignored --nocapture --test-threads=1 voice_hw_mode_change
```

What the hardware tests can show is that speech was synthesized by Kokoro and
rendered to the headphones' Windows output, whose own level meter moved. They
cannot show that a person heard it.

## Verified on the headphones

Run on 2026-10-10 against the paired QuietComfort headphones ("Aurora"), with
the real model and nothing substituted:

- Test Voice, a change of voice, of speed and of volume, and Stop Speaking,
  each rendered to the headphones' own Windows output.
- A switch to Quiet and a switch to Aware, each confirmed by the headphones
  and then spoken; nothing spoken at the request, at an unchanged reading, at
  re-selecting the active mode, or with mode announcements off.

Not verified on hardware: connection and disconnection announcements (the
headphones were not switched off and on), the low-battery warning (the battery
was not near 20%), and anything in an installed build. That a person heard the
speech is not something a test can establish.

## Limitations

- English only: the installed model's voices are American and British English.
- Only "Quiet mode activated.", "Aware mode activated." and the two connection
  sentences are made ready ahead. A custom mode's name and the low-battery
  warning are synthesized when first needed, so the first of each arrives a
  second or two late — longer if the model has to be loaded again.
- The first announcement after a change of voice or speed takes a second or
  two unless the sentence was made ready ahead.
- "Disconnected" is only ever heard through another output.
- A mode change made on the headphones is announced when the application
  next asks, up to 150 ms later, not at the instant it happens.
- Battery levels are as coarse as Windows reports them.
- Windows only.
