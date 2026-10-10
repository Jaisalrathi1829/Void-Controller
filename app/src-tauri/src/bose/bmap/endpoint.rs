//! Finding the BMAP endpoint, and proving it is one.
//!
//! Two lessons are built into this module.
//!
//! **A socket that connects is not a BMAP endpoint.** Upstream documents
//! channels that accept an RFCOMM connection and then never speak BMAP, and
//! this project spent a full session connected to exactly such a service.
//! So no route is ever handed back unvalidated: each candidate must answer a
//! read-only firmware GET from the right address with a STATUS frame.
//!
//! **The channel number is not a constant.** Upstream's notes record the same
//! model answering on channel 8 on one unit and 9 on another. The preferred
//! route is therefore the one resolved from the device's own SDP record; fixed
//! channels are a bounded fallback drawn only from the channels upstream has
//! documented, and are validated the same way.
//!
//! # The two service UUIDs do different jobs
//!
//! * [`BMAP_SERVICE_UUID`] **identifies** a BMAP-capable device. It is not a
//!   route. Measured on the test headphones (QuietComfort Headphones,
//!   firmware 1.0.6): the first connection to that service was accepted and
//!   answered a firmware GET with 14 bytes that do not frame as BMAP; every
//!   connection after that timed out. Upstream uses this UUID the same way —
//!   as a discovery filter, never as a connection target.
//! * [`SPP_SERVICE_UUID`] is what **resolves the channel**. Its SDP record
//!   points at the RFCOMM channel that actually speaks BMAP, as upstream's
//!   notes for this model say.
//!
//! Nothing here can change device state. The only frames sent are GETs.

use super::stream::{ByteStream, StreamTransport, Timing, WireEvent};
use bmap::device::{parse_firmware, Addr, DeviceConfig};
use bmap::protocol::{bmap_packet, parse_all_responses, Operator};
use bmap::Transport;
use std::time::Duration;

/// BMAP service class UUID, as advertised in the device's SDP record. Used to
/// recognise a BMAP-capable device; see the module docs for why it is not
/// connected to. Same value as upstream's `catalog::BMAP_UUID`.
pub const BMAP_SERVICE_UUID: u128 = 0x00000000_deca_fade_deca_deafdecacaff;

/// Standard Serial Port Profile UUID. Upstream's notes for this model record
/// that connecting through it reaches the BMAP channel.
pub const SPP_SERVICE_UUID: u128 = 0x00001101_0000_1000_8000_00805F9B34FB;

/// One way of reaching the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// The channel the device's SDP record gives for the Serial Port service.
    SerialPortService,
    /// A fixed RFCOMM server channel from the documented candidates.
    Channel(u8),
}

impl Route {
    pub fn describe(self) -> String {
        match self {
            Self::SerialPortService => "Serial Port service (SDP)".to_string(),
            Self::Channel(c) => format!("RFCOMM channel {c} (documented fallback)"),
        }
    }
}

/// A connected stream and, when the platform can report it, the RFCOMM
/// channel it landed on.
pub struct Opened<S> {
    pub stream: S,
    pub channel: Option<u8>,
}

/// Why a route could not be connected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectFailure {
    pub message: String,
    /// The device itself could not be reached (off, out of range). Trying
    /// further routes would only repeat a long timeout, so the search stops.
    pub unreachable: bool,
}

/// Opens streams. Implemented over Winsock for real hardware and by a script
/// in tests.
pub trait Connector {
    type Stream: ByteStream;
    fn connect(&mut self, route: Route) -> Result<Opened<Self::Stream>, ConnectFailure>;
}

/// What happened when a route was tried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptResult {
    Validated { firmware: String },
    ConnectFailed(String),
    /// Connected, but did not answer like a BMAP endpoint.
    NotBmap(String),
    /// Not tried: an earlier route already reached this channel and it failed
    /// validation.
    SkippedSameChannel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub route: Route,
    pub channel: Option<u8>,
    pub result: AttemptResult,
    /// What was exchanged with a route that connected but failed validation.
    /// Kept because "it connected and said *something* that was not BMAP" is
    /// exactly the situation that needs diagnosing rather than discarding.
    pub wire: Vec<WireEvent>,
}

impl Attempt {
    pub fn render(&self) -> String {
        let ch = self
            .channel
            .map(|c| format!(" -> channel {c}"))
            .unwrap_or_default();
        let outcome = match &self.result {
            AttemptResult::Validated { firmware } => {
                format!("validated (firmware {firmware})")
            }
            AttemptResult::ConnectFailed(why) => format!("connect failed: {why}"),
            AttemptResult::NotBmap(why) => format!("connected but not BMAP: {why}"),
            AttemptResult::SkippedSameChannel => {
                "skipped (this channel already failed validation)".to_string()
            }
        };
        format!("{}{ch}: {outcome}", self.route.describe())
    }
}

/// A route that answered the validation check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub route: Route,
    pub channel: Option<u8>,
    /// Firmware string returned by the validation GET.
    pub firmware: String,
}

impl Endpoint {
    pub fn describe(&self) -> String {
        match (self.route, self.channel) {
            (Route::SerialPortService, Some(c)) => {
                format!("RFCOMM channel {c}, resolved by Windows for the Serial Port service")
            }
            // Windows resolves the channel but does not say which it chose.
            (Route::SerialPortService, None) => {
                "the RFCOMM channel Windows resolves for the Serial Port service".to_string()
            }
            (Route::Channel(c), _) => format!("RFCOMM channel {c}, a documented fallback"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenError {
    pub attempts: Vec<Attempt>,
    pub unreachable: bool,
}

impl OpenError {
    pub fn summary(&self) -> String {
        if self.unreachable {
            return format!(
                "The headphones could not be reached ({}).",
                self.attempts
                    .last()
                    .map(|a| a.render())
                    .unwrap_or_else(|| "no route tried".into())
            );
        }
        let tried = self
            .attempts
            .iter()
            .map(|a| a.render())
            .collect::<Vec<_>>()
            .join("; ");
        format!("No BMAP endpoint could be validated. Tried: {tried}.")
    }
}

/// Pause after closing a socket before opening the next, so the device can
/// finish tearing the previous RFCOMM link down.
pub const INTER_ATTEMPT_PAUSE: Duration = Duration::from_millis(350);

/// Routes to try, in order.
///
/// The SDP-resolved route first. Then the model's configured channel, then
/// upstream's `FALLBACK_CHANNELS` — the complete list of channels BMAP has
/// been documented on. The list is fixed and short; this is not a scan.
pub fn candidate_routes(config: &DeviceConfig) -> Vec<Route> {
    let mut routes = vec![
        Route::SerialPortService,
        Route::Channel(config.rfcomm_channel),
    ];
    for &channel in bmap::FALLBACK_CHANNELS.iter() {
        if !routes.contains(&Route::Channel(channel)) {
            routes.push(Route::Channel(channel));
        }
    }
    routes
}

/// Asks the endpoint for its firmware version and checks the answer has the
/// shape a BMAP device gives.
///
/// This is upstream's `speaks_bmap` check, tightened: the reply must come from
/// the firmware function, must be a STATUS frame, and must carry a printable
/// version string. A leading unsolicited frame is tolerated rather than
/// mistaken for the reply.
pub fn validate_endpoint<T: Transport>(transport: &T, config: &DeviceConfig) -> Result<String, String> {
    // Some older models stay silent until asked for the BMAP version first.
    if let Some(init) = config.init_packet {
        let pkt = bmap_packet(init.0, init.1, Operator::Get, &[]);
        transport
            .send_recv(&pkt)
            .map_err(|e| format!("init GET [{}.{}] failed: {e}", init.0, init.1))?;
    }

    let Addr(fblock, func) = config.firmware.unwrap_or(Addr(0, 5));
    let pkt = bmap_packet(fblock, func, Operator::Get, &[]);
    let data = transport.send_recv(&pkt).map_err(|e| e.to_string())?;

    let frames = parse_all_responses(&data);
    let reply = frames
        .iter()
        .find(|f| f.fblock == fblock && f.func == func)
        .ok_or_else(|| format!("no reply from the firmware function [{fblock}.{func}]"))?;

    match reply.op {
        Operator::Status => {}
        Operator::Error => return Err(format!("firmware GET was refused: {}", reply.fmt())),
        other => {
            return Err(format!(
                "firmware GET was answered with {} where STATUS was expected",
                other.name()
            ))
        }
    }

    let firmware = parse_firmware(&reply.payload);
    if firmware.is_empty() || !firmware.chars().all(|c| c.is_ascii_graphic()) {
        return Err("firmware reply is not a printable version string".to_string());
    }
    Ok(firmware)
}

/// Reads the current mode and insists on the documented framing.
///
/// Expected, per upstream's notes for this model: `1F 03 03 01 <modeIndex>` —
/// a STATUS frame from the current-mode function with exactly one payload
/// byte. Anything else means this firmware does not match the documented
/// layout, and the caller must not go on to write.
pub fn probe_current_mode<T: Transport>(transport: &T, config: &DeviceConfig) -> Result<u8, String> {
    let Addr(fblock, func) = config
        .current_mode
        .ok_or_else(|| "this model has no current-mode function".to_string())?;

    let pkt = bmap_packet(fblock, func, Operator::Get, &[]);
    let data = transport.send_recv(&pkt).map_err(|e| e.to_string())?;
    let frames = parse_all_responses(&data);

    let from_function: Vec<_> = frames
        .iter()
        .filter(|f| f.fblock == fblock && f.func == func)
        .collect();

    if let Some(err) = from_function.iter().find(|f| f.op == Operator::Error) {
        return Err(format!("current-mode GET was refused: {}", err.fmt()));
    }

    let status = from_function
        .iter()
        .find(|f| f.op == Operator::Status)
        .ok_or_else(|| {
            format!("no STATUS reply from the current-mode function [{fblock}.{func}]")
        })?;

    if status.payload.len() != 1 {
        return Err(format!(
            "current-mode STATUS carried {} payload byte(s) where exactly 1 is documented ({})",
            status.payload.len(),
            status.fmt()
        ));
    }
    Ok(status.payload[0])
}

/// A validated connection: the transport, the endpoint it reached, and every
/// route tried on the way there.
pub type Validated<S> = (StreamTransport<S>, Endpoint, Vec<Attempt>);

/// Tries each candidate route and returns the first that validates.
pub fn open_validated<C: Connector>(
    connector: &mut C,
    config: &DeviceConfig,
    timing: Timing,
    mut pause: impl FnMut(Duration),
) -> Result<Validated<C::Stream>, OpenError> {
    let mut attempts: Vec<Attempt> = Vec::new();
    let mut failed_channels: Vec<u8> = Vec::new();
    let mut need_pause = false;

    for route in candidate_routes(config) {
        if let Route::Channel(c) = route {
            if failed_channels.contains(&c) {
                attempts.push(Attempt {
                    route,
                    channel: Some(c),
                    result: AttemptResult::SkippedSameChannel,
                    wire: Vec::new(),
                });
                continue;
            }
        }

        if need_pause {
            pause(INTER_ATTEMPT_PAUSE);
            need_pause = false;
        }

        let opened = match connector.connect(route) {
            Ok(o) => o,
            Err(failure) => {
                attempts.push(Attempt {
                    route,
                    channel: None,
                    result: AttemptResult::ConnectFailed(failure.message),
                    wire: Vec::new(),
                });
                if failure.unreachable {
                    return Err(OpenError {
                        attempts,
                        unreachable: true,
                    });
                }
                continue;
            }
        };

        let channel = opened.channel.or(match route {
            Route::Channel(c) => Some(c),
            Route::SerialPortService => None,
        });

        let transport = StreamTransport::new(opened.stream, timing);
        transport.drain_greeting();

        match validate_endpoint(&transport, config) {
            Ok(firmware) => {
                attempts.push(Attempt {
                    route,
                    channel,
                    result: AttemptResult::Validated {
                        firmware: firmware.clone(),
                    },
                    // The live transport keeps its own log.
                    wire: Vec::new(),
                });
                return Ok((
                    transport,
                    Endpoint {
                        route,
                        channel,
                        firmware,
                    },
                    attempts,
                ));
            }
            Err(why) => {
                attempts.push(Attempt {
                    route,
                    channel,
                    result: AttemptResult::NotBmap(why),
                    wire: transport.take_wire_log(),
                });
                if let Some(c) = channel {
                    failed_channels.push(c);
                }
                // `transport` drops here, closing the socket.
                need_pause = true;
            }
        }
    }

    Err(OpenError {
        attempts,
        unreachable: false,
    })
}

#[cfg(test)]
mod tests {
    use super::super::stream::testing::{data, Chunk, ScriptedStream};
    use super::*;
    use bmap::devices;
    use std::collections::HashMap;

    const FW_GET: [u8; 4] = [0x00, 0x05, 0x01, 0x00];

    fn fw_status(version: &str) -> Vec<u8> {
        let mut f = vec![0x00, 0x05, 0x03, version.len() as u8];
        f.extend_from_slice(version.as_bytes());
        f
    }

    /// A stream that answers the firmware GET like a real BMAP peer.
    fn bmap_stream() -> ScriptedStream {
        ScriptedStream::new().on_send(vec![data(&fw_status("1.0.6-80+f5f219b"))])
    }

    /// A stream that accepts the connection and never says anything.
    fn silent_stream() -> ScriptedStream {
        ScriptedStream::new()
    }

    enum Scripted {
        Up(ScriptedStream, Option<u8>),
        Refused,
        Unreachable,
    }

    struct FakeConnector {
        routes: HashMap<String, Scripted>,
        tried: Vec<Route>,
    }

    impl FakeConnector {
        fn new(routes: Vec<(Route, Scripted)>) -> Self {
            Self {
                routes: routes
                    .into_iter()
                    .map(|(r, s)| (format!("{r:?}"), s))
                    .collect(),
                tried: Vec::new(),
            }
        }
    }

    impl Connector for FakeConnector {
        type Stream = ScriptedStream;
        fn connect(&mut self, route: Route) -> Result<Opened<ScriptedStream>, ConnectFailure> {
            self.tried.push(route);
            match self.routes.get(&format!("{route:?}")) {
                Some(Scripted::Up(stream, channel)) => Ok(Opened {
                    stream: stream.clone(),
                    channel: *channel,
                }),
                Some(Scripted::Unreachable) => Err(ConnectFailure {
                    message: "timed out".into(),
                    unreachable: true,
                }),
                Some(Scripted::Refused) | None => Err(ConnectFailure {
                    message: "refused".into(),
                    unreachable: false,
                }),
            }
        }
    }

    fn open(
        connector: &mut FakeConnector,
    ) -> Result<(StreamTransport<ScriptedStream>, Endpoint, Vec<Attempt>), OpenError> {
        open_validated(connector, &devices::qc_prince(), Timing::instant(), |_| {})
    }

    const SPP: Route = Route::SerialPortService;

    // --- candidates ---------------------------------------------------------

    #[test]
    fn candidates_are_sdp_first_then_only_documented_channels() {
        let routes = candidate_routes(&devices::qc_prince());
        assert_eq!(
            routes,
            vec![SPP, Route::Channel(8), Route::Channel(2), Route::Channel(9)]
        );
    }

    /// The BMAP UUID identifies a device; it is not somewhere to connect.
    /// On the test hardware that service answered once with non-BMAP bytes
    /// and then stopped accepting connections altogether.
    #[test]
    fn the_bmap_uuid_is_never_a_connection_route() {
        for config in [devices::qc_prince(), devices::qc_ultra2(), devices::qc35()] {
            for route in candidate_routes(&config) {
                assert!(
                    matches!(route, Route::SerialPortService | Route::Channel(_)),
                    "unexpected route {route:?}"
                );
            }
        }
    }

    /// The fallback list is bounded by upstream's documented channels. If
    /// someone widens it into a scan, this is where they should have to stop.
    #[test]
    fn fallback_channels_are_bounded_and_never_a_scan() {
        for config in [devices::qc_prince(), devices::qc_ultra2(), devices::qc35()] {
            let fixed: Vec<u8> = candidate_routes(&config)
                .into_iter()
                .filter_map(|r| match r {
                    Route::Channel(c) => Some(c),
                    _ => None,
                })
                .collect();
            assert!(fixed.len() <= 4, "too many fixed channels: {fixed:?}");
            for c in &fixed {
                assert!(
                    *c == config.rfcomm_channel || bmap::FALLBACK_CHANNELS.contains(c),
                    "channel {c} is not a documented candidate"
                );
            }
        }
    }

    #[test]
    fn service_uuids_match_the_documented_values() {
        assert_eq!(
            format!("{:032x}", BMAP_SERVICE_UUID),
            bmap::catalog::BMAP_UUID.replace('-', "")
        );
        assert_eq!(
            format!("{:032x}", SPP_SERVICE_UUID),
            "0000110100001000800000805f9b34fb"
        );
    }

    // --- selection ----------------------------------------------------------

    #[test]
    fn sdp_resolved_route_is_preferred_and_validated() {
        let mut c = FakeConnector::new(vec![(SPP, Scripted::Up(bmap_stream(), Some(8)))]);
        let (_, endpoint, attempts) = open(&mut c).unwrap();
        assert_eq!(endpoint.route, SPP);
        assert_eq!(endpoint.channel, Some(8));
        assert_eq!(endpoint.firmware, "1.0.6-80+f5f219b");
        assert_eq!(c.tried, vec![SPP], "no further routes once one validates");
        assert_eq!(attempts.len(), 1);
        assert!(endpoint.describe().contains("channel 8"));
    }

    /// The central guarantee: a socket that connects but does not speak BMAP
    /// is never returned.
    #[test]
    fn a_connecting_but_silent_service_is_not_accepted() {
        let mut c = FakeConnector::new(vec![
            (SPP, Scripted::Up(silent_stream(), Some(14))),
            (Route::Channel(8), Scripted::Up(bmap_stream(), None)),
        ]);
        let (_, endpoint, attempts) = open(&mut c).unwrap();
        assert_eq!(endpoint.route, Route::Channel(8));
        assert!(matches!(attempts[0].result, AttemptResult::NotBmap(_)));
    }

    #[test]
    fn falls_back_to_documented_channels_when_sdp_finds_nothing() {
        let mut c = FakeConnector::new(vec![
            (SPP, Scripted::Refused),
            (Route::Channel(8), Scripted::Refused),
            (Route::Channel(2), Scripted::Refused),
            (Route::Channel(9), Scripted::Up(bmap_stream(), None)),
        ]);
        let (_, endpoint, _) = open(&mut c).unwrap();
        // Channel 9 is where upstream's second unit answered.
        assert_eq!(endpoint.route, Route::Channel(9));
        assert_eq!(endpoint.channel, Some(9));
    }

    /// If SDP resolved to channel 8 and it failed validation, connecting to
    /// channel 8 again by number would only repeat the failure.
    #[test]
    fn a_channel_that_failed_validation_is_not_retried_by_number() {
        let mut c = FakeConnector::new(vec![
            (SPP, Scripted::Up(silent_stream(), Some(8))),
            (Route::Channel(8), Scripted::Up(bmap_stream(), None)),
            (Route::Channel(9), Scripted::Up(bmap_stream(), None)),
        ]);
        let (_, endpoint, attempts) = open(&mut c).unwrap();
        assert_eq!(endpoint.route, Route::Channel(9));
        assert!(!c.tried.contains(&Route::Channel(8)));
        assert!(attempts
            .iter()
            .any(|a| a.result == AttemptResult::SkippedSameChannel));
    }

    #[test]
    fn nothing_validating_is_an_error_listing_every_attempt() {
        let mut c = FakeConnector::new(vec![(SPP, Scripted::Up(silent_stream(), Some(8)))]);
        let err = open(&mut c).unwrap_err();
        assert!(!err.unreachable);
        assert_eq!(err.attempts.len(), 4);
        assert!(err.summary().contains("No BMAP endpoint could be validated"));
    }

    /// An unreachable device ends the search at once instead of sitting
    /// through a connect timeout per route.
    #[test]
    fn unreachable_device_stops_the_search_immediately() {
        let mut c = FakeConnector::new(vec![(SPP, Scripted::Unreachable)]);
        let err = open(&mut c).unwrap_err();
        assert!(err.unreachable);
        assert_eq!(c.tried, vec![SPP]);
    }

    #[test]
    fn pauses_only_after_a_socket_was_opened_and_closed() {
        let mut pauses = 0;
        let mut c = FakeConnector::new(vec![
            (SPP, Scripted::Up(silent_stream(), Some(3))),
            (Route::Channel(8), Scripted::Refused),
            (Route::Channel(2), Scripted::Up(bmap_stream(), None)),
        ]);
        open_validated(&mut c, &devices::qc_prince(), Timing::instant(), |_| pauses += 1).unwrap();
        // One pause, after the silent socket was closed. The refused connect
        // never opened a socket, so nothing needed tearing down after it.
        assert_eq!(pauses, 1);
    }

    // --- validation ---------------------------------------------------------

    fn validate(stream: ScriptedStream) -> Result<String, String> {
        let t = StreamTransport::new(stream, Timing::instant());
        validate_endpoint(&t, &devices::qc_prince())
    }

    #[test]
    fn validation_sends_only_the_read_only_firmware_get() {
        let s = bmap_stream();
        validate(s.clone()).unwrap();
        assert_eq!(s.sent(), vec![FW_GET.to_vec()]);
    }

    #[test]
    fn validation_rejects_a_reply_from_the_wrong_function() {
        let s = ScriptedStream::new().on_send(vec![data(&[0x02, 0x02, 0x03, 0x01, 0x50])]);
        assert!(validate(s).is_err());
    }

    #[test]
    fn validation_rejects_an_error_reply() {
        let s = ScriptedStream::new().on_send(vec![data(&[0x00, 0x05, 0x04, 0x01, 0x04])]);
        let why = validate(s).unwrap_err();
        assert!(why.contains("refused"), "{why}");
    }

    #[test]
    fn validation_rejects_a_non_status_operator() {
        let s = ScriptedStream::new().on_send(vec![data(&[0x00, 0x05, 0x06, 0x01, 0x31])]);
        let why = validate(s).unwrap_err();
        assert!(why.contains("STATUS was expected"), "{why}");
    }

    #[test]
    fn validation_rejects_a_non_printable_payload() {
        let s = ScriptedStream::new().on_send(vec![data(&[0x00, 0x05, 0x03, 0x02, 0x00, 0xFF])]);
        assert!(validate(s).is_err());
    }

    #[test]
    fn validation_tolerates_an_unsolicited_frame_before_the_reply() {
        let mut bytes = vec![0x02, 0x02, 0x03, 0x01, 0x50];
        bytes.extend_from_slice(&fw_status("1.0.6"));
        let s = ScriptedStream::new().on_send(vec![data(&bytes)]);
        assert_eq!(validate(s).unwrap(), "1.0.6");
    }

    /// QC35/QC45 need a GET [0.1] before they answer anything.
    #[test]
    fn init_packet_models_are_primed_before_validation() {
        let s = ScriptedStream::new()
            .on_send(vec![data(&[0x00, 0x01, 0x03, 0x05, b'1', b'.', b'1', b'.', b'0'])])
            .on_send(vec![data(&fw_status("4.8.1"))]);
        let t = StreamTransport::new(s.clone(), Timing::instant());
        assert_eq!(validate_endpoint(&t, &devices::qc35()).unwrap(), "4.8.1");
        assert_eq!(s.sent()[0], vec![0x00, 0x01, 0x01, 0x00]);
    }

    // --- strict mode probe --------------------------------------------------

    fn probe(reply: &[u8]) -> Result<u8, String> {
        let s = ScriptedStream::new().on_send(vec![data(reply)]);
        let t = StreamTransport::new(s, Timing::instant());
        probe_current_mode(&t, &devices::qc_prince())
    }

    #[test]
    fn probe_accepts_the_documented_frame() {
        assert_eq!(probe(&[0x1F, 0x03, 0x03, 0x01, 0x01]), Ok(1));
        assert_eq!(probe(&[0x1F, 0x03, 0x03, 0x01, 0x00]), Ok(0));
    }

    #[test]
    fn probe_rejects_an_unexpected_payload_length() {
        let why = probe(&[0x1F, 0x03, 0x03, 0x02, 0x01, 0x00]).unwrap_err();
        assert!(why.contains("exactly 1 is documented"), "{why}");
    }

    #[test]
    fn probe_rejects_an_error_reply() {
        let why = probe(&[0x1F, 0x03, 0x04, 0x01, 0x04]).unwrap_err();
        assert!(why.contains("refused"), "{why}");
    }

    #[test]
    fn probe_rejects_a_non_status_reply() {
        // A bare PROCESSING frame says nothing about the current mode.
        assert!(probe(&[0x1F, 0x03, 0x07, 0x00]).is_err());
    }

    #[test]
    fn probe_reports_silence() {
        let s = ScriptedStream::new().on_send(vec![Chunk::Silence]);
        let t = StreamTransport::new(s, Timing::instant());
        assert!(probe_current_mode(&t, &devices::qc_prince()).is_err());
    }
}
