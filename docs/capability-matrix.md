# Capability Matrix

The authoritative record of what this application can actually do.

**Last updated:** 2026-10-10
**Hardware status:** A real Bose QuietComfort Headphones (renamed "Aurora", SIG
vendor `0x009E`, product `0x4075`, firmware `1.0.6-80+f5f219b`) is connected
over Bluetooth Classic. Battery, device identity, Windows volume, firmware
version and noise control — Quiet, Aware, and the custom mode with its level —
are verified against it. Each was changed by this application and confirmed by
the headphones' own report — see Experiments 5 and 6 in
[protocol-notes.md](protocol-notes.md).

## Status definitions

| Status | Meaning |
| --- | --- |
| `UNKNOWN` | We have not established whether the feature is accessible. |
| `SUPPORTED` | A technically valid interface appears to expose it, but it has **not** been confirmed on the physical device. |
| `VERIFIED` | The actual physical device was tested and the feature was confirmed to work. |
| `EXPERIMENTAL` | Evidence suggests it may work, but verification is incomplete. |
| `UNSUPPORTED` | Cannot currently be accessed safely through available interfaces. |

`SUPPORTED` is never treated as working. The UI renders it with an explicit
caveat, and `CapabilityStatus::is_actionable()` returns `false` for it.

## Matrix

| Feature | Mechanism | Status | Hardware verified |
| --- | --- | --- | --- |
| Device detection | Windows PnP, SIG vendor id `0x009E` | **VERIFIED** | **Yes** |
| Connection state | Windows PnP property | **VERIFIED** | **Yes** |
| Battery | Windows PnP battery property (HFP child node) | **VERIFIED** | **Yes** — read 30% |
| Device identity | Windows PnP | **VERIFIED** | **Yes** |
| Windows volume | Core Audio | **VERIFIED** | **Yes** (see below) |
| Playback / transport | Windows media session (not yet wired) | UNKNOWN | No |
| Noise control (Quiet) | BMAP over RFCOMM, `[31.3]` | **VERIFIED** | **Yes** — Aware → Quiet confirmed by read-back |
| Aware mode | BMAP over RFCOMM, `[31.3]` | **VERIFIED** | **Yes** — Quiet → Aware confirmed by read-back |
| Custom noise control | BMAP over RFCOMM, `[31.3]` and `[31.6]` | **VERIFIED** | **Yes** — switch into the custom mode and a level change, both confirmed by read-back |
| Equalizer | BMAP over RFCOMM, `[1.7]` | **VERIFIED** | **Yes** — read (bass +4, mid +1, treble +4), changed and restored, confirmed by read-back |
| Multipoint | BMAP (not driven) | UNKNOWN | No |
| Firmware version | BMAP over RFCOMM, `[0.5]` | **VERIFIED** | **Yes** — read `1.0.6-80+f5f219b` |
| Auto-off | BMAP (not driven) | UNKNOWN | No |
| Voice prompts | BMAP (not driven) | UNKNOWN | No |
| Sidetone | BMAP (not driven) | UNKNOWN | No |
| Device rename | BMAP (not driven) | UNKNOWN | No |

This table is the project's record of what has been demonstrated on the test
unit. **The running application does not start from it.** Each session rebuilds
its statuses from what it has itself observed, so noise control shows
`EXPERIMENTAL` when the app opens and becomes `VERIFIED` once the headphones
have confirmed a switch in each direction in that session — and the custom
mode once they have confirmed a switch into it and a change to its level.
Nothing is carried over on trust.

"Not driven" means upstream documents the feature for this model and this
application sends nothing for it. Documented is not demonstrated, so those rows
stay `UNKNOWN`.

## Mechanism notes

### Windows volume — what "VERIFIED" means here, precisely

Core Audio enumeration was run against this machine's real hardware. It
returned `Speakers (Realtek(R) Audio)` at 44%, muted, default render endpoint,
48000 Hz — and a volume round-trip confirmed the endpoint reports back the
value that was set.

That verifies the **Windows audio path**, on this machine, for this endpoint.
It does **not** verify anything about a Bose endpoint, which will not exist
until the headphones are paired and connected. When they are, the same code
will read whatever endpoint Windows creates for them, and the capability will
re-verify against that.

This distinction matters and the code preserves it: `attach_windows_audio()`
builds a `HardwareProof` from the endpoint it actually read, naming that
endpoint in the evidence string.

Note also that Windows system volume for an endpoint is a different mechanism
from any volume the headphones keep internally. The UI labels it as such and
never presents the two as one control.

### Battery — now verified, and the bug that hid it

Confirmed on the physical device: **30%**, read from the Windows PnP battery
property while the headphones were connected.

The value lives on the **HFP child node** (`Aurora Hands-Free AG`,
service `0000111E`), not on the top-level `BTHENUM\DEV_...` node, which reports
nothing. The original implementation read only top-level nodes and therefore
reported "no battery reported" for a device that was in fact reporting it.
`enumerate_bluetooth_devices()` now folds properties across every node sharing
a device address.

### Device detection — why the name hint was not enough

The test unit is renamed **"Aurora"** and matches none of the Bose name hints,
so name-based detection found nothing. Every profile child node carries
`VID&0001009E` — SIG company `0x009E`, Bose Corporation — which identifies it
unambiguously. `is_bose_device()` prefers the vendor id and falls back to the
name only when no vendor id is exposed.

### Noise control — what `VERIFIED` means here, precisely

On 2026-10-10 the application's own backend switched the test headphones from
Aware to Quiet and back to Aware. For each switch it read the mode first, sent
the request, received the device's acknowledgement, and then read the mode
again — and the device reported the mode that had been asked for. The
before and after values both came from the headphones, never from local state.
That is the definition of `VERIFIED` this project has used from the start, met
in both directions.

What it does not cover:

* **The device's report, not the sound.** Nothing in this project measures
  whether the change is audible.
* **This unit and this firmware.** Other models are refused by the application
  until each has been run against hardware.
* **Failure handling on hardware.** Timeouts, error replies, partial frames and
  a channel dying mid-request are covered by unit tests against scripted
  streams. Real headphones cannot safely be made to misbehave on cue.

An outcome short of a confirming read-back is never reported as success. A
request that was sent but not confirmed is "sent, state could not be
verified", and the UI keeps showing the last mode the headphones confirmed.

### Custom noise control — what is and is not covered

"Custom" is the headphones' own custom mode: one of two user-configurable
slots, if one is set up. On 2026-10-10 the application switched the test
headphones into theirs ("Focus"), changed its noise-cancellation level, changed
it back, and returned to the starting mode — every step read back from the
device, and the mode's configuration byte-for-byte unchanged at the end.

`VERIFIED` here needs two things, because they are two different operations: a
confirmed switch *into* the custom mode, and a confirmed change to its level.

Covered: selecting the custom mode; reading its name and level; setting its
level from 0 to 10. A level change is saved in that mode on the headphones —
that is the only place this firmware keeps it.

Not covered, and not attempted: creating, renaming or deleting a mode; wind
block; anything on headphones that have no custom mode set up, where the
control is withheld and says why.

### Equalizer — what `VERIFIED` covers

Three bands, -10 to +10, held on the headphones. The application shows the
values the headphones report and, on 2026-10-10, changed one band and restored
it with each step read back. In the running application the equalizer is
`EXPERIMENTAL` once it has been read and `VERIFIED` once a change has been
confirmed in that session.

This is the hardware equalizer only. The "Software EQ" panel in the UI is not
implemented and says so.

### Multipoint and the rest — still `UNKNOWN`

Upstream documents some of these for this model. This application has not sent
a single frame for any of them, so there is no evidence of its own to stand on.

### Vendor protocol features — the original reasoning

Bose headphones expose device control through a vendor-specific protocol rather
than a standard Bluetooth profile. Standard interfaces were considered first,
per the API-first rule:

| Standard interface | Applicable? |
| --- | --- |
| A2DP | Audio streaming only. No device control. |
| AVRCP | Transport controls (play/pause/next). No ANC or EQ. |
| HFP/HSP | Call audio; carries a battery indication. No ANC or EQ. |
| BLE Battery Service (0x180F) | Battery only. |
| BLE Device Information (0x180A) | Static strings only. |
| HID | Not applicable to these controls. |

No standard interface exposes noise control or EQ. That leaves a vendor
protocol, which cannot be investigated without the physical device.

That reasoning still holds for everything except noise control and the
equalizer. The build returns `Unsupported` with a reason from
`set_noise_control` for "off", rather than sending unverified bytes at the
hardware. See [protocol-notes.md](protocol-notes.md).

## What it takes to change a row

1. Find the feature in upstream's notes for the exact model and firmware.
2. Read it from the device first, and check the reply against the documented
   shape before anything is written.
3. Change it, and read it back. Only a read-back that shows the requested
   value counts.
4. Do it in both directions, on the physical device, through the application's
   own backend.
5. Update this matrix from what was observed, not from what was expected.
