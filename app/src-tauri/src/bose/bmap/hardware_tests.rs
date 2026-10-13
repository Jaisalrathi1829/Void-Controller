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

/// Asks the headphones four things this application has not asked them
/// before: their own name (`GET [1.2]`), whether they will hold two
/// connections at once (`GET [1.10]`), which devices they are paired with
/// (`GET [4.4]`), and for each of those what it is called and whether it is
/// connected (`GET [4.5]` with its address). All are requests for state:
/// nothing is set, connected, disconnected or removed.
///
/// The addresses that come back belong to the owner's phone and computers.
/// They are printed with their middle masked, and nothing here is kept.
#[test]
#[ignore = "requires the physical headphones; diagnostic output only"]
fn hw_diag_device_settings() {
    use super::endpoint::{Connector, Route};
    use super::stream::{hex, StreamTransport, Timing};
    use bmap::protocol::{parse_all_responses, Operator};
    use bmap::transport::Transport;

    let hp = find_headphones();
    let opened = WinsockConnector::new(hp.address)
        .connect(Route::SerialPortService)
        .unwrap_or_else(|f| panic!("could not open: {}", f.message));
    let transport = StreamTransport::new(opened.stream, Timing::standard());
    transport.drain_greeting();

    // Six address bytes with the middle four hidden.
    let masked = |bytes: &[u8]| -> String {
        bytes
            .chunks(6)
            .map(|mac| match mac {
                [first, _, _, _, _, last] => format!("{first:02X}:**:**:**:**:{last:02X}"),
                other => hex(other),
            })
            .collect::<Vec<_>>()
            .join(" ")
    };

    println!("\n  Windows calls them: {:?}", hp.name);

    for (what, packet) in [
        ("GET [1.2]  name      ", vec![0x01, 0x02, 0x01, 0x00]),
        ("GET [1.10] multipoint", vec![0x01, 0x0A, 0x01, 0x00]),
    ] {
        match transport.send_recv(&packet) {
            Ok(bytes) => {
                println!("  {what} -> {}", hex(&bytes));
                for frame in parse_all_responses(&bytes) {
                    println!(
                        "      [{}.{}] {:?}  {:?}",
                        frame.fblock,
                        frame.func,
                        frame.op,
                        String::from_utf8_lossy(&frame.payload)
                    );
                }
            }
            Err(e) => println!("  {what} failed: {e}"),
        }
    }

    let list = transport.send_recv(&[0x04, 0x04, 0x01, 0x00]);
    let mut addresses: Vec<[u8; 6]> = Vec::new();
    match &list {
        Ok(bytes) => {
            for frame in parse_all_responses(bytes) {
                if (frame.fblock, frame.func, frame.op) != (4, 4, Operator::Status) {
                    println!("  GET [4.4]  list       -> other frame: {}", frame.fmt());
                    continue;
                }
                let (head, rest) = frame.payload.split_at(frame.payload.len().min(1));
                println!(
                    "  GET [4.4]  list       -> {} payload byte(s): first {} then {}",
                    frame.payload.len(),
                    hex(head),
                    masked(rest)
                );
                for mac in rest.chunks_exact(6) {
                    addresses.push(mac.try_into().expect("six bytes"));
                }
            }
        }
        Err(e) => println!("  GET [4.4]  list       failed: {e}"),
    }

    for mac in &addresses {
        let mut packet = vec![0x04, 0x05, 0x01, 0x06];
        packet.extend_from_slice(mac);
        match transport.send_recv(&packet) {
            Ok(bytes) => {
                for frame in parse_all_responses(&bytes) {
                    if (frame.fblock, frame.func) != (4, 5) {
                        println!("  GET [4.5]  info       -> other frame: {}", frame.fmt());
                        continue;
                    }
                    let p = &frame.payload;
                    let (mac_part, after) = p.split_at(p.len().min(6));
                    // What follows the address, byte by byte, and as text
                    // from each plausible start, so the layout can be read
                    // off rather than assumed.
                    println!(
                        "  GET [4.5]  info {}  {:?}  {} byte(s) after the address: {}",
                        masked(mac_part),
                        frame.op,
                        after.len(),
                        hex(&after[..after.len().min(4)])
                    );
                    for skip in 1..=3usize {
                        if after.len() > skip {
                            println!(
                                "      text after {skip} byte(s): {:?}",
                                String::from_utf8_lossy(&after[skip..])
                            );
                        }
                    }
                }
            }
            Err(e) => println!("  GET [4.5]  info {} failed: {e}", masked(mac)),
        }
    }

    // The wire log is not printed: it holds the addresses in full.
    transport.take_wire_log();
}

/// Finds out what switching multipoint off and on again looks like on the
/// wire, and what it does to the connections. **This one writes**: it sends
/// `SETGET [1.10]` with 0, watches, then sends it with 1 — or the other way
/// round if multipoint was off to begin with — so the headphones end as they
/// began. Run only with the owner's agreement: with multipoint off the
/// headphones keep one connection, and it may not be this computer's.
///
/// If the link is lost when multipoint goes off, the test keeps trying to
/// reopen it for a while to put the setting back, and says loudly if it
/// could not.
#[test]
#[ignore = "requires the physical headphones; changes multipoint and changes it back"]
fn hw_diag_multipoint_toggle() {
    use super::endpoint::{Connector, Route};
    use super::stream::{hex, StreamTransport, Timing};
    use bmap::protocol::{parse_all_responses, Operator};
    use bmap::transport::Transport;
    use std::time::{Duration, Instant};

    let hp = find_headphones();
    let open = || {
        WinsockConnector::new(hp.address)
            .connect(Route::SerialPortService)
            .map(|opened| {
                let t = StreamTransport::new(opened.stream, Timing::standard());
                t.drain_greeting();
                t
            })
    };
    let mut transport = open().unwrap_or_else(|f| panic!("could not open: {}", f.message));

    // The one byte `GET [1.10]` answers with, or why not.
    let state = |t: &StreamTransport<_>| -> Result<Vec<u8>, String> {
        let bytes = t.send_recv(&[0x01, 0x0A, 0x01, 0x00]).map_err(|e| e.to_string())?;
        parse_all_responses(&bytes)
            .into_iter()
            .find(|f| (f.fblock, f.func) == (1, 10))
            .map(|f| {
                if f.op == Operator::Status {
                    Ok(f.payload)
                } else {
                    Err(f.fmt())
                }
            })
            .unwrap_or_else(|| Err(format!("no [1.10] frame in {}", hex(&bytes))))
    };
    // Each paired device's status byte and name, addresses left out.
    let devices = |t: &StreamTransport<_>| -> String {
        let Ok(bytes) = t.send_recv(&[0x04, 0x04, 0x01, 0x00]) else {
            return "list failed".into();
        };
        let Some(list) = parse_all_responses(&bytes)
            .into_iter()
            .find(|f| (f.fblock, f.func, f.op) == (4, 4, Operator::Status))
        else {
            return format!("no list in {}", hex(&bytes));
        };
        let mut out = format!("first byte {:02X};", list.payload.first().copied().unwrap_or(0));
        for mac in list.payload.get(1..).unwrap_or(&[]).chunks_exact(6) {
            let mut packet = vec![0x04, 0x05, 0x01, 0x06];
            packet.extend_from_slice(mac);
            match t.send_recv(&packet) {
                Ok(bytes) => {
                    for f in parse_all_responses(&bytes) {
                        if (f.fblock, f.func, f.op) == (4, 5, Operator::Status) && f.payload.len() >= 9 {
                            out.push_str(&format!(
                                " [{} {:?}]",
                                hex(&f.payload[6..9]),
                                String::from_utf8_lossy(&f.payload[9..])
                            ));
                        } else {
                            out.push_str(&format!(" [{}]", f.fmt().len()));
                        }
                    }
                }
                Err(e) => out.push_str(&format!(" [failed: {e}]")),
            }
        }
        out
    };

    let began = state(&transport).expect("multipoint could be read before anything was sent");
    println!("\n  before: [1.10] = {}   devices: {}", hex(&began), devices(&transport));
    let was_on = began.first().is_some_and(|b| b & 0x02 != 0);
    println!("  multipoint was {}", if was_on { "ON" } else { "OFF" });

    let set = |t: &StreamTransport<_>, on: bool| {
        let packet = [0x01, 0x0A, 0x02, 0x01, u8::from(on)];
        let asked = Instant::now();
        match t.send_recv(&packet) {
            Ok(bytes) => {
                println!("  SETGET [1.10] {} -> {}  ({:?})", u8::from(on), hex(&bytes), asked.elapsed());
                for f in parse_all_responses(&bytes) {
                    println!("      [{}.{}] {:?} {}", f.fblock, f.func, f.op, hex(&f.payload));
                }
            }
            Err(e) => println!("  SETGET [1.10] {} failed: {e}", u8::from(on)),
        }
    };

    // Away from where it began.
    set(&transport, !was_on);
    let mut lost = false;
    for wait in [0u64, 200, 500, 1000, 2000, 3000] {
        std::thread::sleep(Duration::from_millis(wait));
        match state(&transport) {
            Ok(now) => println!("    +{wait:>4} ms  [1.10] = {}   devices: {}", hex(&now), devices(&transport)),
            Err(e) => {
                println!("    +{wait:>4} ms  could not read: {e}");
                lost = true;
                break;
            }
        }
    }

    if lost {
        println!("  the link was lost; trying to reopen it to put multipoint back");
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut reopened = None;
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_secs(3));
            if let Ok(t) = open() {
                reopened = Some(t);
                break;
            }
        }
        transport = reopened.expect(
            "MULTIPOINT WAS LEFT CHANGED: the link could not be reopened. Reconnect the \
             headphones to this computer and run this test again to restore it",
        );
        println!("  reopened");
    }

    // And back.
    set(&transport, was_on);
    for wait in [0u64, 200, 500, 1000, 2000, 3000, 5000] {
        std::thread::sleep(Duration::from_millis(wait));
        match state(&transport) {
            Ok(now) => println!("    +{wait:>4} ms  [1.10] = {}   devices: {}", hex(&now), devices(&transport)),
            Err(e) => println!("    +{wait:>4} ms  could not read: {e}"),
        }
    }

    let ended = state(&transport).expect("multipoint could be read at the end");
    println!("  after: [1.10] = {}", hex(&ended));
    transport.take_wire_log();
    assert_eq!(
        ended.first().map(|b| b & 0x02 != 0),
        Some(was_on),
        "MULTIPOINT WAS LEFT CHANGED from how it was found"
    );
}

/// Switches multipoint on and says what the headphones then report. For
/// putting it back after `hw_diag_multipoint_toggle` has lost the link with
/// it off. **Writes**: one `SETGET [1.10]` with 1.
#[test]
#[ignore = "requires the physical headphones; switches multipoint on"]
fn hw_set_multipoint_on() {
    use super::endpoint::{Connector, Route};
    use super::stream::{hex, StreamTransport, Timing};
    use bmap::protocol::parse_all_responses;
    use bmap::transport::Transport;

    let hp = find_headphones();
    let opened = WinsockConnector::new(hp.address)
        .connect(Route::SerialPortService)
        .unwrap_or_else(|f| panic!("could not open: {}", f.message));
    let transport = StreamTransport::new(opened.stream, Timing::standard());
    transport.drain_greeting();

    let read = || {
        transport
            .send_recv(&[0x01, 0x0A, 0x01, 0x00])
            .map(|bytes| {
                parse_all_responses(&bytes)
                    .into_iter()
                    .find(|f| (f.fblock, f.func) == (1, 10))
                    .map(|f| f.payload)
                    .unwrap_or_default()
            })
            .map_err(|e| e.to_string())
    };
    println!("\n  before: [1.10] = {:?}", read().map(|p| hex(&p)));
    let reply = transport.send_recv(&[0x01, 0x0A, 0x02, 0x01, 0x01]);
    println!("  SETGET [1.10] 1 -> {:?}", reply.as_ref().map(|b| hex(b)).map_err(|e| e.to_string()));
    for wait in [0u64, 300, 1000] {
        std::thread::sleep(std::time::Duration::from_millis(wait));
        println!("    +{wait:>4} ms  [1.10] = {:?}", read().map(|p| hex(&p)));
    }
    let now = read().expect("multipoint could be read back");
    transport.take_wire_log();
    assert_eq!(now.first().map(|b| b & 0x01), Some(1), "multipoint is on");
}

/// Finds out what renaming the headphones looks like on the wire. **This one
/// writes**: `SETGET [1.2]` with a test name, then with the name they had, so
/// they end called what they were called. Run only with the owner's
/// agreement.
#[test]
#[ignore = "requires the physical headphones; renames them and renames them back"]
fn hw_diag_rename_and_back() {
    use super::endpoint::{Connector, Route};
    use super::stream::{hex, StreamTransport, Timing};
    use bmap::protocol::{parse_all_responses, Operator};
    use bmap::transport::Transport;
    use std::time::{Duration, Instant};

    let hp = find_headphones();
    let open = || {
        WinsockConnector::new(hp.address)
            .connect(Route::SerialPortService)
            .map(|opened| {
                let t = StreamTransport::new(opened.stream, Timing::standard());
                t.drain_greeting();
                t
            })
    };
    let mut transport = open().unwrap_or_else(|f| panic!("could not open: {}", f.message));

    let read = |t: &StreamTransport<_>| -> Result<Vec<u8>, String> {
        let bytes = t.send_recv(&[0x01, 0x02, 0x01, 0x00]).map_err(|e| e.to_string())?;
        parse_all_responses(&bytes)
            .into_iter()
            .find(|f| (f.fblock, f.func) == (1, 2))
            .map(|f| if f.op == Operator::Status { Ok(f.payload) } else { Err(f.fmt()) })
            .unwrap_or_else(|| Err(format!("no [1.2] frame in {}", hex(&bytes))))
    };
    let show = |payload: &[u8]| {
        format!(
            "flag {:02X}, {:?}",
            payload.first().copied().unwrap_or(0xFF),
            String::from_utf8_lossy(payload.get(1..).unwrap_or(&[]))
        )
    };
    let set = |t: &StreamTransport<_>, name: &str| -> bool {
        let mut packet = vec![0x01, 0x02, 0x02, name.len() as u8];
        packet.extend_from_slice(name.as_bytes());
        let asked = Instant::now();
        match t.send_recv(&packet) {
            Ok(bytes) => {
                println!("  SETGET [1.2] {name:?} -> {}  ({:?})", hex(&bytes), asked.elapsed());
                for f in parse_all_responses(&bytes) {
                    println!("      [{}.{}] {:?} {}", f.fblock, f.func, f.op, hex(&f.payload));
                }
                true
            }
            Err(e) => {
                println!("  SETGET [1.2] {name:?} failed: {e}");
                false
            }
        }
    };

    let began = read(&transport).expect("the name could be read before anything was sent");
    let original = String::from_utf8(began.get(1..).unwrap_or(&[]).to_vec()).expect("a UTF-8 name");
    println!("\n  before: {}   (Windows calls them {:?})", show(&began), hp.name);
    assert!(!original.is_empty(), "the headphones reported a name");

    let trial = "Aurora Test";
    set(&transport, trial);
    let mut lost = false;
    for wait in [0u64, 200, 800, 2000] {
        std::thread::sleep(Duration::from_millis(wait));
        match read(&transport) {
            Ok(now) => println!("    +{wait:>4} ms  {}", show(&now)),
            Err(e) => {
                println!("    +{wait:>4} ms  could not read: {e}");
                lost = true;
                break;
            }
        }
    }
    if lost {
        println!("  the link was lost; trying to reopen it to put the name back");
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut reopened = None;
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_secs(3));
            if let Ok(t) = open() {
                reopened = Some(t);
                break;
            }
        }
        transport = reopened.unwrap_or_else(|| {
            panic!("THE NAME WAS LEFT CHANGED: the link could not be reopened. The headphones were called {original:?}")
        });
    }

    set(&transport, &original);
    for wait in [0u64, 200, 800] {
        std::thread::sleep(Duration::from_millis(wait));
        match read(&transport) {
            Ok(now) => println!("    +{wait:>4} ms  {}", show(&now)),
            Err(e) => println!("    +{wait:>4} ms  could not read: {e}"),
        }
    }

    let ended = read(&transport).expect("the name could be read at the end");
    println!("  after: {}", show(&ended));
    transport.take_wire_log();
    assert_eq!(ended, began, "THE NAME WAS LEFT CHANGED from {original:?}");
}

/// Finds out what asking the headphones to drop one of the devices they are
/// connected to, and to connect it again, looks like on the wire. **This one
/// writes**: `START [4.2]` with the address of a connected device that is not
/// this computer, and then `START [4.1]` with the same address, so that the
/// device ends connected as it began. Run only with the owner's agreement:
/// for the seconds in between, that device — their phone — is disconnected
/// from the headphones.
///
/// The frames are an older model's (QC35, `based-connect`): disconnect is the
/// six address bytes, connect is a zero and then the six. Whether this model
/// takes them is what is being found out. Addresses are printed masked.
#[test]
#[ignore = "requires the physical headphones; disconnects another device and reconnects it"]
fn hw_diag_paired_disconnect_and_reconnect() {
    use super::endpoint::{Connector, Route};
    use super::stream::{hex, StreamTransport, Timing};
    use bmap::protocol::{parse_all_responses, Operator};
    use bmap::transport::Transport;
    use std::time::{Duration, Instant};

    let hp = find_headphones();
    let opened = WinsockConnector::new(hp.address)
        .connect(Route::SerialPortService)
        .unwrap_or_else(|f| panic!("could not open: {}", f.message));
    let transport = StreamTransport::new(opened.stream, Timing::standard());
    transport.drain_greeting();

    // The list's first byte and its addresses.
    let list = |t: &StreamTransport<_>| -> (Option<u8>, Vec<[u8; 6]>) {
        let Ok(bytes) = t.send_recv(&[0x04, 0x04, 0x01, 0x00]) else {
            return (None, Vec::new());
        };
        parse_all_responses(&bytes)
            .into_iter()
            .find(|f| (f.fblock, f.func, f.op) == (4, 4, Operator::Status))
            .map(|f| {
                let macs = f
                    .payload
                    .get(1..)
                    .unwrap_or(&[])
                    .chunks_exact(6)
                    .map(|m| m.try_into().expect("six bytes"))
                    .collect();
                (f.payload.first().copied(), macs)
            })
            .unwrap_or((None, Vec::new()))
    };
    // One device's three bytes after its address, and its name.
    let info = |t: &StreamTransport<_>, mac: &[u8; 6]| -> Result<(Vec<u8>, String), String> {
        let mut packet = vec![0x04, 0x05, 0x01, 0x06];
        packet.extend_from_slice(mac);
        let bytes = t.send_recv(&packet).map_err(|e| e.to_string())?;
        parse_all_responses(&bytes)
            .into_iter()
            .find(|f| (f.fblock, f.func) == (4, 5))
            .map(|f| {
                if f.op == Operator::Status && f.payload.len() >= 9 {
                    Ok((
                        f.payload[6..9].to_vec(),
                        String::from_utf8_lossy(&f.payload[9..]).into_owned(),
                    ))
                } else {
                    Err(f.fmt().chars().take(40).collect())
                }
            })
            .unwrap_or_else(|| Err("no [4.5] frame".into()))
    };
    // Every frame that comes back for a request, without its payload where
    // the payload may hold an address.
    let show = |bytes: &[u8]| {
        for f in parse_all_responses(bytes) {
            let safe = f.payload.len() < 6;
            println!(
                "      [{}.{}] {:?}  {} payload byte(s){}",
                f.fblock,
                f.func,
                f.op,
                f.payload.len(),
                if safe { format!(": {}", hex(&f.payload)) } else { String::new() }
            );
        }
    };

    let (first, macs) = list(&transport);
    println!("\n  list: first byte {first:02X?}, {} device(s)", macs.len());
    let mut target = None;
    for mac in &macs {
        match info(&transport, mac) {
            Ok((status, name)) => {
                println!("    {name:?}: {}", hex(&status));
                if status[0] == 0x01 && target.is_none() {
                    target = Some((*mac, name));
                }
            }
            Err(e) => println!("    (could not be read: {e})"),
        }
    }
    let Some((mac, name)) = target else {
        println!("  nothing is connected besides this computer; nothing to try");
        return;
    };

    println!("\n  asking the headphones to disconnect {name:?}");
    let mut packet = vec![0x04, 0x02, 0x05, 0x06];
    packet.extend_from_slice(&mac);
    let asked = Instant::now();
    match transport.send_recv_drain(&packet) {
        Ok(bytes) => {
            println!("    answered after {:?}:", asked.elapsed());
            show(&bytes);
        }
        Err(e) => println!("    START [4.2] failed: {e}"),
    }
    let mut dropped = false;
    for _ in 0..12 {
        std::thread::sleep(Duration::from_millis(500));
        match info(&transport, &mac) {
            Ok((status, _)) => {
                println!("    +{:>5} ms  {}", asked.elapsed().as_millis(), hex(&status));
                if status[0] & 0x01 == 0 {
                    dropped = true;
                    break;
                }
            }
            Err(e) => println!("    +{:>5} ms  could not read: {e}", asked.elapsed().as_millis()),
        }
    }
    let (first, macs_now) = list(&transport);
    println!(
        "    list now: first byte {first:02X?}, {} device(s); the device is {} it",
        macs_now.len(),
        if macs_now.contains(&mac) { "still on" } else { "no longer on" }
    );
    println!("    disconnected: {dropped}");

    println!("\n  asking the headphones to connect {name:?} again");
    let mut packet = vec![0x04, 0x01, 0x05, 0x07, 0x00];
    packet.extend_from_slice(&mac);
    let asked = Instant::now();
    match transport.send_recv_drain(&packet) {
        Ok(bytes) => {
            println!("    answered after {:?}:", asked.elapsed());
            show(&bytes);
        }
        Err(e) => println!("    START [4.1] failed: {e}"),
    }
    let mut back = false;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(500));
        match info(&transport, &mac) {
            Ok((status, _)) => {
                if status[0] & 0x01 != 0 {
                    println!("    +{:>5} ms  {}", asked.elapsed().as_millis(), hex(&status));
                    back = true;
                    break;
                }
            }
            Err(e) => println!("    +{:>5} ms  could not read: {e}", asked.elapsed().as_millis()),
        }
    }
    let (first, macs_end) = list(&transport);
    println!("    list at the end: first byte {first:02X?}, {} device(s)", macs_end.len());
    transport.take_wire_log();
    assert!(
        back || !dropped,
        "{name:?} WAS LEFT DISCONNECTED from the headphones: reconnect it from its own Bluetooth settings"
    );
    println!("    connected again: {back}");
}
