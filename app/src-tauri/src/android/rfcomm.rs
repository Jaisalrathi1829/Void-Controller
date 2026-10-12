//! RFCOMM through Android's `BluetoothSocket`.
//!
//! The counterpart of the Winsock connector. The socket itself lives on the
//! Kotlin side, one at a time; this hands it bytes and takes bytes back. What
//! those bytes are, and whether an answer is acceptable, is decided above —
//! the stream knows nothing of BMAP.

use super::bridge;
use crate::bose::bmap::endpoint::{ConnectFailure, Connector, Opened, Route};
use crate::bose::bmap::stream::{ByteStream, Recv};
use serde::Deserialize;
use std::io;
use std::time::Duration;

/// `AA:BB:CC:DD:EE:FF`, as Android writes a 48-bit address.
pub fn address_text(address: u64) -> String {
    let b = address.to_be_bytes();
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        b[2], b[3], b[4], b[5], b[6], b[7]
    )
}

/// The reverse of [`address_text`].
pub fn parse_address(text: &str) -> Option<u64> {
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() != 6 || parts.iter().any(|p| p.len() != 2) {
        return None;
    }
    parts
        .iter()
        .try_fold(0u64, |acc, p| Some((acc << 8) | u64::from(u8::from_str_radix(p, 16).ok()?)))
}

pub struct AndroidConnector {
    address: u64,
}

impl AndroidConnector {
    /// `address` is the 48-bit Bluetooth device address.
    pub fn new(address: u64) -> Self {
        Self { address }
    }
}

/// The open connection. Dropping it closes the socket.
pub struct AndroidStream {
    _private: (),
}

impl Connector for AndroidConnector {
    type Stream = AndroidStream;

    fn connect(&mut self, route: Route) -> Result<Opened<AndroidStream>, ConnectFailure> {
        #[derive(Deserialize)]
        struct Reply {
            ok: bool,
            #[serde(default)]
            message: String,
            #[serde(default)]
            unreachable: bool,
        }

        let address = address_text(self.address);
        let payload = match route {
            // Android resolves the channel from the device's own record.
            Route::SerialPortService => serde_json::json!({ "address": address }),
            Route::Channel(channel) => serde_json::json!({ "address": address, "channel": channel }),
        };
        let reply: Reply = bridge::call("rfcommOpen", payload).map_err(|message| ConnectFailure {
            message,
            unreachable: true,
        })?;
        if !reply.ok {
            bridge::log(&format!(
                "{} did not open: {}{}",
                route.describe(),
                reply.message,
                if reply.unreachable { " (device out of reach)" } else { "" }
            ));
            return Err(ConnectFailure {
                message: reply.message,
                unreachable: reply.unreachable,
            });
        }
        Ok(Opened {
            stream: AndroidStream { _private: () },
            // Android does not say which channel a service resolved to.
            channel: match route {
                Route::SerialPortService => None,
                Route::Channel(channel) => Some(channel),
            },
        })
    }
}

impl ByteStream for AndroidStream {
    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        bridge::run("rfcommSend", serde_json::json!({ "data": bridge::to_hex(bytes) }))
            .map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e))
    }

    fn recv(&mut self, timeout: Duration) -> io::Result<Recv> {
        #[derive(Deserialize)]
        struct Reply {
            kind: String,
            #[serde(default)]
            data: String,
        }
        let millis = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let reply: Reply = bridge::call("rfcommRecv", serde_json::json!({ "timeoutMs": millis }))
            .map_err(io::Error::other)?;
        match reply.kind.as_str() {
            "data" => match bridge::from_hex(&reply.data) {
                Some(bytes) if !bytes.is_empty() => Ok(Recv::Data(bytes)),
                Some(_) => Ok(Recv::Timeout),
                None => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the bridge returned bytes that were not hexadecimal",
                )),
            },
            "timeout" => Ok(Recv::Timeout),
            _ => Ok(Recv::Closed),
        }
    }
}

impl Drop for AndroidStream {
    fn drop(&mut self) {
        let _ = bridge::run("rfcommClose", ());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_written_the_way_android_writes_it() {
        assert_eq!(address_text(0xE458_BCF9_F02E), "E4:58:BC:F9:F0:2E");
        assert_eq!(address_text(0x0000_0000_0001), "00:00:00:00:00:01");
    }

    #[test]
    fn an_address_reads_back_to_the_same_number() {
        assert_eq!(parse_address("E4:58:BC:F9:F0:2E"), Some(0xE458_BCF9_F02E));
        assert_eq!(parse_address("e4:58:bc:f9:f0:2e"), Some(0xE458_BCF9_F02E));
        assert_eq!(parse_address(&address_text(0x1234_5678_9ABC)), Some(0x1234_5678_9ABC));
    }

    #[test]
    fn things_that_are_not_addresses_are_refused() {
        for text in [
            "",
            "E4:58:BC:F9:F0",
            "E4:58:BC:F9:F0:2E:00",
            "E458BCF9F02E",
            "G4:58:BC:F9:F0:2E",
            "E4:5:8BC:F9:F0:2E",
        ] {
            assert_eq!(parse_address(text), None, "{text:?}");
        }
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn a_connection_that_cannot_be_attempted_is_a_failure_not_a_stream() {
        let failure = AndroidConnector::new(0xE458_BCF9_F02E)
            .connect(Route::SerialPortService)
            .err()
            .expect("there is no Android here");
        assert!(failure.unreachable);
    }
}
