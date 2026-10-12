//! Tests that talk to physical headphones.
//!
//! Every test here is `#[ignore]`d: they need a paired, connected Bose device
//! and would fail for the wrong reason anywhere else. Run them explicitly:
//!
//! ```text
//! cargo test --manifest-path app/src-tauri/Cargo.toml -- --ignored --nocapture --test-threads=1 hw_
//! ```
//!
//! `--test-threads=1` matters: the headphones accept one control connection
//! at a time, and these tests must not race each other for it.
//!
//! The tests in this file are **read-only** — they send GET frames only. The
//! tests that change the noise-control mode live with `RealBoseDevice`, so
//! they exercise the application's own code path.

#![cfg(all(test, windows))]

use super::endpoint::Endpoint;
use super::link::{BmapLink, LinkTiming};
use super::noise::describe_index;
use super::winsock::WinsockConnector;
use crate::bluetooth::pnp;

pub(crate) struct Headphones {
    pub name: String,
    pub address: u64,
    pub product_id: u16,
}

/// Finds the connected Bose device by SIG vendor id.
pub(crate) fn find_headphones() -> Headphones {
    let dev = pnp::enumerate_bluetooth_devices()
        .into_iter()
        .filter(|d| d.is_top_level())
        .find(|d| crate::bose::is_bose_vendor(d.vendor_id))
        .expect("no paired Bose device found (SIG vendor id 0x009E)");

    assert_eq!(
        dev.is_connected,
        Some(true),
        "the Bose device is paired but not connected; power it on and connect it to this PC"
    );

    let address_hex = pnp::device_address(&dev.instance_id).expect("device address");
    Headphones {
        name: dev.friendly_name.unwrap_or_else(|| "Bose device".into()),
        address: u64::from_str_radix(&address_hex, 16).expect("hex address"),
        product_id: dev.product_id.expect("product id on a profile node"),
    }
}

pub(crate) fn open_link(hp: &Headphones) -> BmapLink<WinsockConnector> {
    let config = super::config_for_product(hp.product_id).unwrap_or_else(|| {
        panic!(
            "product {:#06x} is not on this application's supported list",
            hp.product_id
        )
    });
    BmapLink::new(
        WinsockConnector::new(hp.address),
        config,
        LinkTiming::standard(),
        crate::device::now_rfc3339,
    )
}

pub(crate) fn print_wire_log(link: &mut BmapLink<WinsockConnector>) {
    for event in link.take_wire_log() {
        println!("    {}", event.render());
    }
}

fn print_endpoint(endpoint: &Endpoint) {
    println!("  endpoint : {}", endpoint.describe());
    println!("  firmware : {}", endpoint.firmware);
}

/// Stage A: establish a validated BMAP connection and read the current mode.
/// Sends two kinds of frame, both GETs: firmware and current mode.
#[test]
#[ignore = "requires the physical headphones"]
fn hw_a1_validates_endpoint_and_reads_mode() {
    let hp = find_headphones();
    println!("\n  device   : {} (product {:#06x})", hp.name, hp.product_id);

    let mut link = open_link(&hp);
    let result = link.read_mode(true);

    println!("  routes tried:");
    for attempt in link.last_attempts() {
        println!("    - {}", attempt.render());
        for event in &attempt.wire {
            println!("        {}", event.render());
        }
    }
    println!("  wire:");
    print_wire_log(&mut link);

    let index = result.unwrap_or_else(|e| panic!("could not read the mode: {e}"));
    let status = link.status();
    print_endpoint(status.endpoint.as_ref().expect("an open endpoint"));
    println!(
        "  mode     : index {index} ({})",
        describe_index(link.config(), index)
    );

    assert!(
        status.framing_problem.is_none(),
        "mode reply did not match the documented framing: {:?}",
        status.framing_problem
    );
    assert_eq!(status.confirmed.expect("confirmed mode").index, index);
}

/// Reconnection: close the link, open it again, and read again.
#[test]
#[ignore = "requires the physical headphones"]
fn hw_a2_reconnects_after_close() {
    let hp = find_headphones();
    let mut link = open_link(&hp);

    let first = link.read_mode(true).expect("first read");
    link.close();
    assert!(!link.is_open());

    // Give the headset a moment to finish tearing the channel down, as
    // upstream's connect backoff does.
    std::thread::sleep(std::time::Duration::from_millis(600));

    let second = link.read_mode(true).unwrap_or_else(|e| {
        for attempt in link.last_attempts() {
            println!("    - {}", attempt.render());
        }
        panic!("could not read after reconnecting: {e}")
    });
    println!("\n  before close: index {first}; after reconnect: index {second}");
    assert!(link.is_open());
}

/// Several reads on one connection: the session is reused and stays in step.
#[test]
#[ignore = "requires the physical headphones"]
fn hw_a3_repeated_reads_on_one_connection() {
    let hp = find_headphones();
    let mut link = open_link(&hp);

    let mut seen = Vec::new();
    for _ in 0..5 {
        seen.push(link.read_mode(true).expect("read"));
    }
    println!("\n  five reads: {seen:?}");
    assert!(
        seen.windows(2).all(|w| w[0] == w[1]),
        "mode changed between reads with nothing sent: {seen:?}"
    );
}

/// Diagnostic, not a pass/fail test: shows what each route does when sent the
/// read-only firmware GET. Run on its own with `hw_diag_routes`.
///
/// Only the routes the application itself uses are exercised. The BMAP-UUID
/// service is deliberately left alone: see the notes in `endpoint.rs`.
#[test]
#[ignore = "requires the physical headphones; diagnostic output only"]
fn hw_diag_routes() {
    use super::endpoint::{Connector, Route};
    use super::stream::{StreamTransport, Timing};
    use bmap::protocol::{bmap_packet, Operator};
    use bmap::Transport;

    let hp = find_headphones();
    let routes = vec![Route::SerialPortService, Route::Channel(8)];

    for route in routes {
        println!("\n  === {} ===", route.describe());
        let mut connector = WinsockConnector::new(hp.address);
        let started = std::time::Instant::now();
        let opened = match connector.connect(route) {
            Ok(o) => o,
            Err(f) => {
                println!("  connect failed after {:?}: {}", started.elapsed(), f.message);
                continue;
            }
        };
        println!(
            "  connected in {:?}; channel reported by Windows: {:?}",
            started.elapsed(),
            opened.channel
        );

        let transport = StreamTransport::new(opened.stream, Timing::standard());
        // Listen before sending anything, to separate a greeting from a reply.
        transport.drain_greeting();
        for e in transport.take_wire_log() {
            println!("    (before any send) {}", e.render());
        }

        let get_firmware = bmap_packet(0, 5, Operator::Get, &[]);
        match transport.send_recv(&get_firmware) {
            Ok(_) => println!("  firmware GET: answered"),
            Err(e) => println!("  firmware GET: {e}"),
        }
        for e in transport.take_wire_log() {
            println!("    {}", e.render());
        }
        drop(transport);
        std::thread::sleep(std::time::Duration::from_millis(800));
    }
}

/// Diagnostic: the device's SDP records, as Windows reads them. Opens no
/// RFCOMM channel and sends nothing to any service.
///
/// This is the evidence behind the service table in `docs/protocol-notes.md`.
#[test]
#[ignore = "requires the physical headphones; diagnostic output only"]
fn hw_diag_sdp_records() {
    use super::endpoint::{BMAP_SERVICE_UUID, SPP_SERVICE_UUID};
    use windows::core::GUID;

    let hp = find_headphones();
    let base = |short: u128| GUID::from_u128((short << 96) | 0x0000_1000_8000_0080_5F9B_34FB);
    let queries = [
        ("every record (L2CAP protocol, 0x0100)", base(0x0100)),
        ("Serial Port service (0x1101)", GUID::from_u128(SPP_SERVICE_UUID)),
        ("BMAP service", GUID::from_u128(BMAP_SERVICE_UUID)),
    ];

    for (label, guid) in queries {
        println!("\n  {label}:");
        let records = super::winsock::sdp_records(hp.address, guid);
        if records.is_empty() {
            println!("    (no records)");
        }
        for r in records {
            // ServiceClassIDList is attribute 0x0001, a sequence of UUIDs:
            // 09 00 01 35 <len> <uuid elements>.
            let classes = r
                .raw
                .windows(4)
                .position(|w| w == [0x09, 0x00, 0x01, 0x35])
                .and_then(|i| {
                    let len = *r.raw.get(i + 4)? as usize;
                    r.raw.get(i + 5..i + 5 + len)
                })
                .map(super::stream::hex)
                .unwrap_or_default();
            println!(
                "    port {:>2}  {:<18} classes [{classes}]",
                r.port,
                format!("{:?}", r.name)
            );
        }
    }
}

/// Diagnostic: which channel does the Windows-resolved Serial Port route land
/// on? Windows does not say, but RFCOMM carries one connection per channel
/// between two devices, so if a documented channel cannot be opened while the
/// resolved connection is held (and vice versa), they are the same channel.
///
/// The one place two connections are attempted at once. The second socket
/// never gets as far as sending anything.
#[test]
#[ignore = "requires the physical headphones; diagnostic output only"]
fn hw_diag_resolved_channel() {
    use super::endpoint::{Connector, Route};
    use super::stream::{StreamTransport, Timing};
    use bmap::protocol::{bmap_packet, Operator};
    use bmap::Transport;

    let hp = find_headphones();
    let documented = Route::Channel(open_link(&hp).config().rfcomm_channel);

    for (held, probe) in [
        (Route::SerialPortService, documented),
        (documented, Route::SerialPortService),
    ] {
        println!("\n  holding: {}", held.describe());
        let opened = match WinsockConnector::new(hp.address).connect(held) {
            Ok(o) => o,
            Err(f) => {
                println!("    could not open: {}", f.message);
                continue;
            }
        };
        let transport = StreamTransport::new(opened.stream, Timing::standard());
        transport.drain_greeting();
        let answered = transport
            .send_recv(&bmap_packet(0, 5, Operator::Get, &[]))
            .is_ok();
        println!("    firmware GET on the held connection: answered = {answered}");

        match WinsockConnector::new(hp.address).connect(probe) {
            Ok(second) => {
                println!("    {} also opened: different channels", probe.describe());
                drop(second);
            }
            Err(f) => println!("    {} while held: {}", probe.describe(), f.message),
        }

        drop(transport);
        std::thread::sleep(std::time::Duration::from_millis(1200));
    }
}

/// Diagnostic: the mode slots the headphones report, raw and parsed.
///
/// Read-only. `START [31.1]` asks the device to list its mode configurations;
/// it changes nothing. This is the check that the slot layout on this unit is
/// the documented one, made before anything that writes a slot is trusted.
#[test]
#[ignore = "requires the physical headphones; diagnostic output only"]
fn hw_diag_mode_slots() {
    use super::endpoint::{Connector, Route};
    use super::stream::{hex, StreamTransport, Timing};
    use bmap::protocol::Operator;
    use bmap::BmapConnection;

    let hp = find_headphones();
    let config = super::config_for_product(hp.product_id).expect("supported model");
    let opened = WinsockConnector::new(hp.address)
        .connect(Route::SerialPortService)
        .unwrap_or_else(|f| panic!("could not open: {}", f.message));
    let transport = StreamTransport::new(opened.stream, Timing::standard());
    transport.drain_greeting();
    let conn = BmapConnection::new(transport.clone(), config.clone());

    println!("\n  current mode index: {:?}", conn.mode_idx());

    let all = config.get_all_modes.expect("model lists modes");
    let mode_config = config.mode_config.expect("model has mode configs");
    let parser = config.parse_mode_config.expect("model has a parser");
    let asked = std::time::Instant::now();
    let replies = conn.start_drain(all, &[]).expect("mode list");
    println!("  mode list read in {:?}", asked.elapsed());

    // And a few more times, for how long it usually takes. Listing is a read:
    // asking again changes nothing.
    let mut times = Vec::new();
    for _ in 0..8 {
        std::thread::sleep(std::time::Duration::from_millis(150));
        let asked = std::time::Instant::now();
        let again = conn.start_drain(all, &[]).expect("mode list");
        times.push(asked.elapsed());
        assert_eq!(again.len(), replies.len(), "the same frames every time");
        let last = again.last().expect("a reply");
        assert_eq!(
            (last.fblock, last.func, last.op),
            (all.0, all.1, Operator::Result),
            "the reply ends with RESULT from the address that was asked"
        );
    }
    times.sort();
    println!(
        "  eight more: fastest {:?}, median {:?}, slowest {:?}",
        times[0],
        times[times.len() / 2],
        times[times.len() - 1]
    );
    transport.take_wire_log();

    for r in &replies {
        if (r.fblock, r.func) != (mode_config.0, mode_config.1) || r.op != Operator::Status {
            println!("  other frame: {}", r.fmt());
            continue;
        }
        println!("\n  [{} bytes] {}", r.payload.len(), hex(&r.payload));
        match parser(&r.payload) {
            Some(m) => println!(
                "    index {}  name {:?}  raw cnc {}  wind {}  spatial {}  editable {}  configured {}",
                m.mode_idx, m.name, m.cnc_level, m.wind_block, m.spatial, m.editable, m.configured
            ),
            None => println!("    (did not parse)"),
        }
    }
    println!("\n  wire:");
    for e in transport.take_wire_log() {
        println!("    {}", e.render());
    }
}

/// Asks the headphones which product they are: one `GET [0.3]`, the address
/// Bose's product-information block uses for the product id. Read-only.
///
/// Windows reports the product id itself, so the desktop application never
/// asks. A platform that does not (Android) has to, and this is the check
/// that the answer is what Windows says it is.
#[test]
#[ignore = "requires the physical headphones; diagnostic output only"]
fn hw_diag_product_id() {
    use super::endpoint::{Connector, Route};
    use super::stream::{hex, StreamTransport, Timing};
    use bmap::transport::Transport;

    let hp = find_headphones();
    let opened = WinsockConnector::new(hp.address)
        .connect(Route::SerialPortService)
        .unwrap_or_else(|f| panic!("could not open: {}", f.message));
    let transport = StreamTransport::new(opened.stream, Timing::standard());
    transport.drain_greeting();
    let reply = transport.send_recv(&[0x00, 0x03, 0x01, 0x00]);
    println!("\n  Windows says product id: 0x{:04X}", hp.product_id);
    match &reply {
        Ok(bytes) => println!("  GET [0.3] -> {}", hex(bytes)),
        Err(e) => println!("  GET [0.3] failed: {e}"),
    }
    for e in transport.take_wire_log() {
        println!("    {}", e.render());
    }
}

/// Asks the headphones for their battery level over the control protocol and
/// sets it beside the level Windows reports. Read-only.
///
/// Android does not always pass a Bluetooth device's battery level on to
/// applications, so there the protocol's own answer is the fallback. This is
/// the check that it is the same number.
#[test]
#[ignore = "requires the physical headphones; diagnostic output only"]
fn hw_diag_battery() {
    use super::endpoint::{Connector, Route};
    use super::stream::{StreamTransport, Timing};
    use bmap::BmapConnection;

    let hp = find_headphones();
    let config = super::config_for_product(hp.product_id).expect("supported model");
    let opened = WinsockConnector::new(hp.address)
        .connect(Route::SerialPortService)
        .unwrap_or_else(|f| panic!("could not open: {}", f.message));
    let transport = StreamTransport::new(opened.stream, Timing::standard());
    transport.drain_greeting();
    let conn = BmapConnection::new(transport.clone(), config);

    let windows = crate::bose::real::sight_device(None).and_then(|s| s.battery);
    let status = conn.battery_status();
    println!("\n  Windows reports      : {windows:?}");
    println!("  the headphones report: {status:?}");
    for e in transport.take_wire_log() {
        println!("    {}", e.render());
    }
    let level = status.expect("the headphones answered").aggregate;
    assert!(level <= 100);
}
