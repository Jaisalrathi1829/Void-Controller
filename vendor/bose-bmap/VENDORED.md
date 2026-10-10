# Vendored: bose-bmap

This directory is a vendored copy of the Rust library from
[aaronsb/bosectl](https://github.com/aaronsb/bosectl).

| | |
| --- | --- |
| Upstream | https://github.com/aaronsb/bosectl (`rust/`) |
| Commit | `70570b29c1d3c9ba775b08a29eaea72385de27b5` ("Bump version to 0.5.0", 2026-09-28) |
| Crate | `bose-bmap` 0.5.0, library name `bmap` |
| Licence | MIT, Copyright (c) 2026 Aaron Bockelie — see [LICENSE](LICENSE) |
| Vendored | 2026-10-10 |

The MIT licence requires that the copyright notice and permission notice be
included in all copies or substantial portions of the software. `LICENSE` in
this directory is the upstream file, unmodified.

## Why it is vendored rather than a dependency

Upstream targets Linux. `src/transport.rs` uses `std::os::fd` and raw
`AF_BLUETOOTH` sockets through `libc`, and `src/lib.rs` matches on `libc`
errno values, so the published crate does not compile for Windows. Everything
else — packet codec, device configs, parsers, the `BmapConnection` logic — is
platform-neutral, and that is the part this project needs.

## What is identical to upstream

Verified byte-for-byte against the upstream git blobs at the commit above:

- `LICENSE`
- `README.md`
- `src/protocol.rs` — BMAP packet build/parse, operators, error names
- `src/error.rs`
- `src/device.rs` — `DeviceConfig`, parsers, builders
- `src/devices.rs` — per-model configs, including `qc_prince` (`0x4075`)
- `src/catalog.rs` — product-id catalog, `BMAP_UUID`
- `src/connection.rs` — `BmapConnection<T: Transport>`
- `src/transport.rs` — Linux transport (compiled only on unix)
- `src/discovery.rs` — `bluetoothctl` discovery (unused on Windows)

## What was changed

No logic was changed. The patch is build configuration only.

**`Cargo.toml`**
- `libc` moved from `[dependencies]` to `[target.'cfg(unix)'.dependencies]`.
- The `[[bin]] bmapctl` target was removed.
- `doctest = false` on the library: the crate-level doc example calls
  `connect()`, which is unix-only, so it cannot compile as a doctest on
  Windows.

**`src/lib.rs`** — attributes only:
- `pub mod transport;` is `#[cfg(unix)]`; on other targets
  `transport_portable.rs` is mounted at the same path.
- `#[cfg(unix)]` added to `connect`, `RETRY_DELAYS`, `BUSY_MESSAGE`,
  `is_retryable`, `open_transport`, `connect_with_retry`, `probe_channels`,
  `send_init`, `speaks_bmap`, and the two imports they need.
- The two test modules that exercise that code are `#[cfg(all(test, unix))]`.

**`src/transport_portable.rs`** — new file, not from upstream. Contains only
the `Transport` trait, copied verbatim from upstream `transport.rs`.

## Fixtures

Upstream's unit tests load one shared fixture by a path relative to the source
file (`../../fixtures/...`). It is vendored at the matching relative location,
`vendor/fixtures/packets/qc-ultra2-earbuds/battery-status.hex`, together with
`vendor/fixtures/README.md`, so the test sources stay byte-identical. Both are
unmodified upstream files under the same MIT licence.

With that in place upstream's own suite runs on Windows: 103 tests.

## What was not vendored

- `src/main.rs` (the `bmapctl` CLI) and `build.rs` (which exists only to give
  that CLI a git hash).
- The Python and C++ implementations, the capture JSON files, and the
  capture tooling.

## Updating

Re-copy `rust/src/*.rs` (except `main.rs`) from a newer upstream commit,
convert to LF, re-apply the `lib.rs` attributes above, and update the commit
hash in this file. To confirm nothing else drifted:

```bash
git -C <upstream> show <commit>:rust/src/connection.rs | cmp - vendor/bose-bmap/src/connection.rs
```
