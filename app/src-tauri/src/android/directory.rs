//! Which paired device is the headphones, as Android tells it.
//!
//! Windows hands over a vendor id and a product id for every paired device.
//! Android hands over neither. So two things are done differently here, and
//! both are stricter than a name match:
//!
//! * A device is a candidate if Android's record of it lists Bose's control
//!   service among its services — the same test the desktop applies — and
//!   only failing that if its name says Bose.
//! * Its model is whatever the device itself answers to a product-id request
//!   over the control protocol (`GET [0.3]`). Measured on the test headphones
//!   on 2026-10-11: the answer is `40 75 0D`, product 0x4075, the id Windows
//!   reports for the same unit.

use super::bridge::{self, PairedDevice};
use super::rfcomm::{parse_address, AndroidConnector};
use crate::bose::bmap::endpoint::{ConnectFailure, Connector, Route, BMAP_SERVICE_UUID};
use crate::bose::bmap::stream::ByteStream;
use crate::bose::bmap::stream::{StreamTransport, Timing};
use bmap::Transport;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Instance ids on Android are this prefix and the device's address. Like
/// their Windows counterparts they stay in the native layer.
const INSTANCE_PREFIX: &str = "ANDROID\\";

pub fn instance_id(address: &str) -> String {
    format!("{INSTANCE_PREFIX}{}", address.to_ascii_uppercase())
}

/// The address inside an Android instance id.
pub fn address_of(instance_id: &str) -> Option<&str> {
    instance_id.strip_prefix(INSTANCE_PREFIX)
}

/// One paired device, in the application's own terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    pub instance_id: String,
    pub address_text: String,
    pub address: Option<u64>,
    pub name: String,
    pub connected: bool,
    pub battery: Option<u8>,
    /// Whether Android's record of the device lists Bose's control service.
    pub advertises_bmap: bool,
    pub low_energy_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub radio_present: bool,
    pub radio_enabled: bool,
    pub permission: bool,
    pub devices: Vec<Paired>,
}

/// Whether a list of service UUIDs includes Bose's control service.
///
/// Android has, on some versions, returned 128-bit UUIDs with their bytes
/// reversed, so both spellings count.
fn lists_bmap(uuids: &[String]) -> bool {
    let forward = BMAP_SERVICE_UUID;
    let reversed = u128::from_be_bytes({
        let mut bytes = forward.to_be_bytes();
        bytes.reverse();
        bytes
    });
    uuids.iter().any(|text| {
        u128::from_str_radix(&text.replace('-', ""), 16)
            .is_ok_and(|value| value == forward || value == reversed)
    })
}

fn paired_from(device: PairedDevice) -> Paired {
    Paired {
        instance_id: instance_id(&device.address),
        address: parse_address(&device.address),
        advertises_bmap: lists_bmap(&device.uuids),
        address_text: device.address,
        name: device.name,
        connected: device.connected,
        battery: device.battery.filter(|level| *level <= 100),
        low_energy_only: device.low_energy_only,
    }
}

/// Everything Android knows about Bluetooth on this phone. Sends nothing.
pub fn report() -> Result<Report, String> {
    let raw = bridge::paired()?;
    Ok(Report {
        radio_present: raw.radio_present,
        radio_enabled: raw.radio_enabled,
        permission: raw.permission,
        devices: raw.devices.into_iter().map(paired_from).collect(),
    })
}

/// Picks the device this application is about: the one with the given
/// instance id, or — when none is given — the first that lists Bose's control
/// service, and failing that the first whose name says Bose.
pub fn find(devices: Vec<Paired>, preferred: Option<&str>) -> Option<Paired> {
    match preferred {
        Some(want) => devices.into_iter().find(|d| d.instance_id == want),
        None => {
            let by_service = devices.iter().position(|d| d.advertises_bmap);
            let by_name = || {
                devices
                    .iter()
                    .position(|d| !d.low_energy_only && crate::bose::looks_like_bose(&d.name))
            };
            let index = by_service.or_else(by_name)?;
            devices.into_iter().nth(index)
        }
    }
}

/// The routes a model is asked for over, before the model is known: the
/// Serial Port service, then the fixed channels the protocol has been
/// documented on. The same short list the control link itself tries; not a
/// scan.
fn probe_routes() -> Vec<Route> {
    let mut routes = vec![Route::SerialPortService];
    for &channel in bmap::FALLBACK_CHANNELS.iter() {
        routes.push(Route::Channel(channel));
    }
    routes
}

/// Asks whatever answers on the probe routes for its product id: one
/// read-only `GET [0.3]` per route that connects, stopping at the first
/// answer that is a product id. A route that connects and says something
/// else is closed and passed over, as it is by the control link.
fn ask_product<C: Connector>(connector: &mut C, timing: Timing, between: Duration) -> Option<u16>
where
    C::Stream: ByteStream,
{
    for route in probe_routes() {
        let opened = match connector.connect(route) {
            Ok(opened) => opened,
            // Out of reach altogether: the other routes would only wait too.
            Err(ConnectFailure { unreachable: true, .. }) => return None,
            Err(_) => continue,
        };
        let found = {
            let transport = StreamTransport::new(opened.stream, timing);
            transport.drain_greeting();
            transport
                .send_recv(&[0x00, 0x03, 0x01, 0x00])
                .ok()
                .and_then(|reply| product_from_reply(&reply))
        };
        // That connection is closed now; let the headphones notice before
        // anything opens another.
        std::thread::sleep(between);
        if found.is_some() {
            return found;
        }
    }
    None
}

/// How long to leave the headphones alone after they failed to say what
/// model they are — because another program holds their control channel,
/// most likely. Asking again at once would only fail again.
const PROBE_RETRY: Duration = Duration::from_secs(15);

/// Time for the headphones to finish closing one connection before the next
/// is opened to the same service.
const AFTER_PROBE: Duration = Duration::from_millis(350);

#[derive(Default)]
struct Probes {
    known: HashMap<u64, u16>,
    failed: HashMap<u64, Instant>,
}

static PROBES: Mutex<Option<Probes>> = Mutex::new(None);

/// Extracts the product id from the reply to `GET [0.3]`: a STATUS frame from
/// that address whose payload starts with the id, most significant byte first.
fn product_from_reply(reply: &[u8]) -> Option<u16> {
    const STATUS: u8 = 3;
    let [0x00, 0x03, flags, len, payload @ ..] = reply else {
        return None;
    };
    if flags & 0x0F != STATUS || usize::from(*len) < 2 || payload.len() < 2 {
        return None;
    }
    Some(u16::from_be_bytes([payload[0], payload[1]]))
}

/// The model of the connected device at `address`, asked of the device.
///
/// Asked once and remembered for the life of the process. One read-only
/// request on a connection of its own, made before the control link exists —
/// the link cannot be built without knowing the model. `None` if the device
/// could not be asked or did not answer as expected.
pub fn product_id(address: u64) -> Option<u16> {
    let mut guard = PROBES.lock();
    let probes = guard.get_or_insert_with(Probes::default);
    if let Some(id) = probes.known.get(&address) {
        return Some(*id);
    }
    if probes
        .failed
        .get(&address)
        .is_some_and(|at| at.elapsed() < PROBE_RETRY)
    {
        return None;
    }

    let asked = ask_product(&mut AndroidConnector::new(address), Timing::standard(), AFTER_PROBE);

    match asked {
        Some(id) => {
            bridge::log(&format!("the headphones identify as product 0x{id:04X}"));
            probes.failed.remove(&address);
            probes.known.insert(address, id);
        }
        None => {
            bridge::log("the headphones did not say which model they are; will ask again later");
            probes.failed.insert(address, Instant::now());
        }
    }
    asked
}

#[cfg(test)]
mod tests {
    use super::*;

    const BMAP: &str = "00000000-deca-fade-deca-deafdecacaff";
    const SPP: &str = "00001101-0000-1000-8000-00805f9b34fb";

    fn device(address: &str, name: &str, uuids: &[&str]) -> Paired {
        paired_from(PairedDevice {
            address: address.to_string(),
            name: name.to_string(),
            connected: true,
            battery: Some(70),
            uuids: uuids.iter().map(|u| u.to_string()).collect(),
            low_energy_only: false,
        })
    }

    #[test]
    fn the_control_service_is_recognised_in_either_byte_order() {
        assert!(lists_bmap(&[SPP.to_string(), BMAP.to_string()]));
        assert!(lists_bmap(&[BMAP.to_uppercase()]));
        // As some versions of Android report it.
        assert!(lists_bmap(&["ffcacade-afde-cade-defa-cade00000000".to_string()]));
        assert!(!lists_bmap(&[SPP.to_string()]));
        assert!(!lists_bmap(&[]));
        assert!(!lists_bmap(&["not a uuid".to_string()]));
    }

    /// The test headphones are renamed, so a name match alone finds nothing.
    #[test]
    fn the_headphones_are_found_by_their_service_whatever_they_are_called() {
        let devices = vec![
            device("11:22:33:44:55:66", "Car stereo", &[SPP]),
            device("E4:58:BC:F9:F0:2E", "Aurora", &[SPP, BMAP]),
        ];
        let found = find(devices, None).unwrap();
        assert_eq!(found.name, "Aurora");
        assert_eq!(found.address, Some(0xE458_BCF9_F02E));
        assert_eq!(found.instance_id, r"ANDROID\E4:58:BC:F9:F0:2E");
    }

    #[test]
    fn a_name_is_the_fallback_and_only_the_fallback() {
        let by_name = vec![
            device("11:22:33:44:55:66", "Car stereo", &[SPP]),
            device("AA:BB:CC:DD:EE:FF", "Bose QC Headphones", &[]),
        ];
        assert_eq!(find(by_name, None).unwrap().name, "Bose QC Headphones");

        // The service wins over an earlier device with a likely name.
        let both = vec![
            device("AA:BB:CC:DD:EE:FF", "Bose speaker", &[]),
            device("E4:58:BC:F9:F0:2E", "Aurora", &[BMAP]),
        ];
        assert_eq!(find(both, None).unwrap().name, "Aurora");

        assert_eq!(find(vec![device("11:22:33:44:55:66", "Car stereo", &[SPP])], None), None);
        assert_eq!(find(Vec::new(), None), None);
    }

    #[test]
    fn a_remembered_device_is_the_only_one_that_will_do() {
        let devices = vec![
            device("E4:58:BC:F9:F0:2E", "Aurora", &[BMAP]),
            device("AA:BB:CC:DD:EE:FF", "Other Bose", &[BMAP]),
        ];
        let want = instance_id("aa:bb:cc:dd:ee:ff");
        assert_eq!(find(devices.clone(), Some(&want)).unwrap().name, "Other Bose");
        assert_eq!(find(devices, Some(r"ANDROID\00:00:00:00:00:00")), None);
    }

    #[test]
    fn an_instance_id_gives_its_address_back() {
        let id = instance_id("e4:58:bc:f9:f0:2e");
        assert_eq!(address_of(&id), Some("E4:58:BC:F9:F0:2E"));
        assert_eq!(
            address_of(r"BTHENUM\DEV_E458BCF9F02E"),
            None,
            "a Windows id is not one of these"
        );
    }

    #[test]
    fn an_impossible_battery_level_is_no_level() {
        let mut raw = PairedDevice {
            address: "E4:58:BC:F9:F0:2E".into(),
            name: "Aurora".into(),
            connected: true,
            battery: Some(255),
            uuids: Vec::new(),
            low_energy_only: false,
        };
        assert_eq!(paired_from(raw.clone()).battery, None);
        raw.battery = Some(100);
        assert_eq!(paired_from(raw).battery, Some(100));
    }

    /// The reply recorded from the test headphones on 2026-10-11.
    #[test]
    fn the_product_id_is_read_from_the_recorded_reply() {
        assert_eq!(
            product_from_reply(&[0x00, 0x03, 0x03, 0x03, 0x40, 0x75, 0x0D]),
            Some(0x4075)
        );
    }

    #[test]
    fn anything_else_is_not_a_product_id() {
        // An error reply from the same address.
        assert_eq!(product_from_reply(&[0x00, 0x03, 0x04, 0x01, 0x05]), None);
        // A reply from a different address.
        assert_eq!(product_from_reply(&[0x00, 0x05, 0x03, 0x03, 0x40, 0x75, 0x0D]), None);
        // Too short to hold an id.
        assert_eq!(product_from_reply(&[0x00, 0x03, 0x03, 0x01, 0x40]), None);
        assert_eq!(product_from_reply(&[0x00, 0x03, 0x03]), None);
        assert_eq!(product_from_reply(&[]), None);
    }
}

#[cfg(test)]
mod probe_tests {
    use super::*;
    use crate::bose::bmap::endpoint::Opened;
    use crate::bose::bmap::stream::testing::{data, ScriptedStream};

    /// A device, described by what each route does when it is tried.
    struct Device {
        routes: Vec<(Route, Result<ScriptedStream, ConnectFailure>)>,
        tried: Vec<Route>,
    }

    impl Device {
        fn new(routes: Vec<(Route, Result<ScriptedStream, ConnectFailure>)>) -> Self {
            Self {
                routes,
                tried: Vec::new(),
            }
        }
    }

    impl Connector for Device {
        type Stream = ScriptedStream;
        fn connect(&mut self, route: Route) -> Result<Opened<ScriptedStream>, ConnectFailure> {
            self.tried.push(route);
            match self.routes.iter().position(|(r, _)| *r == route) {
                Some(i) => self.routes.remove(i).1.map(|stream| Opened {
                    stream,
                    channel: None,
                }),
                None => Err(refused()),
            }
        }
    }

    fn refused() -> ConnectFailure {
        ConnectFailure {
            message: "refused".into(),
            unreachable: false,
        }
    }

    /// A stream that answers the product-id request as the headphones did.
    fn prince() -> ScriptedStream {
        ScriptedStream::new().on_send(vec![data(&[0x00, 0x03, 0x03, 0x03, 0x40, 0x75, 0x0D])])
    }

    fn ask(device: &mut Device) -> Option<u16> {
        ask_product(device, Timing::instant(), Duration::ZERO)
    }

    #[test]
    fn the_serial_port_service_answers_and_nothing_else_is_tried() {
        let stream = prince();
        let mut device = Device::new(vec![(Route::SerialPortService, Ok(stream.clone()))]);
        assert_eq!(ask(&mut device), Some(0x4075));
        assert_eq!(device.tried, vec![Route::SerialPortService]);
        // One request, and a read-only one.
        assert_eq!(stream.sent(), vec![vec![0x00, 0x03, 0x01, 0x00]]);
    }

    #[test]
    fn a_refused_service_falls_back_to_a_documented_channel() {
        let mut device = Device::new(vec![(Route::Channel(8), Ok(prince()))]);
        assert_eq!(ask(&mut device), Some(0x4075));
        assert_eq!(
            device.tried,
            vec![Route::SerialPortService, Route::Channel(2), Route::Channel(8)]
        );
    }

    /// A service can accept a connection and not speak the protocol. It is
    /// asked the one read-only question, and passed over.
    #[test]
    fn a_route_that_connects_but_is_not_the_protocol_is_passed_over() {
        let other = ScriptedStream::new().on_send(vec![data(b"AT+BRSF=0\r")]);
        let silent = ScriptedStream::new();
        let mut device = Device::new(vec![
            (Route::SerialPortService, Ok(other.clone())),
            (Route::Channel(2), Ok(silent)),
            (Route::Channel(8), Ok(prince())),
        ]);
        assert_eq!(ask(&mut device), Some(0x4075));
        assert_eq!(other.sent(), vec![vec![0x00, 0x03, 0x01, 0x00]], "asked once, then left");
    }

    #[test]
    fn headphones_out_of_reach_are_not_tried_route_after_route() {
        let gone = ConnectFailure {
            message: "the device is off".into(),
            unreachable: true,
        };
        let mut device = Device::new(vec![(Route::SerialPortService, Err(gone))]);
        assert_eq!(ask(&mut device), None);
        assert_eq!(device.tried, vec![Route::SerialPortService]);
    }

    #[test]
    fn nothing_answering_is_no_model_and_only_documented_routes_were_tried() {
        let mut device = Device::new(Vec::new());
        assert_eq!(ask(&mut device), None);
        assert_eq!(device.tried, probe_routes());
        assert_eq!(
            probe_routes(),
            vec![
                Route::SerialPortService,
                Route::Channel(2),
                Route::Channel(8),
                Route::Channel(9)
            ],
            "the Serial Port service, then the channels the protocol is documented on"
        );
    }

    /// A device that answers with a different model's id is reported as that
    /// model, and gets that model's description.
    #[test]
    fn another_models_id_is_reported_as_it_is() {
        let ultra = ScriptedStream::new().on_send(vec![data(&[0x00, 0x03, 0x03, 0x03, 0x40, 0x82, 0x01])]);
        let mut device = Device::new(vec![(Route::SerialPortService, Ok(ultra))]);
        let id = ask(&mut device).unwrap();
        assert_eq!(id, 0x4082);
        let config = crate::bose::bmap::config_for_product(id).expect("a known model");
        assert_eq!(config.info.name, "QuietComfort Ultra Headphones (2nd Gen)");
    }
}
