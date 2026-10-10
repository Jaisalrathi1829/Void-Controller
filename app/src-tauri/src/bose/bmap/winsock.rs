//! RFCOMM over Winsock (`AF_BTH` / `BTHPROTO_RFCOMM`).
//!
//! This is the one Windows-specific piece of the BMAP stack: a byte stream.
//! Everything above it — framing, endpoint validation, the protocol itself —
//! is platform-neutral and tested without a socket.
//!
//! Two things are done differently from this project's earlier probe tools:
//!
//! * **SDP resolution is left to Windows.** Connecting with a service class id
//!   and port 0 makes Windows resolve the RFCOMM channel itself. Windows does
//!   not then say which channel it chose (`getpeername` reports port 0), so
//!   that route's channel is reported as unknown rather than guessed.
//! * **No authentication or encryption socket options are set.** Upstream's
//!   Windows fork notes that a plain connection is what matches the Linux
//!   behaviour the protocol was documented against. The link is already
//!   encrypted at the baseband by pairing.

#![cfg(windows)]

use super::endpoint::{ConnectFailure, Connector, Opened, Route, SPP_SERVICE_UUID};
use super::stream::{ByteStream, Recv};
use std::io;
use std::sync::Once;
use std::time::Duration;
use windows::core::GUID;
use windows::Win32::Devices::Bluetooth::{AF_BTH, BTHPROTO_RFCOMM, SOCKADDR_BTH};
use windows::core::PSTR;
use windows::Win32::Networking::WinSock::{
    closesocket, connect, getpeername, getsockopt, ioctlsocket, recv, select, send, socket,
    WSAGetLastError, WSAStartup, FD_SET, FIONBIO, SEND_RECV_FLAGS, SOCKADDR, SOCKET, SOCKET_ERROR,
    SOCK_STREAM, SOL_SOCKET, SO_ERROR, TIMEVAL, WSADATA,
};

/// Longest a single connect attempt may take.
///
/// A blocking Bluetooth `connect` on Windows has no timeout of its own and was
/// measured hanging for a full 60 seconds against a service that does not
/// answer. Every wait in this stack is supposed to be bounded, so the connect
/// is made non-blocking and given this limit instead.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

/// `WSAEWOULDBLOCK`: a non-blocking connect is in progress.
const WSAEWOULDBLOCK: i32 = 10035;

static WSA_INIT: Once = Once::new();

/// Winsock is initialised once per process and left initialised: sockets here
/// are short-lived and frequent, and pairing every one with `WSACleanup` buys
/// nothing.
fn ensure_winsock() {
    WSA_INIT.call_once(|| unsafe {
        let mut data = WSADATA::default();
        let _ = WSAStartup(0x0202, &mut data);
    });
}

fn last_error() -> i32 {
    unsafe { WSAGetLastError().0 }
}

/// Describes a Winsock connect failure and says whether the device itself was
/// unreachable.
///
/// A timeout is deliberately *not* treated as "unreachable". One service on a
/// healthy, connected device was observed timing out while another answered
/// at once, so a timeout says something about the route, not about the
/// headphones. Only the errors Windows reserves for a host that is actually
/// gone end the search.
fn classify_connect_error(code: i32) -> ConnectFailure {
    let (hint, unreachable) = match code {
        10060 => ("this route did not answer in time", false),
        10064 => ("the device is down or powered off", true),
        10065 => ("no route to the device", true),
        10050 | 10051 => ("the Bluetooth network is unreachable", true),
        10061 => ("the connection was refused (no listener on this channel)", false),
        10048 => ("the channel is already in use", false),
        10049 | 10108 | 11001 | 11004 => ("the service was not found in the device's SDP record", false),
        10013 => ("access was denied", false),
        _ => ("unexpected Winsock error", false),
    };
    ConnectFailure {
        message: format!("{hint} (Winsock {code})"),
        unreachable,
    }
}

/// One record from the device's SDP database.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct SdpRecord {
    pub name: String,
    /// RFCOMM channel, or the L2CAP PSM for records that are not RFCOMM.
    pub port: u32,
    /// The raw record, as SDP data elements.
    pub raw: Vec<u8>,
}

/// Lists the SDP records a device returns for a service or protocol UUID.
///
/// Diagnostic only — nothing in the application calls this. It is how the
/// service table in `docs/protocol-notes.md` was produced, and it is kept so
/// the same table can be produced for another unit. A standard SDP query:
/// it opens no RFCOMM channel and sends nothing to any service.
#[cfg(test)]
pub(crate) fn sdp_records(address: u64, service: GUID) -> Vec<SdpRecord> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Networking::WinSock::{
        WSALookupServiceBeginW, WSALookupServiceEnd, WSALookupServiceNextW, LUP_FLUSHCACHE,
        LUP_RETURN_ADDR, LUP_RETURN_BLOB, LUP_RETURN_NAME, WSAQUERYSETW,
    };

    /// `NS_BTH`, the Bluetooth namespace for `WSALookupService*`.
    const NS_BTH: u32 = 16;

    ensure_winsock();

    // Windows wants the device as a string, "(AA:BB:CC:DD:EE:FF)".
    let b = address.to_be_bytes();
    let context: Vec<u16> = format!(
        "({:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X})",
        b[2], b[3], b[4], b[5], b[6], b[7]
    )
    .encode_utf16()
    .chain(std::iter::once(0))
    .collect();

    let mut records = Vec::new();
    unsafe {
        let mut class_id = service;
        let mut query: WSAQUERYSETW = std::mem::zeroed();
        query.dwSize = std::mem::size_of::<WSAQUERYSETW>() as u32;
        query.lpServiceClassId = &mut class_id;
        query.dwNameSpace = NS_BTH;
        query.lpszContext = PWSTR(context.as_ptr() as *mut u16);

        // LUP_FLUSHCACHE: ask the device, not Windows' cache of old answers.
        let flags = LUP_FLUSHCACHE | LUP_RETURN_ADDR | LUP_RETURN_BLOB | LUP_RETURN_NAME;
        let mut lookup = HANDLE::default();
        if WSALookupServiceBeginW(&query, flags, &mut lookup) != 0 {
            return records;
        }

        loop {
            // u64-backed so the result struct is suitably aligned.
            let mut buffer = vec![0u64; 2048];
            let mut length = (buffer.len() * std::mem::size_of::<u64>()) as u32;
            let rc = WSALookupServiceNextW(
                lookup,
                flags,
                &mut length,
                Some(buffer.as_mut_ptr() as *mut WSAQUERYSETW),
            );
            if rc != 0 {
                break;
            }

            let result = &*(buffer.as_ptr() as *const WSAQUERYSETW);
            let name = if result.lpszServiceInstanceName.is_null() {
                String::new()
            } else {
                result.lpszServiceInstanceName.to_string().unwrap_or_default()
            };
            let mut port = 0u32;
            if result.dwNumberOfCsAddrs > 0 && !result.lpcsaBuffer.is_null() {
                let remote = (*result.lpcsaBuffer).RemoteAddr;
                if !remote.lpSockaddr.is_null()
                    && remote.iSockaddrLength as usize >= std::mem::size_of::<SOCKADDR_BTH>()
                {
                    let sa = std::ptr::read_unaligned(remote.lpSockaddr as *const SOCKADDR_BTH);
                    port = sa.port;
                }
            }
            let mut raw = Vec::new();
            if !result.lpBlob.is_null() {
                let blob = &*result.lpBlob;
                if !blob.pBlobData.is_null() {
                    raw = std::slice::from_raw_parts(blob.pBlobData, blob.cbSize as usize).to_vec();
                }
            }
            records.push(SdpRecord { name, port, raw });
        }
        let _ = WSALookupServiceEnd(lookup);
    }
    records
}

fn fd_set_of(s: SOCKET) -> FD_SET {
    let mut set = FD_SET {
        fd_count: 1,
        fd_array: [SOCKET(0); 64],
    };
    set.fd_array[0] = s;
    set
}

fn timeval_of(d: Duration) -> TIMEVAL {
    TIMEVAL {
        tv_sec: d.as_secs().min(i32::MAX as u64) as i32,
        tv_usec: d.subsec_micros() as i32,
    }
}

/// A connected RFCOMM socket.
pub struct RfcommSocket {
    socket: SOCKET,
}

impl RfcommSocket {
    fn open(
        address: u64,
        service: GUID,
        port: u32,
        timeout: Duration,
    ) -> Result<Self, ConnectFailure> {
        ensure_winsock();
        unsafe {
            let s = socket(AF_BTH as i32, SOCK_STREAM, BTHPROTO_RFCOMM as i32)
                .map_err(|_| classify_connect_error(last_error()))?;
            // From here the socket is owned, so every early return closes it.
            let sock = Self { socket: s };

            let addr = SOCKADDR_BTH {
                addressFamily: AF_BTH,
                btAddr: address,
                serviceClassId: service,
                port,
            };

            // Non-blocking for the duration of the connect, so it can be
            // abandoned after `timeout` instead of whenever Windows gives up.
            let mut on: u32 = 1;
            if ioctlsocket(s, FIONBIO, &mut on) == SOCKET_ERROR {
                return Err(classify_connect_error(last_error()));
            }

            let rc = connect(
                s,
                &addr as *const SOCKADDR_BTH as *const SOCKADDR,
                std::mem::size_of::<SOCKADDR_BTH>() as i32,
            );
            if rc == SOCKET_ERROR {
                let code = last_error();
                if code != WSAEWOULDBLOCK {
                    return Err(classify_connect_error(code));
                }

                // In progress. Writable means connected; an exception means
                // the attempt failed and SO_ERROR says why.
                let mut writable = fd_set_of(s);
                let mut failed = fd_set_of(s);
                let tv = timeval_of(timeout);
                let ready = select(0, None, Some(&mut writable), Some(&mut failed), Some(&tv));
                if ready == SOCKET_ERROR {
                    return Err(classify_connect_error(last_error()));
                }
                if ready == 0 {
                    return Err(ConnectFailure {
                        message: format!(
                            "this route did not connect within {} s",
                            timeout.as_secs()
                        ),
                        unreachable: false,
                    });
                }
                if failed.fd_count > 0 {
                    let mut code: i32 = 0;
                    let mut len = std::mem::size_of::<i32>() as i32;
                    let _ = getsockopt(
                        s,
                        SOL_SOCKET,
                        SO_ERROR,
                        PSTR(&mut code as *mut i32 as *mut u8),
                        &mut len,
                    );
                    return Err(classify_connect_error(code));
                }
            }

            // Back to blocking: reads are bounded by `select` in `recv`.
            let mut off: u32 = 0;
            let _ = ioctlsocket(s, FIONBIO, &mut off);
            Ok(sock)
        }
    }

    /// The RFCOMM channel this socket is connected to, as Windows reports it.
    fn peer_channel(&self) -> Option<u8> {
        unsafe {
            let mut addr = SOCKADDR_BTH::default();
            let mut len = std::mem::size_of::<SOCKADDR_BTH>() as i32;
            let rc = getpeername(
                self.socket,
                &mut addr as *mut SOCKADDR_BTH as *mut SOCKADDR,
                &mut len,
            );
            if rc == SOCKET_ERROR {
                return None;
            }
            // Measured: for a connection made by service class id, Windows
            // reports port 0 here rather than the channel it resolved. Only a
            // real channel number is passed on.
            let port = addr.port;
            u8::try_from(port).ok().filter(|p| (1..=30).contains(p))
        }
    }
}

impl ByteStream for RfcommSocket {
    fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut sent = 0usize;
        while sent < bytes.len() {
            let n = unsafe { send(self.socket, &bytes[sent..], SEND_RECV_FLAGS(0)) };
            if n == SOCKET_ERROR {
                return Err(io::Error::from_raw_os_error(last_error()));
            }
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "the socket accepted no bytes",
                ));
            }
            sent += n as usize;
        }
        Ok(())
    }

    /// Waits with `select` rather than `SO_RCVTIMEO`: Winsock documents a
    /// socket whose receive timed out as being in an indeterminate state,
    /// whereas a `select` that times out leaves it untouched.
    fn recv(&mut self, timeout: Duration) -> io::Result<Recv> {
        unsafe {
            let mut readable = fd_set_of(self.socket);
            let tv = timeval_of(timeout);

            let ready = select(0, Some(&mut readable), None, None, Some(&tv));
            if ready == SOCKET_ERROR {
                return Err(io::Error::from_raw_os_error(last_error()));
            }
            if ready == 0 {
                return Ok(Recv::Timeout);
            }

            let mut buf = [0u8; 4096];
            let n = recv(self.socket, &mut buf, SEND_RECV_FLAGS(0));
            if n == SOCKET_ERROR {
                return Err(io::Error::from_raw_os_error(last_error()));
            }
            if n == 0 {
                return Ok(Recv::Closed);
            }
            Ok(Recv::Data(buf[..n as usize].to_vec()))
        }
    }
}

impl Drop for RfcommSocket {
    fn drop(&mut self) {
        unsafe {
            let _ = closesocket(self.socket);
        }
    }
}

/// Opens RFCOMM sockets to one Bluetooth device.
pub struct WinsockConnector {
    address: u64,
}

impl WinsockConnector {
    /// `address` is the 48-bit Bluetooth device address.
    pub fn new(address: u64) -> Self {
        Self { address }
    }
}

impl Connector for WinsockConnector {
    type Stream = RfcommSocket;

    fn connect(&mut self, route: Route) -> Result<Opened<RfcommSocket>, ConnectFailure> {
        let (service, port) = match route {
            // Port 0 with a service class id: Windows resolves the channel.
            Route::SerialPortService => (GUID::from_u128(SPP_SERVICE_UUID), 0),
            Route::Channel(channel) => (GUID::zeroed(), u32::from(channel)),
        };
        let socket = RfcommSocket::open(self.address, service, port, CONNECT_TIMEOUT)?;
        let channel = socket.peer_channel();
        Ok(Opened {
            stream: socket,
            channel,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_down_errors_stop_the_route_search() {
        for code in [10064, 10065, 10050, 10051] {
            assert!(classify_connect_error(code).unreachable, "code {code}");
        }
    }

    /// Measured on hardware: one service timed out while another on the same
    /// connected device answered immediately. A timeout must not abandon the
    /// routes that have not been tried yet.
    #[test]
    fn a_timeout_on_one_route_does_not_condemn_the_device() {
        assert!(!classify_connect_error(10060).unreachable);
    }

    /// A refused channel or a missing SDP record means "try the next route",
    /// not "the headphones are gone".
    #[test]
    fn refused_and_not_found_let_the_search_continue() {
        for code in [10061, 10048, 10049, 10108] {
            assert!(!classify_connect_error(code).unreachable, "code {code}");
        }
    }

    #[test]
    fn failure_message_keeps_the_winsock_code() {
        assert!(classify_connect_error(10061).message.contains("10061"));
    }
}
