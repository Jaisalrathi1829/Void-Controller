# Protocol Notes

**Status (2026-10-10): noise control works on the test headphones. Switching
between Quiet, Aware and the user's custom mode, changing that custom mode's
level, and reading and changing the three-band equalizer have each been
confirmed by the device's own read-back — see
[Experiment 5](#experiment-5--quietaware-control-over-bmap),
[Experiment 6](#experiment-6--the-custom-mode-and-its-level) and
[Experiment 7](#experiment-7--the-hardware-equalizer). The protocol is
Bose's BMAP. Its implementation is vendored from
[aaronsb/bosectl](https://github.com/aaronsb/bosectl), not derived here.**

This document is a log, kept in order. Experiments 1–4 are left as they were
written, including the conclusions that turned out to be wrong; each carries a
dated correction. Two mistakes ran through all four, and both were this
project's own:

1. **The wrong service.** `SRfcomm` was taken to be the control channel, and
   the BMAP service UUID was dismissed as a Windows placeholder. It is the
   other way round.
2. **Frames one byte short.** A bug in this project's snoop-log parser dropped
   the last byte of most frames, so the "observed" frames Experiment 4 replayed
   were not the frames Bose Music had sent.

## Observed device — 2026-08-08

A real Bose QuietComfort, renamed by its owner to **"Aurora"**, connected to
the development machine over Bluetooth Classic.

| Property | Observed value |
| --- | --- |
| SIG company id | `0x009E` (Bose Corporation), on every profile child node |
| Product id | `0x4075` |
| Transport | Bluetooth Classic only — **no `BTHLE` node exists** |
| Battery | **30%**, via the Windows PnP battery property |
| Audio endpoints | `Headphones (Aurora)` (A2DP), `Headset (Aurora Hands-Free)` (HFP) |

### Services Windows enumerated

| UUID | Service |
| --- | --- |
| `0000110B` | A2DP Audio Sink |
| `0000110C` | AVRCP Target |
| `0000110E` | AVRCP Controller |
| `0000111E` | Handsfree (HFP) |
| `{9B26D8C0-A8ED-440B-95B0-C4714A518BCC}` | **Vendor-specific RFCOMM** (`SRfcomm`) |
| `{00000000-DECA-FADE-DECA-DEAFDECACAFF}` | ~~Windows placeholder for a service it has no driver for~~ **Bose BMAP** (corrected 2026-10-10) |

> **Correction, 2026-10-10.** The last two rows were misread.
> `00000000-DECA-FADE-DECA-DEAFDECACAFF` is not a Windows placeholder: it is
> the UUID Bose devices advertise for BMAP, their control protocol, and
> upstream uses it to recognise a BMAP-capable device. `SRfcomm` is a different
> service that accepts a connection and does not speak BMAP. The device's SDP
> table, read on 2026-10-10, is in Experiment 5.

### What this establishes, and what it does not

**Establishes:**

* The device is identifiable by vendor id rather than name. This matters — the
  unit is renamed "Aurora" and matches none of the Bose name hints, so
  name-based detection found nothing at all. Vendor id detection found it
  immediately.
* Battery is readable, but **only from the HFP child node**. The top-level
  `BTHENUM\DEV_...` node reports no battery. Code that reads only top-level
  nodes — as this project's did until this session — misses it entirely.
* A vendor-specific RFCOMM service exists and is almost certainly the control
  channel Bose Music uses. *(Wrong — see the correction above. The service
  exists; it is not the control channel.)*

**Does not establish:** anything about what that RFCOMM channel accepts. No
frame format, no opcode, no command has been observed, because nothing was
sent. Noise control and EQ remain `UNKNOWN`, not `SUPPORTED` — the presence of
a channel is not evidence of what travels over it.

## Experiment 1 — listen-only RFCOMM, half-closed transmit

**2026-08-08T11:55:48Z. Zero bytes transmitted.**

| | |
| --- | --- |
| Service | `{9B26D8C0-A8ED-440B-95B0-C4714A518BCC}` |
| Connect | **Succeeded** |
| Frames received | **0** |
| Channel lifetime | **16 ms**, closed by the device |

### What this establishes

The vendor RFCOMM service is real, is advertised over SDP, and **accepts
inbound connections**. Windows resolved the channel number itself and the
connect completed. That is now confirmed rather than inferred.

### What it does not establish, and why

The 16 ms close is **confounded by the tool's own behaviour** and must not be
read as device policy.

The first build called `shutdown(SD_SEND)` immediately after connecting, as an
extra OS-level guarantee against transmitting. That call announces to the peer
that this side will never send anything. A device speaking a request/response
protocol has no reason to hold a channel open for a peer that has declared it
will never ask a question — so hanging up is the reasonable thing for it to do.

In other words the experiment measured our own end-of-stream signal, not the
headphones' behaviour toward a silent-but-open peer. 16 ms is far too fast to
be an idle timeout, which supports this reading.

`shutdown(SD_SEND)` is now opt-in (`--half-close`) and off by default. The
no-write guarantee is unaffected: transmitting requires a `send` call, and no
such symbol is imported anywhere in the crate.

### Not yet established

Whether the device volunteers anything to a silent peer that has *not*
announced end-of-stream. Experiment 2 repeats the capture without the
half-close.

Nothing about ANC, Aware, EQ or any other feature. A channel accepting a
connection says nothing about what travels over it.

## Experiment 2 — listen-only RFCOMM, no half-close

**2026-08-08T12:07:41Z. Zero bytes transmitted.**

| | |
| --- | --- |
| Service | `{9B26D8C0-A8ED-440B-95B0-C4714A518BCC}` |
| Connect | Succeeded |
| Channel lifetime | **192 seconds**, closed by the operator |
| Frames received | **0** |
| Actions marked | 16 |

Marked actions, all performed physically on the headphones while connected:
ANC Quiet ×2, ANC Aware ×2, ANC Custom ×2, Volume Up ×2, Volume Down ×4,
Play ×2, Pause ×2.

### Two findings

**1. Experiment 1's 16 ms close was our fault, now confirmed.**
Removing `shutdown(SD_SEND)` changed the channel lifetime from 16 ms to
192 seconds with no other change. The device is content to hold an idle
channel open indefinitely; it was the end-of-stream announcement it objected
to. The Experiment 1 confound is settled.

**2. The channel is strictly request/response.**
Across 192 seconds the device sent **nothing** — not a heartbeat, not a
greeting, and critically not a state notification when the noise-control mode
was physically changed six times. A device that pushed state changes would
have emitted something at 23954 ms, 35692 ms, 51913 ms, 64574 ms, 71041 ms or
77216 ms. Nothing arrived.

### What this means for the project

**Passive observation of this channel is exhausted.** There is no further
information to be gained by listening, because the device does not speak
unless spoken to. This is a genuine negative result, not a failed capture.

Every vendor-protocol capability therefore remains **UNKNOWN**. Specifically,
none of the following changed status as a result of these experiments:
noise control, Aware mode, custom noise control, equalizer, multipoint,
firmware version, auto-off, voice prompts, sidetone, device rename.

Reaching them would require transmitting a frame the device recognises. That
is a write to physical hardware and a separate decision, which has not been
taken. No frame format is known, and none will be guessed — see the stop
condition at the end of this document.

> **Correction, 2026-10-10.** Both listening experiments were pointed at
> `SRfcomm`, which the SDP table puts on RFCOMM channel 25. BMAP is served on a
> different channel. "The channel is strictly request/response" and "passive
> observation is exhausted" are statements about `SRfcomm`; the control
> channel was never listened to. Nothing was lost by that — no frame format
> was known at the time — but the conclusion was about the wrong thing.

## Experiment 3 — Bose Music traffic, recovered from an Android HCI snoop log

**2026-08-12T14:27Z. Offline analysis of a phone-side capture. Nothing was
transmitted by this project.**

The phone's Bluetooth HCI snoop log was captured while Bose Music performed
noise-mode changes, then parsed offline. 19840 HCI records yielded 705 RFCOMM
frames for the headphones, 451 of them carrying payload.

### RFCOMM channel map

| DLCI | Frames | Contents |
| --- | --- | --- |
| 0 | 18 | RFCOMM control (PN/MSC negotiation) |
| 16 | **369** | **Bose vendor protocol** |
| 24 | 56 | Hands-Free Profile — plain AT commands (`AT+BRSF`, `AT+BAC`, `AT+CIND`) |
| 39 | 8 | Unidentified |

DLCI 16 is the channel of interest. It carries a structured binary protocol,
not AT commands.

### Frame structure

Every frame observed on DLCI 16 fits:

```text
byte 0   function group
byte 1   function within the group
byte 2   operator
byte 3   payload length
byte 4+  payload
```

Multiple frames are frequently concatenated into a single RFCOMM payload.

Operators observed, with the roles their usage is consistent with:

| Value | Apparent role |
| --- | --- |
| `0x01` | request / get |
| `0x03` | response / status |
| `0x05` | set |
| `0x06` | notify |
| `0x07` | result or end-of-list |

### Function groups observed

| Group | Contents seen |
| --- | --- |
| `0x00` | Device info — firmware `1.0.6-80+f5f219b`, serial, BD address |
| `0x01` | Device settings — product name "Aurora" |
| `0x02` | Status block, includes `28 FF FF 00` (unidentified) |
| `0x04` | Paired/connected device management — addresses and names |
| `0x05` | Media metadata — track title, album |
| `0x1F` | **Noise control** |
| `0x07` `0x09` `0x12` | Small, unidentified |

### Group `0x1F` — noise control

`1F 06` enumerates the available modes. Each entry is a fixed 0x2F-byte block
whose payload contains the mode name in plain ASCII:

| Index | Name |
| --- | --- |
| `0x00` | `Quiet` |
| `0x01` | `Aware` |
| `0x02` | `Home` |
| `0x03` | *(empty name)* |

`1F 03` carries the current mode. The full exchange for a mode change, seen
identically six times:

```text
PHONE  -> DEVICE   1F 03 05 02 XX      set current mode to XX
DEVICE -> PHONE    1F 03 07 00         result
DEVICE -> PHONE    1F 04 03 01 XX      notify
DEVICE -> PHONE    1F 03 06 01 XX      notify current mode = XX
```

> **Correction, 2026-10-10.** The phone-to-device frames in this section are
> each one byte short. This project's snoop parser subtracted the RFCOMM
> credit octet from the frame length — the length field does not count it —
> and so dropped the final byte of every credit-carrying frame. What Bose
> Music sent was six bytes, `1F 03 05 02 XX 01`: the payload is
> `[mode index, announce]`.
>
> The evidence was on the page. The frame above carries a length byte of `02`
> and one payload byte. That mismatch was noticed at the time and explained
> away instead of chased.
>
> The parser is fixed (`tools/bose-btsnoop-parse/src/stack.rs`, with
> wire-accurate tests), and the original capture re-parses to the six-byte
> form.
>
> The operator roles in the table above were guesses. In BMAP they are `0x00`
> SET, `0x01` GET, `0x02` SETGET, `0x03` STATUS, `0x04` ERROR, `0x05` START,
> `0x06` RESULT, `0x07` PROCESSING. And index `0x02` ("Home") is a
> user-configurable slot, not a third preset.

### Correlation

Six mode changes were captured, and in every case the mode-definition block
the device subsequently returned matched the value byte:

| Time | Frame | Value | Block returned |
| --- | --- | --- | --- |
| 14:28:09.551 | `1F 03 05 02 00` | `0x00` | `Quiet` |
| 14:28:14.630 | `1F 03 05 02 01` | `0x01` | `Aware` |
| 14:28:19.504 | `1F 03 05 02 00` | `0x00` | `Quiet` |
| 14:28:24.152 | `1F 03 05 02 01` | `0x01` | `Aware` |
| 14:29:13.708 | `1F 03 05 02 02` | `0x02` | `Home` |
| 14:29:15.808 | `1F 03 05 02 01` | `0x01` | `Aware` |

Six repetitions, one varying byte, and the device naming the resulting mode in
ASCII. The alternating Quiet/Aware pattern at roughly five-second intervals
matches the sequence the operator reported performing.

### What this justifies, and what it does not

**Justifies** moving noise control, Aware mode and firmware version from
`UNKNOWN` to `SUPPORTED`: a specific interface has been identified, with the
frame layout, the opcode and the parameter values all observed rather than
guessed.

**Does not justify `VERIFIED`.** Verification in this project means *our* code
caused an observed state change on the physical device. This project has still
transmitted exactly zero bytes. Everything here was performed by Bose Music
and watched afterwards from a log.

**No evidence for EQ.** No equalizer traffic appears in this capture — the
slider was evidently not moved. Equalizer stays `UNKNOWN`. Group `0x04`
(paired device management) is suggestive for multipoint but has not been
decoded, so multipoint also stays `UNKNOWN`.

### Privacy note

A snoop log is highly personal. This one contained the headphones' serial
number, Bluetooth addresses, the names of paired devices, and **the title of
the media playing at the time**. The parser filters to a single device, but
the resulting reports still contain all of the above for that device.

`.gitignore` excludes `btsnoop-rfcomm.*`, `btsnoop_hci*` and `*.cfa`. Capture
files and parser output must never be committed.

## Experiment 4 — transmitting the observed frames

**2026-08-12. First transmitting experiment, authorised by the user.**

Replayed Bose Music's exact opening frames, then its exact mode-change frame,
against the same vendor RFCOMM service.

| Step | Frame | Result |
| --- | --- | --- |
| Protocol version | `00 01 01` | 3/3 bytes accepted, **no reply** |
| Enumerate noise group | `1F 01 05` | 3/3 bytes accepted, **no reply** |
| Read current mode | `1F 03 01 00` | 4/4 bytes accepted, **no reply** |
| Set mode to Quiet | `1F 03 05 02 00` | 5/5 bytes accepted, **no reply** |

**The headphones did not change mode.** Confirmed by the user physically
observing the device, which is the only signal that mattered here.

### Variations tried, all identical

| Variation | Result |
| --- | --- |
| Channel resolved from the vendor UUID via SDP | Connects, silent |
| Direct RFCOMM server channel 8 (DLCI 16 >> 1) | Connects, silent |
| `SO_BTH_AUTHENTICATE` + `SO_BTH_ENCRYPT` | Connects, silent |
| Listen window extended to ~6s | Silent |

### What is ruled out

* **Wrong endpoint.** The capture shows the phone sending SABM on DLCI 16 and
  the headphones answering UA, so the headphones are the RFCOMM server on
  channel 8 — which is what we connect to, both by UUID and by number.
* **Dropped writes.** Winsock accepted every byte of every frame.
* **Too short a wait.** Six seconds of silence.
* **Channel dropping.** It stays open throughout; no close is reported.
* **Malformed frames.** They are byte-for-byte replays of frames the device
  demonstrably acted on when Bose Music sent them.

### What this establishes

A connection to the vendor RFCOMM service can be *established* from Windows,
but the device does not process application data sent over it — it neither
replies nor acts. Something about the session differs from Bose Music's in a
way that is not visible at the RFCOMM layer.

Remaining untested hypotheses, in order of plausibility:

1. **The device serves one vendor session at a time and the phone holds it.**
   The capture showed the headphones connected to both the phone and this PC.
   Not yet tested with the phone fully disconnected.
2. **RFCOMM credit-based flow control.** The capture's PN negotiation on
   DLCI 0 used the credit-based convergence layer. If the device is granted
   zero send credits it cannot reply, which would look exactly like this and
   is invisible above the RFCOMM layer.
3. **Application-level authorisation** performed by Bose Music before the
   frames we replayed.

The decisive next step is a snoop capture of Bose Music connecting **from
cold** — Bluetooth off, logging on, Bluetooth on, app opened. The existing
capture starts mid-session and therefore does not contain the DLCI 0
parameter negotiation for DLCI 16.

### Capability status

Unchanged. Noise control and Aware mode remain **SUPPORTED**, not `VERIFIED`
— the interface is documented and real, but this project still cannot make
the device do anything.

They are deliberately not marked `UNSUPPORTED` either. That status is terminal
and means the feature cannot be reached through available interfaces, and the
cold-connect capture has not been tried. Absence of a working path today is
not proof that none exists.

> **Correction, 2026-10-10 — why this failed.** Not for any of the three
> hypotheses above. No session lock, flow-control problem or authorisation
> step is involved; none was needed to make it work.
>
> * **The frames were malformed.** "Byte-for-byte replays" was false: three of
>   the four frames were one byte short (see the correction to Experiment 3).
>   BMAP is length-prefixed over a byte stream, so a short frame does not
>   simply fail on its own — the device reads the start of the next frame as
>   the missing remainder, and everything after it is out of step. That would
>   also account for the one well-formed frame, `1F 03 01 00`, going
>   unanswered. This last step is inference. It has not been tested and will
>   not be, because testing it means sending malformed frames on purpose.
> * **One of the two endpoints was wrong.** "Resolved from the vendor UUID"
>   meant `SRfcomm`, which is channel 25 and does not speak BMAP. Channel 8 was
>   right.
>
> What is established is the other half: the same headphones, on channel 8,
> answer well-formed frames at once (Experiment 5). The tool used here,
> `tools/bose-anc-probe`, has been removed. It could only send the short
> frames.

## Experiment 5 — Quiet/Aware control over BMAP

**2026-10-10. Read-only checks first; then mode changes, each confirmed by
reading the mode back from the headphones.**

### Sources

The protocol was not reverse-engineered here. It is documented, with a working
implementation, in [aaronsb/bosectl](https://github.com/aaronsb/bosectl) (MIT),
whose notes cover this exact model and firmware
(`docs/quietcomfort-headphones-prince.md`). Its Rust crate is vendored at
commit `70570b29` and its packet codec, model configuration and reply
validation are used unmodified; see `vendor/bose-bmap/VENDORED.md`. Only the
transport is new, because upstream's is Linux-only.

[MadEyez/quietrebellion](https://github.com/MadEyez/quietrebellion), a Windows
port tested on a different model, was read for how it uses Winsock. No code
was taken from it, and nothing about it was assumed to hold for this model.

### What the device advertises

SDP records as Windows reads them (`hw_diag_sdp_records`):

| Service | Class UUID | RFCOMM channel |
| --- | --- | --- |
| Headset | `0x1108`, `0x1131` | 11 |
| Hands-Free | `0x111E` | 12 |
| BMAP | `00000000-deca-fade-deca-deafdecacaff` | 14 |
| `SRfcomm` | `9b26d8c0-a8ed-440b-95b0-c4714a518bcc` | 25 |
| A2DP, AVRCP | `0x110B`, `0x110C`, `0x110E`/`0x110F` | — (L2CAP) |
| Serial Port | `0x1101` | *no record returned* |

Three things follow. The second is the surprising one.

* The BMAP UUID **identifies** the device as BMAP-capable. The application
  requires Windows to list it before it opens anything.
* The channel that record names **does not speak the documented protocol.**
  A connection made through the BMAP UUID was accepted once and answered a
  firmware request with 14 bytes that do not frame as BMAP (not captured —
  failed attempts keep their wire log now). The next attempt hung for the full
  60 seconds of a blocking Winsock connect; the one after timed out at the
  8-second limit added because of it. Upstream lists 14 among the channels
  that "accept the socket without speaking BMAP". It is not used as a route.
* BMAP is served on **channel 8**, which no SDP record Windows lists points
  at. Windows nevertheless resolves the Serial Port UUID to it: a connection
  made by that UUID and a connection to channel 8 by number exclude each
  other (`WSAEADDRINUSE` in both orders, `hw_diag_resolved_channel`), which is
  what one channel looks like. Upstream reports the same resolution on Linux,
  and a second unit answering on 9 — so the number is not assumed.

### Endpoint selection

In order, each validated before use:

1. the channel Windows resolves for the Serial Port service;
2. upstream's configured channel for the model (8);
3. upstream's documented fallbacks (2, 9).

Nothing outside that list is tried. A route counts only if the device answers
a firmware GET (`00 05 01 00`) with a STATUS frame from the same address
carrying a printable string; an open socket proves nothing. And before any
write is allowed, the mode read must come back in exactly the documented
shape — STATUS from `[31.3]` with one payload byte. A reply in any other
shape leaves the session read-only.

### Results on the physical device

Firmware `1.0.6-80+f5f219b` — the firmware upstream's notes were written
against.

| Check | Test | Result |
| --- | --- | --- |
| Endpoint validated, mode read | `hw_a1`, `hw_b0` | Pass |
| Reconnect after close | `hw_a2` | Pass |
| Five reads on one connection | `hw_a3` | Pass, identical |
| **Switch to Quiet, confirmed by read-back** | `hw_b1` | **Pass** |
| **Switch to Aware, confirmed by read-back** | `hw_b1` | **Pass** |
| Requesting the current mode sends no START | `hw_b2` | Pass |
| Connection dropped; next refresh reconnects (×3) | `hw_b3` | Pass, ~1.2 s each |
| Headphones paired but off: reported disconnected, no mode shown, change refused, nothing sent | `hw_c1` | Pass |

The two transitions, as transmitted and received:

```text
TX  1F 03 01 00          GET current mode
RX  1F 03 03 01 01       STATUS  index 1 (Aware)
TX  1F 03 05 02 00 00    START   mode 0, no voice prompt
RX  1F 03 07 00          PROCESSING
TX  1F 03 01 00          GET current mode
RX  1F 03 03 01 00       STATUS  index 0 (Quiet)     <- confirmed

TX  1F 03 05 02 01 00    START   mode 1, no voice prompt
RX  1F 03 07 00          PROCESSING
TX  1F 03 01 00          GET current mode
RX  1F 03 03 01 01       STATUS  index 1 (Aware)     <- confirmed
```

The device acknowledges with PROCESSING and applies the change afterwards, so
the acknowledgement is never treated as the result. The application polls the
mode — at most five times, 250 ms apart — and reports success only when the
device reports the mode that was asked for. Sent, acknowledged and confirmed
are three separate things in the code and in what the user is told.

One unprompted observation. Earlier in the same session the headphones
reported Quiet; a later read-only run found them reporting Aware. This project
had sent nothing but GETs in between, so the change came from outside it —
the headphones' button, or another connected device — and the next read
reflected it.

### What this does and does not establish

**Established:** this application's own requests change the mode the
headphones report, in both directions, and the report is read back from the
device each time. Noise control and Aware mode are `VERIFIED` on this unit.

**Not established:**

* "Verified" means the device's own report. Whether the change is audible was
  not measured by anything in this project.
* Timeouts, ERROR replies, partial and concatenated frames, and a channel
  dying mid-request are covered by unit tests against scripted streams, not on
  hardware — there is no safe way to make real headphones misbehave on cue.
  The headphones being off *before* a session is covered (`hw_c1`); switching
  them off in the middle of one, with a connection open, has not been tried.
* One unit, one firmware. The other models upstream supports are refused by
  this application until each has been run against hardware.
* The BMAP-UUID service stopped accepting connections from this PC after it
  was first probed. It is unrelated to the working channel and was left alone
  afterwards. Whether it recovers when the headphones reconnect is not known.
* Nothing else in the protocol is driven. EQ, multipoint and the rest are
  untouched and remain `UNKNOWN`. (Custom modes followed later the same day:
  Experiment 6.)

### In the application

`VERIFIED` above is the project's record. Inside the running application the
status is rebuilt from what that session has itself observed: noise control is
`EXPERIMENTAL` once the endpoint is validated and the mode has been read, and
becomes `VERIFIED` when the headphones have confirmed a switch in each
direction during that session.

## Experiment 6 — the custom mode and its level

**2026-10-10. A read-only listing first; then a switch into the custom mode
and a change to its level, each confirmed by read-back, and both put back.**

### What the headphones hold

Besides the two presets there are two user-configurable slots. Listing them is
a read (`START` on the mode-list function, `1F 01 05 00`), answered with one
47-byte configuration per slot — the length and layout upstream documents for
this model. On the test unit (`hw_diag_mode_slots`):

| Slot | Name | Raw level | Editable | Set up |
| --- | --- | --- | --- | --- |
| 0 | Quiet | 0 | no | — |
| 1 | Aware | 10 | no | — |
| 2 | Focus | 6 | yes | yes |
| 3 | *(empty)* | 0 | yes | no |

The device's level runs backwards from how a person would label it: raw 0 is
the most cancellation and raw 10 lets the most through. The application shows
strength = 10 − raw, so "Focus" at raw 6 is shown as level 4.

### What "Custom" means in the application

The headphones' own custom mode — whichever editable slot is set up. If they
are already in one, that one; otherwise the one last seen this session;
otherwise the first. The application does not create, rename or delete modes,
and headphones with no custom mode get no Custom control, with the reason
shown.

The slider edits the level **of the mode the headphones are in**, and the
change is saved in that mode on the device. That is how the headphones work:
there is no separate live level on this firmware (upstream found the direct
register, `[31.10]`, unsupported).

### The write, and the guard on it

A mode configuration carries a name, a voice-prompt id and other settings in
the same frame as the level. So changing the level means writing all of it
back, and the risk is changing something else along the way. Before a write
goes out, the frame — built by upstream's builder from what the device just
reported — is compared field by field against the device's own bytes. It must
differ in the level byte and nowhere else. If the name would not survive
re-encoding, or a setting this build does not model (automatic noise
cancellation) would be reset, nothing is sent.

Switching into a slot is the same `START [31.3]` frame as for a preset,
carrying the slot index. Only the model's editable slots can ever be named.

### Results on the physical device

`hw_b5`, in one run. Levels are the application's (10 = most cancellation).

| Step | Sent | Device then reported |
| --- | --- | --- |
| Aware → custom | `1F 03 05 02 02 00` | mode index 2 ("Focus") |
| Level 4 → 6 | `1F 06 02 27 02 00 0D "Focus"… 04 00 00 00` | raw 4 |
| Level 6 → 4 | `1F 06 02 27 02 00 0D "Focus"… 06 00 00 00` | raw 6 |
| Custom → Aware | `1F 03 05 02 01 00` | mode index 1 |

After the last step the configuration of "Focus" was byte-for-byte what it had
been at the start, and the headphones were back in the mode they began in.
Each level write was one frame; the device answered each with the slot's new
configuration, and a separate listing afterwards agreed.

### What this does and does not establish

**Established:** the application can put these headphones into their custom
mode and change that mode's level, and both are read back from the device.
Custom noise control is `VERIFIED` on this unit.

**Not established:**

* As before, "verified" is the device's report, not a measurement of sound.
* Only the level is written. Wind block, which lives in the same
  configuration, is carried through unchanged and is not adjustable here.
* A unit with *two* custom modes has not been tested; the choice between them
  is covered by unit tests only.
* Automatic noise cancellation was off on the test unit's mode. A mode with it
  on is refused rather than overwritten, and that refusal is exercised only in
  unit tests.
* A level change takes about three seconds end to end, because every step is
  read back and each listing waits for the device to finish talking. It has
  not been tuned.

## Experiment 7 — the hardware equalizer

**2026-10-10. Read first, then one band changed by one step and put back.**

The equalizer is three bands — bass, mid, treble — each from -10 to +10, at
`[1.7]`. Upstream documents it for this model and firmware, and its `eq()` and
`set_eq()` are used as they are. A read returns one `[min, max, current, band]`
group per band; a write is one SETGET per band, `[value, band]`.

A reply is accepted only if it is exactly three bands, each once, each with
the documented range and a value inside it. Anything else leaves the
equalizer unread and unwritten.

On the test unit (`hw_b6`, `hw_b7`):

```text
TX  01 07 01 00                                        GET
RX  01 07 03 0C F6 0A 04 00 F6 0A 01 01 F6 0A 04 02    bass +4, mid +1, treble +4

TX  01 07 02 02 05 00                                  bass = +5
TX  01 07 02 02 01 01                                  mid  = +1
TX  01 07 02 02 04 02                                  treble = +4
TX  01 07 01 00                                        GET
RX  01 07 03 0C F6 0A 05 00 F6 0A 01 01 F6 0A 04 02    bass +5 — confirmed
```

It was then set back to bass +4 and that was confirmed the same way.

All three bands are written on every change, including the ones that are not
changing, because that is what upstream's `set_eq` does. The device answers
each write with the whole equalizer.

**Established:** the application reads the equalizer these headphones hold
and can change it, confirmed by read-back. **Not established:** that the
change is audible (nothing here measures sound); negative gains on hardware
(covered by unit tests only — the hardware test moved one band between +4 and
+5); behaviour on any other model.

## Where the protocol knowledge lives

This document records what was measured on one pair of headphones. It is not a
protocol reference, and the codebase does not contain one of its own: frame
encoding, operators, error codes and per-model addresses come from the
vendored `bose-bmap` crate (`vendor/bose-bmap`, MIT, Copyright (c) 2026 Aaron
Bockelie).

## Nothing below here is a finding

The remaining sections were written before any hardware was available. They
are kept as the reasoning that led here, not as results.

## API-first: what was ruled out before considering a vendor protocol

Per the API-first rule, standard interfaces were assessed first.

| Interface | Exposes | Covers ANC/EQ? |
| --- | --- | --- |
| A2DP | Audio streaming | No |
| AVRCP | Play/pause/next/previous, metadata | No |
| HFP / HSP | Call audio, battery indication | No |
| BLE Battery Service (0x180F) | Battery level | No |
| BLE Device Information (0x180A) | Manufacturer, model, firmware strings | No |
| BLE Generic Access (0x1800) | Device name, appearance | No |
| HID | Key/consumer controls | No |

Conclusion: **no standard Bluetooth profile exposes noise control or EQ.** Those
features are reachable only through a vendor-specific interface, which cannot be
investigated without the hardware.

Two features *are* plausibly reachable through standard interfaces, and are the
right things to implement first:

* **Battery** — via the Windows PnP property, which Windows populates from HFP
  or the BLE Battery Service. Mechanism verified on this machine against a
  different device; unverified for Bose.
* **Transport controls** — via the Windows global media session, which works
  for any device Windows routes audio to, without touching Bluetooth directly.

## Planned investigation, when hardware is present

Read-only and passive, per the agreed posture. The tool sends nothing to the
device beyond standard protocol reads.

1. Pair the QC and confirm it appears under `BTHENUM` / `BTHLE`.
2. Enumerate SDP service records — which RFCOMM channels are advertised.
3. Enumerate GATT services and characteristics, and their properties.
4. Read every readable characteristic once.
5. Subscribe to notifications and idle.
6. With capture running, physically operate the headphones — change ANC mode,
   press volume, power cycle — and record which handles emit data and when.
7. Correlate emissions against timestamped manual markers.

Step 6 is the valuable one. Passive observation of the device reporting its own
state changes is the strongest evidence available without writing anything, and
it is what turns `unknown` into a real finding.

## Explicit non-goals

The following are out of scope permanently, not merely deferred:

- Firmware modification, extraction or flashing
- Bypassing or defeating authentication, pairing or encryption
- Brute-forcing anything
- Destructive or uncontrolled fuzzing
- Sending commands whose effect is not understood

The goal is interoperability with hardware the user owns, not compromise of it.

## Stop condition

If, after the investigation above, a feature cannot be reached through a
standard Windows API, a standard Bluetooth profile, a publicly documented
interface, or safely verified observation of the user's own device, it will be
marked `UNSUPPORTED` with the reasoning recorded — and the work will stop there
rather than escalating to riskier techniques.

## Experiment 8 — the 200 ms settle pause is not needed (2026-10-11)

Upstream's transports sleep 200 ms after every send before reading. This
transport waits for the reply against a deadline instead of reading once, so
the pause was removed (`Timing::standard`, `settle: 0`) and everything re-run
on the headphones (firmware 1.0.6-80+f5f219b):

- 40 `GET [31.3]` reads, 150 ms apart: 10–50 ms each, median 22 ms (200–202 ms
  with the pause).
- `hw_a1`–`hw_a3`, `hw_b0`–`hw_b7`: all pass. Quiet/Aware in both directions,
  the custom mode and its level, and the equalizer each still confirm by
  read-back.
- A mode change is confirmed about 0.19 s after the request (1.2 s before).

This is what makes it reasonable to ask for the mode every 150 ms while spoken
mode announcements are on, which is how a press of the button on the
headphones is noticed: the channel pushes nothing (Experiment 2).

## Experiment 9 — a mode list ends when the headphones say so (2026-10-12)

**No new traffic was sent for this.** It is a change to how an existing,
read-only reply is read, made from the capture already taken in Experiment 6,
and it has not yet been timed on the headphones.

`START [31.1]` — "list the modes" — is the one request here that is answered
with several frames, most of them from another address. As captured from the
test unit:

```
1F 01 07 00            [31.1] PROCESSING
1F 06 07 00            [31.6] PROCESSING
1F 06 03 2F …  × 4     [31.6] STATUS, one 47-byte configuration per slot
1F 06 06 00            [31.6] RESULT
1F 01 06 00            [31.1] RESULT
```

Upstream's transport, and this one until now, read such a reply by waiting
for the device to fall silent: 500 ms with nothing arriving meant it was
over. So every listing took the reply plus half a second, with the control
link held throughout — and nothing else can be asked while it is held, not
even which mode the headphones are in.

That mattered more than it looks:

- The list was read on every refresh spent in a custom mode (every 4–5 s),
  and again before every switch into one.
- The 150 ms mode checks that catch a press of the headphones' button queue
  behind it.

So a switch into "Focus" from the application waited about half a second
before anything was sent and about half a second more before the screen and
the announcement caught up, and a button press while in Focus could be late
by up to the same amount. This is what was reported from a phone as "the
voice prompt lags on Focus, and sometimes on the mode after it".

The reply carries its own end. The address that was asked answers `RESULT`
last (or `ERROR`, if it will not answer at all), and nothing follows. The
transport now treats that frame as the end of a drained reply
(`ends_drained_reply` in `bose/bmap/stream.rs`):

- only a frame from the address the request was sent to counts — `[31.6]
  RESULT` arrives one frame earlier and is not the end;
- `PROCESSING` from that address is not the end either;
- a device that never sends the frame is handled as before, by silence.

Alongside it, the list is asked for less: once when the headphones arrive,
then every ten seconds while they are in a custom mode (its level is on
screen then and can be changed from a phone), and before a switch into a
custom mode only if it was not read within the last ten seconds.

**Established:** the frame order above, by capture (Experiment 6). That the
transport returns at the last frame without a further read, by unit test
against that capture, including the reply arriving split across reads.

**Not established:** the time a listing now takes on the headphones. The
expectation is tens of milliseconds, like any other request; it has not been
measured, because the desktop application was running and holds the one
control connection. `hw_diag_mode_slots` prints the wire log and will show
it.
