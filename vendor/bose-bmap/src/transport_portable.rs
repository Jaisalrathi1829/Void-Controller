//! [QC Control Center patch] Portable `Transport` trait.
//!
//! Not an upstream file. Upstream's `transport.rs` defines this trait alongside
//! a Linux `AF_BLUETOOTH` socket implementation, which does not compile on
//! Windows (`std::os::fd`, `libc`). On non-unix targets `lib.rs` mounts this
//! file as `crate::transport` instead, so the platform-neutral modules
//! (`connection`, `protocol`, `device`, `devices`, `catalog`) build unchanged
//! and the application supplies its own transport.
//!
//! The trait below is copied verbatim from upstream `transport.rs`
//! (Copyright (c) 2026 Aaron Bockelie, MIT — see ../LICENSE).

use crate::error::BmapResult;

/// Transport abstraction for sending/receiving BMAP packets.
///
/// Implemented by `RfcommTransport` for real Bluetooth. Can be mocked for testing.
pub trait Transport {
    fn send_recv(&self, packet: &[u8]) -> BmapResult<Vec<u8>>;
    fn send_recv_drain(&self, packet: &[u8]) -> BmapResult<Vec<u8>>;
}
