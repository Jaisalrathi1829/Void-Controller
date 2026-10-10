//! BMAP: Bose's control protocol, over RFCOMM.
//!
//! The protocol itself — packet codec, operators, error codes, per-model
//! configuration, reply validation — is **not implemented here**. It comes
//! from the `bmap` crate, vendored from
//! [aaronsb/bosectl](https://github.com/aaronsb/bosectl) (MIT, Copyright (c)
//! 2026 Aaron Bockelie). See `vendor/bose-bmap/VENDORED.md`.
//!
//! Upstream's own transport is Linux-only, so what lives in this module is the
//! part Windows needs and the discipline this application adds on top:
//!
//! | Module | Role |
//! | --- | --- |
//! | [`stream`] | Reassembles BMAP frames from a raw byte stream; implements upstream's `Transport` trait. |
//! | [`endpoint`] | Chooses a route (SDP first, documented channels as fallback) and proves it speaks BMAP before it is used. |
//! | [`noise`] | Mode switching confirmed by reading the mode back. |
//! | [`custom`] | The user-configurable modes: listing them, and adjusting the active one's level. |
//! | [`eq`] | The three-band hardware equalizer, read strictly and written with read-back. |
//! | [`link`] | One controlled connection at a time; reconnect, backoff, last confirmed state. |
//! | [`winsock`] | The RFCOMM socket. The only platform-specific code. |
//!
//! Nothing in this module is reachable from the frontend as raw bytes. The UI
//! can ask for "Quiet", "Aware" or "Custom", and for a level from 0 to 10; the
//! frames are built by upstream's codec from upstream's model configuration.

pub mod custom;
pub mod endpoint;
pub mod eq;
pub mod link;
pub mod noise;
pub mod stream;

#[cfg(windows)]
pub mod winsock;

#[cfg(all(test, windows))]
pub(crate) mod hardware_tests;

pub use custom::{LevelChange, Slot};
pub use eq::{EqChange, EqLevels};
pub use link::{BmapLink, LinkError, LinkErrorKind, LinkStatus, LinkTiming};
pub use noise::{ModeChange, Preset, Target, Verdict};

/// Product ids this application will open a BMAP control link to.
///
/// Deliberately an allowlist rather than "anything in upstream's catalog".
/// Upstream supports more models, and several share this protocol layout, but
/// this application has only ever been run against one. Extending the list is
/// a decision to make with the hardware in hand.
pub const SUPPORTED_PRODUCT_IDS: &[u16] = &[
    0x4075, // QuietComfort Headphones ("prince")
];

/// Upstream's device configuration for a product id, if this application
/// supports it.
pub fn config_for_product(product_id: u16) -> Option<bmap::DeviceConfig> {
    if !SUPPORTED_PRODUCT_IDS.contains(&product_id) {
        return None;
    }
    let entry = bmap::catalog::lookup_device(product_id)?;
    bmap::devices::get_device(entry.config?)
}

/// The connector used for real hardware on this platform.
#[cfg(windows)]
pub type PlatformConnector = winsock::WinsockConnector;

#[cfg(not(windows))]
pub type PlatformConnector = unsupported::UnsupportedConnector;

/// Stand-in for platforms without an RFCOMM implementation, so the rest of
/// the crate compiles unchanged. Every connect fails with an explanation.
#[cfg(not(windows))]
pub mod unsupported {
    use super::endpoint::{ConnectFailure, Connector, Opened, Route};
    use super::stream::{ByteStream, Recv};
    use std::io;
    use std::time::Duration;

    pub struct NoStream;

    impl ByteStream for NoStream {
        fn send(&mut self, _bytes: &[u8]) -> io::Result<()> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "no RFCOMM on this platform"))
        }
        fn recv(&mut self, _timeout: Duration) -> io::Result<Recv> {
            Ok(Recv::Closed)
        }
    }

    pub struct UnsupportedConnector;

    impl UnsupportedConnector {
        pub fn new(_address: u64) -> Self {
            Self
        }
    }

    impl Connector for UnsupportedConnector {
        type Stream = NoStream;
        fn connect(&mut self, _route: Route) -> Result<Opened<NoStream>, ConnectFailure> {
            Err(ConnectFailure {
                message: "the BMAP control link is only implemented for Windows".into(),
                unreachable: true,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_device_resolves_to_upstreams_prince_config() {
        let config = config_for_product(0x4075).expect("0x4075 is supported");
        assert_eq!(config.info.codename, "prince");
        assert_eq!(config.rfcomm_channel, 8);
        assert!(config.current_mode.is_some());
    }

    /// Upstream knows these models, but this application has never been run
    /// against them, so it must not open a control link to them.
    #[test]
    fn models_upstream_supports_but_we_have_not_tested_are_refused() {
        for untested in [0x4082u16, 0x4039, 0x4020, 0x402F] {
            assert!(
                bmap::catalog::is_supported(untested),
                "test premise: upstream supports {untested:#06x}"
            );
            assert!(
                config_for_product(untested).is_none(),
                "{untested:#06x} must stay refused until tested on hardware"
            );
        }
    }

    #[test]
    fn unknown_products_are_refused() {
        assert!(config_for_product(0xFFFF).is_none());
        assert!(config_for_product(0x0000).is_none());
    }
}
