# Bluetooth Discovery

**Status: the standalone tool in `tools/bose-discovery/` is not yet
implemented.** What exists today is read-only enumeration inside the
application (`bluetooth/pnp.rs`, `bluetooth/radio.rs`), surfaced on the
Diagnostics page.

## Posture: read-only and passive

Agreed constraint for this project. The discovery process:

- enumerates devices and services
- reads readable characteristics
- subscribes to notifications and listens
- **sends nothing else to the device**

No writes, no probe frames, no speculative commands. Opening a vendor RFCOMM
channel is also excluded for now, because doing so can prevent Bose Music from
connecting simultaneously.

That is the posture of *discovery*. The application itself has, since
2026-10-10, opened the BMAP control channel to supported headphones in order
to read and switch the noise-control mode — see Experiment 5 in
[protocol-notes.md](protocol-notes.md) and the hardware-safety section of
[security.md](security.md).

## What works today

`bluetooth/pnp.rs` reads, via CfgMgr32:

| Property | Key |
| --- | --- |
| Friendly name | `{a45c254e-df1c-4efd-8020-67d146a850e0}, 14` |
| Device description | `{a45c254e-df1c-4efd-8020-67d146a850e0}, 2` |
| Battery level | `{104EA319-6EE2-4701-BD47-8DDBF425BBE5}, 2` |
| Connection state | `{83DA6326-97A6-4088-9453-A1923F573B29}, 15` |

It enumerates the `BTHENUM` (Classic) and `BTHLE` (LE) enumerators, and filters
to top-level device nodes.

### Why filtering matters

Windows creates a child PnP node per Bluetooth profile. A single pair of
headphones produces entries for AVRCP transport, handsfree, A2DP and more, each
carrying the same friendly name. Without filtering, one device appears as six.

`PnpDevice::is_top_level()` keeps only `BTHENUM\DEV_*` and `BTHLE\DEV_*` nodes
without a service GUID in the instance id. Covered by
`per_profile_child_nodes_are_excluded`, using real instance ids captured from
the development machine.

## Planned tool

`tools/bose-discovery/` will produce `device-report.json` and
`device-report.txt` containing:

- device name and salted id (never the raw address)
- transport and connection state
- advertised SDP services / RFCOMM channels
- GATT services, characteristics and their properties
- values of readable characteristics
- a timestamped event log from the passive capture

### Manual event markers

During capture the operator marks physical actions so emissions can be
correlated with them:

```
VOLUME_UP   VOLUME_DOWN   PLAY      PAUSE
ANC_CHANGE  AWARE_CHANGE  POWER
CONNECT     DISCONNECT    OTHER
```

The workflow for each: record baseline → perform the action physically →
capture what the device emits → compare.

This is what can promote a capability from `unknown` to a real finding without
writing a single byte to the hardware.

## Report privacy

Reports contain salted hashes, not Bluetooth addresses. `.gitignore` excludes
`device-report.*`, `captures/` and `*.btcapture` so captures cannot be
committed by accident.
