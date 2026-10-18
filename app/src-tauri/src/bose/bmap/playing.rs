//! What the headphones are playing from — which of the devices they are
//! connected to — and whether it is playing; and play, pause and skip,
//! passed on through them to that device. So a computer can show and control
//! what a phone is playing through the headphones, and a phone what a
//! computer is.
//!
//! Function block 5, AudioManagement. Seen first in the Bose app's own
//! traffic (protocol-notes, Experiment 3), then asked by this application
//! (Experiment 17), on the test headphones:
//!
//! ```text
//! GET   [5.1]       ->  00 02 | 01 | six bytes   source: Bluetooth, and its address
//! GET   [5.4]       ->  01 FF FF                 01 playing, 02 paused, 00 stopped;
//!                                                then two bytes not read
//! START [5.3] code  ->  [5.3] PROCESSING code    passed on to the source
//! GET   [5.6]       ->  ERROR 05                 the track: not without signing in
//! ```
//!
//! **The track is not read.** The Bose app is told the title, artist and
//! album with `START [5.6]`; asked by this application, the headphones
//! answer `GET [5.6]` with "authentication required" and `START [5.6]` not at
//! all. Getting past that is a non-goal. What is read is which device is
//! playing and whether it is.
//!
//! `[5.1]`'s layout — two bytes of the sources supported, the active one
//! (0 none, 1 Bluetooth, 2 AUX), then a Bluetooth source's address — and
//! `[5.3]` Control — `START` with one byte, the codes below — are upstream's
//! (bosectl `NOTES.md`, from the Bose app's own `AudioControlValue`). The
//! test headphones take all four codes here without signing in, and pause
//! and play are confirmed by `[5.4]`.
//!
//! The source's address is used here to ask the headphones which paired
//! device it is (`[4.5]`), and goes no further: what this module returns
//! names the device and says whether it is the one asking.

use super::noise::{is_link_failure, VerifyPolicy};
use super::unit::{exchange, read_paired_device, PairedDevice};
use bmap::error::{BmapError, BmapResult};
use bmap::protocol::{bmap_packet, parse_all_responses, Operator};
use bmap::Transport;
use std::time::Duration;

/// `[5.1]`: the source the headphones are playing from.
pub const SOURCE: (u8, u8) = (5, 1);
/// `[5.3]`: play, pause, skip — sent on to the source.
pub const CONTROL: (u8, u8) = (5, 3);
/// `[5.4]`: whether the source is playing.
pub const STATUS: (u8, u8) = (5, 4);
/// `[5.6]`: the track: title, artist, album. Behind authentication on the
/// test headphones; kept for the experiments that showed it.
pub const NOW_PLAYING: (u8, u8) = (5, 6);

fn unexpected(message: String) -> BmapError {
    BmapError::Device { message, code: 0 }
}

fn refusal(frame: &bmap::protocol::BmapResponse) -> BmapError {
    match frame.payload.first() {
        Some(5) => BmapError::Auth(frame.fmt()),
        code => BmapError::Device {
            message: frame.fmt(),
            code: code.copied().unwrap_or(0),
        },
    }
}

/// What the headphones are playing from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Nothing.
    None,
    /// A Bluetooth device, by its address. Kept inside this module.
    Bluetooth([u8; 6]),
    /// The cable.
    Auxiliary,
}

/// A `[5.1]` reply: two bytes of the sources the headphones take, the active
/// source, and for a Bluetooth source its address.
pub fn parse_source(payload: &[u8]) -> BmapResult<Source> {
    match payload.get(2) {
        Some(0) => Ok(Source::None),
        Some(1) => match payload.get(3..9) {
            Some(address) => Ok(Source::Bluetooth(address.try_into().expect("six bytes"))),
            None => Err(unexpected(format!(
                "a Bluetooth source was reported with {} byte(s) where an address takes 6",
                payload.len().saturating_sub(3)
            ))),
        },
        Some(2) => Ok(Source::Auxiliary),
        Some(other) => Err(unexpected(format!(
            "a source of type {other}, which is not described"
        ))),
        None => Err(unexpected(format!(
            "a source reply of {} byte(s) where at least 3 were observed",
            payload.len()
        ))),
    }
}

/// Whether the source is playing, as `[5.4]` says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Playing,
    Paused,
    Stopped,
}

const STOPPED: u8 = 0x00;
const PLAYING: u8 = 0x01;
const PAUSED: u8 = 0x02;

/// A `[5.4]` reply: the state, then two bytes that are not read.
///
/// On the test headphones, `01` while playing and `02` while paused, each
/// time it was paused and played from the phone or through them
/// (protocol-notes, Experiment 17); `00` with a video stopped (Experiment 3).
/// Any other value is not read.
pub fn parse_state(payload: &[u8]) -> BmapResult<PlayState> {
    match payload.first() {
        Some(&PLAYING) => Ok(PlayState::Playing),
        Some(&PAUSED) => Ok(PlayState::Paused),
        Some(&STOPPED) => Ok(PlayState::Stopped),
        Some(other) => Err(unexpected(format!(
            "a play state of {other:#04x}, which is not described"
        ))),
        None => Err(unexpected("an empty play state".into())),
    }
}

/// The track, as the source told the headphones. A field the source did not
/// give is empty. Read only by the experiments: see the module notes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
}

impl Track {
    pub fn is_empty(&self) -> bool {
        self.title.is_empty() && self.artist.is_empty() && self.album.is_empty()
    }
}

/// Field numbers in a `[5.6]` status.
const TITLE: u8 = 0;
const ARTIST: u8 = 1;
const ALBUM: u8 = 2;

/// The longest text kept from one field, in characters.
const LONGEST_FIELD: usize = 200;

/// The track from the frames that answered `START [5.6]`: each `[5.6]
/// STATUS` is a field number and its text. Other frames are passed over; a
/// field that is not one of the three is not read. An `ERROR` from `[5.6]`
/// is the headphones saying no.
pub fn parse_track(reply: &[u8]) -> BmapResult<Track> {
    let mut track = Track::default();
    let mut answered = false;
    for frame in parse_all_responses(reply) {
        if (frame.fblock, frame.func) != NOW_PLAYING {
            continue;
        }
        match frame.op {
            Operator::Status => {
                answered = true;
                let Some((&field, text)) = frame.payload.split_first() else {
                    continue;
                };
                let text: String = String::from_utf8_lossy(text)
                    .trim()
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(LONGEST_FIELD)
                    .collect();
                match field {
                    TITLE => track.title = text,
                    ARTIST => track.artist = text,
                    ALBUM => track.album = text,
                    _ => {}
                }
            }
            Operator::Result => answered = true,
            Operator::Error => return Err(refusal(&frame)),
            _ => {}
        }
    }
    if answered {
        Ok(track)
    } else {
        Err(BmapError::Timeout(format!(
            "no answer from [{}.{}]",
            NOW_PLAYING.0, NOW_PLAYING.1
        )))
    }
}

pub fn read_source<T: Transport>(raw: &T) -> BmapResult<Source> {
    parse_source(&exchange(raw, SOURCE, Operator::Get, &[])?)
}

pub fn read_state<T: Transport>(raw: &T) -> BmapResult<PlayState> {
    parse_state(&exchange(raw, STATUS, Operator::Get, &[])?)
}

/// `START [5.6]`, as the Bose app asks. For the experiments only.
pub fn read_track<T: Transport>(raw: &T) -> BmapResult<Track> {
    let packet = bmap_packet(NOW_PLAYING.0, NOW_PLAYING.1, Operator::Start, &[]);
    parse_track(&raw.send_recv_drain(&packet)?)
}

/// Which paired device a Bluetooth source is: its name, and whether it is
/// the device asking. Asked of the headphones by its address.
pub fn identify<T: Transport>(raw: &T, address: &[u8; 6]) -> BmapResult<PairedDevice> {
    read_paired_device(raw, address)
}

/// What the headphones are playing from, and whether it plays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playing {
    pub from: PlayingFrom,
    pub state: PlayState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayingFrom {
    /// One of the paired devices: what the headphones call it, and whether
    /// it is the device asking.
    Device { name: String, this_device: bool },
    /// A Bluetooth device the headphones did not name.
    Unnamed,
    /// The cable.
    Cable,
}

/// The source last named: its address, and what it was found to be. Kept so
/// `[4.5]` is asked once for each source rather than at every reading.
pub type Named = Option<([u8; 6], PlayingFrom)>;

/// What the headphones are playing from and whether it plays: `[5.1]`, then
/// `[4.5]` for a source not named before, then `[5.4]`. `None` when they
/// have no source.
pub fn read_playing<T: Transport>(raw: &T, named: &mut Named) -> BmapResult<Option<Playing>> {
    let from = match read_source(raw)? {
        Source::None => return Ok(None),
        Source::Auxiliary => PlayingFrom::Cable,
        Source::Bluetooth(address) => match named {
            Some((known, from)) if *known == address => from.clone(),
            _ => {
                let from = match identify(raw, &address) {
                    Ok(device) if !device.name.is_empty() => PlayingFrom::Device {
                        name: device.name,
                        this_device: device.this_device,
                    },
                    Ok(_) => PlayingFrom::Unnamed,
                    Err(e) if is_link_failure(&e) || matches!(e, BmapError::Timeout(_)) => {
                        return Err(e)
                    }
                    Err(_) => PlayingFrom::Unnamed,
                };
                *named = Some((address, from.clone()));
                from
            }
        },
    };
    Ok(Some(Playing {
        from,
        state: read_state(raw)?,
    }))
}

/// What can be asked of the source through the headphones. Upstream's codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Play,
    Pause,
    Next,
    Previous,
}

impl Control {
    pub fn code(self) -> u8 {
        match self {
            Control::Play => 0x01,
            Control::Pause => 0x02,
            Control::Next => 0x03,
            Control::Previous => 0x04,
        }
    }
}

/// Asks the headphones to pass a control on to the source: `START [5.3]`
/// with its code. The test headphones answer `[5.3] PROCESSING` with the
/// code; an `ERROR` is a refusal. Whether the source did it is for the next
/// reading of `[5.4]` to say.
pub fn send_control<T: Transport>(raw: &T, control: Control) -> BmapResult<()> {
    let packet = bmap_packet(CONTROL.0, CONTROL.1, Operator::Start, &[control.code()]);
    let frames = parse_all_responses(&raw.send_recv(&packet)?);
    for frame in frames.iter().filter(|f| (f.fblock, f.func) == CONTROL) {
        match frame.op {
            Operator::Error => return Err(refusal(frame)),
            Operator::Processing | Operator::Result | Operator::Status => return Ok(()),
            _ => {}
        }
    }
    Err(BmapError::Timeout(format!(
        "no answer from [{}.{}]",
        CONTROL.0, CONTROL.1
    )))
}

/// What the UI may ask for. Play and pause are one request: which of them is
/// sent is decided by the state the headphones report first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackRequest {
    PlayPause,
    Next,
    Previous,
}

/// The full record of one control passed on through the headphones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackChange {
    /// What was sent.
    pub control: Control,
    pub before: PlayState,
    /// The play state at the last read-back, for play and pause.
    pub after: Option<PlayState>,
}

impl PlaybackChange {
    /// Play or pause, and the headphones then reported it. Next and previous
    /// are never confirmed here: the headphones do not say which track is
    /// playing.
    pub fn is_confirmed(&self) -> bool {
        match self.control {
            Control::Play => self.after == Some(PlayState::Playing),
            Control::Pause => matches!(self.after, Some(PlayState::Paused | PlayState::Stopped)),
            Control::Next | Control::Previous => false,
        }
    }
}

/// Reads the play state, sends one control, and for play and pause reads
/// the state back until it is the one asked for or `policy` runs out.
pub fn control_playback<T: Transport>(
    raw: &T,
    request: PlaybackRequest,
    policy: VerifyPolicy,
    pause: fn(Duration),
) -> BmapResult<PlaybackChange> {
    let before = read_state(raw)?;
    let control = match request {
        PlaybackRequest::PlayPause if before == PlayState::Playing => Control::Pause,
        PlaybackRequest::PlayPause => Control::Play,
        PlaybackRequest::Next => Control::Next,
        PlaybackRequest::Previous => Control::Previous,
    };
    send_control(raw, control)?;

    let wanted = match control {
        Control::Play => Some(true),
        Control::Pause => Some(false),
        Control::Next | Control::Previous => None,
    };
    let mut after = None;
    if let Some(playing) = wanted {
        for _ in 0..policy.attempts.max(1) {
            pause(policy.interval);
            match read_state(raw) {
                Ok(state) => {
                    after = Some(state);
                    if (state == PlayState::Playing) == playing {
                        break;
                    }
                }
                Err(e) if is_link_failure(&e) => return Err(e),
                Err(_) => {}
            }
        }
    }
    Ok(PlaybackChange {
        control,
        before,
        after,
    })
}

#[cfg(test)]
pub mod testing {
    use super::*;

    /// A Bluetooth source, as the test headphones reported the phone.
    pub fn source_status(address: [u8; 6]) -> Vec<u8> {
        let mut frame = vec![0x05, 0x01, 0x03, 0x09, 0x00, 0x02, 0x01];
        frame.extend_from_slice(&address);
        frame
    }

    pub fn state_status(state: PlayState) -> Vec<u8> {
        let byte = match state {
            PlayState::Playing => PLAYING,
            PlayState::Paused => PAUSED,
            PlayState::Stopped => STOPPED,
        };
        vec![0x05, 0x04, 0x03, 0x03, byte, 0xFF, 0xFF]
    }

    pub fn control_processing(control: Control) -> Vec<u8> {
        vec![0x05, 0x03, 0x07, 0x01, control.code()]
    }

    /// A `[5.6]` status frame: the field and its text.
    pub fn field(field: u8, text: &str) -> Vec<u8> {
        let mut frame = vec![0x05, 0x06, 0x03, (1 + text.len()) as u8, field];
        frame.extend_from_slice(text.as_bytes());
        frame
    }

    /// The whole answer to `START [5.6]`, as the test headphones gave the
    /// Bose app.
    pub fn track_reply(title: &str, artist: &str, album: &str) -> Vec<u8> {
        let mut reply = vec![0x05, 0x06, 0x07, 0x00];
        reply.extend(field(0, title));
        reply.extend(field(1, artist));
        reply.extend(field(2, album));
        reply.extend([0x05, 0x06, 0x06, 0x00]);
        reply
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::bose::bmap::stream::testing::{data, ScriptedStream};
    use crate::bose::bmap::stream::{StreamTransport, Timing};
    use crate::bose::bmap::unit::testing::paired_info;

    const PHONE: [u8; 6] = [0x34, 0xF0, 0x43, 0xC9, 0xE0, 0xF6];

    fn transport(stream: &ScriptedStream) -> StreamTransport<ScriptedStream> {
        StreamTransport::new(stream.clone(), Timing::instant())
    }

    #[test]
    fn a_bluetooth_source_as_the_headphones_gave_it() {
        let payload = [0x00, 0x02, 0x01, 0x34, 0xF0, 0x43, 0xC9, 0xE0, 0xF6];
        assert_eq!(parse_source(&payload).unwrap(), Source::Bluetooth(PHONE));
        assert_eq!(parse_source(&[0x00, 0x02, 0x00]).unwrap(), Source::None);
        assert_eq!(parse_source(&[0x00, 0x06, 0x02]).unwrap(), Source::Auxiliary);
    }

    #[test]
    fn a_source_not_in_the_observed_shape_is_not_read() {
        assert!(parse_source(&[0x00, 0x02, 0x01, 0x34]).is_err());
        assert!(parse_source(&[0x00, 0x02, 0x09]).is_err());
        assert!(parse_source(&[0x00]).is_err());
    }

    #[test]
    fn the_play_state_as_observed() {
        assert_eq!(parse_state(&[0x01, 0xFF, 0xFF]).unwrap(), PlayState::Playing);
        assert_eq!(parse_state(&[0x02, 0xFF, 0xFF]).unwrap(), PlayState::Paused);
        assert_eq!(parse_state(&[0x00, 0xFF, 0xFF]).unwrap(), PlayState::Stopped);
        assert!(parse_state(&[0x07, 0xFF, 0xFF]).is_err());
        assert!(parse_state(&[]).is_err());
    }

    #[test]
    fn the_track_one_field_a_frame() {
        // The shape the Bose app was given, with the title replaced: an
        // empty artist and the album Android gives when there is none.
        let track = parse_track(&track_reply("A title", "", "Unknown Album")).unwrap();
        assert_eq!(track.title, "A title");
        assert_eq!(track.artist, "");
        assert_eq!(track.album, "Unknown Album");
        assert!(!track.is_empty());
        assert!(parse_track(&track_reply("", "", "")).unwrap().is_empty());
    }

    #[test]
    fn a_field_not_described_is_passed_over_and_text_is_cleaned() {
        let mut reply = vec![0x05, 0x06, 0x07, 0x00];
        reply.extend(field(0, "  Title\u{7}  "));
        reply.extend(field(9, "something else"));
        reply.extend([0x05, 0x06, 0x06, 0x00]);
        let track = parse_track(&reply).unwrap();
        assert_eq!(track.title, "Title");
        assert_eq!(track.artist, "");
    }

    #[test]
    fn the_track_behind_authentication_is_an_error() {
        // What the test headphones answer this application.
        let locked = [0x05, 0x06, 0x04, 0x01, 0x05];
        assert!(matches!(parse_track(&locked), Err(BmapError::Auth(_))));
        let refused = [0x05, 0x06, 0x04, 0x01, 0x04];
        assert!(matches!(parse_track(&refused), Err(BmapError::Device { code: 4, .. })));
        let other = [0x05, 0x04, 0x03, 0x03, 0x01, 0xFF, 0xFF];
        assert!(matches!(parse_track(&other), Err(BmapError::Timeout(_))));
    }

    #[test]
    fn the_control_codes_are_upstreams() {
        assert_eq!(Control::Play.code(), 1);
        assert_eq!(Control::Pause.code(), 2);
        assert_eq!(Control::Next.code(), 3);
        assert_eq!(Control::Previous.code(), 4);
    }

    #[test]
    fn the_source_is_named_once() {
        let stream = ScriptedStream::new()
            .on_send(vec![data(&source_status(PHONE))])
            .on_send(vec![data(&paired_info(PHONE, 0x01, "Jaisal's S24 Ultra"))])
            .on_send(vec![data(&state_status(PlayState::Playing))])
            .on_send(vec![data(&source_status(PHONE))])
            .on_send(vec![data(&state_status(PlayState::Paused))]);
        let raw = transport(&stream);
        let mut named = None;

        let first = read_playing(&raw, &mut named).unwrap().unwrap();
        assert_eq!(
            first.from,
            PlayingFrom::Device {
                name: "Jaisal's S24 Ultra".into(),
                this_device: false
            }
        );
        assert_eq!(first.state, PlayState::Playing);

        let second = read_playing(&raw, &mut named).unwrap().unwrap();
        assert_eq!(second.state, PlayState::Paused);
        // [5.1], [4.5], [5.4]; then [5.1] and [5.4] only.
        let asked: Vec<(u8, u8)> = stream.sent().iter().map(|f| (f[0], f[1])).collect();
        assert_eq!(asked, vec![(5, 1), (4, 5), (5, 4), (5, 1), (5, 4)]);
        assert!(stream.sent().iter().all(|f| f[2] == 0x01), "only GETs");
    }

    #[test]
    fn this_device_and_no_source() {
        let stream = ScriptedStream::new()
            .on_send(vec![data(&source_status(PHONE))])
            .on_send(vec![data(&paired_info(PHONE, 0x03, "DESKTOP"))])
            .on_send(vec![data(&state_status(PlayState::Stopped))])
            .on_send(vec![data(&[0x05, 0x01, 0x03, 0x03, 0x00, 0x02, 0x00])]);
        let raw = transport(&stream);
        let mut named = None;
        let playing = read_playing(&raw, &mut named).unwrap().unwrap();
        assert_eq!(
            playing.from,
            PlayingFrom::Device {
                name: "DESKTOP".into(),
                this_device: true
            }
        );
        assert_eq!(playing.state, PlayState::Stopped);
        assert_eq!(read_playing(&raw, &mut named).unwrap(), None);
    }

    #[test]
    fn a_source_the_headphones_will_not_name_is_unnamed() {
        let stream = ScriptedStream::new()
            .on_send(vec![data(&source_status(PHONE))])
            .on_send(vec![data(&[0x04, 0x05, 0x04, 0x01, 0x04])])
            .on_send(vec![data(&state_status(PlayState::Playing))]);
        let raw = transport(&stream);
        let mut named = None;
        let playing = read_playing(&raw, &mut named).unwrap().unwrap();
        assert_eq!(playing.from, PlayingFrom::Unnamed);
    }

    #[test]
    fn play_pause_sends_what_the_state_calls_for_and_reads_it_back() {
        let stream = ScriptedStream::new()
            .on_send(vec![data(&state_status(PlayState::Playing))])
            .on_send(vec![data(&control_processing(Control::Pause))])
            .on_send(vec![data(&state_status(PlayState::Playing))])
            .on_send(vec![data(&state_status(PlayState::Paused))]);
        let raw = transport(&stream);
        let change =
            control_playback(&raw, PlaybackRequest::PlayPause, VerifyPolicy::instant(5), |_| {})
                .unwrap();
        assert_eq!(change.control, Control::Pause);
        assert_eq!(change.after, Some(PlayState::Paused));
        assert!(change.is_confirmed());
        assert_eq!(stream.sent()[1], vec![0x05, 0x03, 0x05, 0x01, 0x02]);
    }

    #[test]
    fn play_that_does_not_take_is_not_confirmed() {
        let mut stream = ScriptedStream::new()
            .on_send(vec![data(&state_status(PlayState::Paused))])
            .on_send(vec![data(&control_processing(Control::Play))]);
        for _ in 0..3 {
            stream = stream.on_send(vec![data(&state_status(PlayState::Paused))]);
        }
        let raw = transport(&stream);
        let change =
            control_playback(&raw, PlaybackRequest::PlayPause, VerifyPolicy::instant(3), |_| {})
                .unwrap();
        assert_eq!(change.control, Control::Play);
        assert_eq!(change.after, Some(PlayState::Paused));
        assert!(!change.is_confirmed());
    }

    #[test]
    fn next_is_passed_on_and_never_called_confirmed() {
        let stream = ScriptedStream::new()
            .on_send(vec![data(&state_status(PlayState::Playing))])
            .on_send(vec![data(&control_processing(Control::Next))]);
        let raw = transport(&stream);
        let change =
            control_playback(&raw, PlaybackRequest::Next, VerifyPolicy::instant(3), |_| {}).unwrap();
        assert_eq!(change.control, Control::Next);
        assert_eq!(change.after, None);
        assert!(!change.is_confirmed());
        assert_eq!(stream.sent().len(), 2, "no read-back for a skip");
    }

    #[test]
    fn a_refused_control_is_an_error() {
        let stream = ScriptedStream::new()
            .on_send(vec![data(&state_status(PlayState::Playing))])
            .on_send(vec![data(&[0x05, 0x03, 0x04, 0x01, 0x05])]);
        let raw = transport(&stream);
        let result =
            control_playback(&raw, PlaybackRequest::Next, VerifyPolicy::instant(3), |_| {});
        assert!(matches!(result, Err(BmapError::Auth(_))));
    }
}
