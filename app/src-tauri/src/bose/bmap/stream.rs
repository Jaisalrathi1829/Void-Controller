//! BMAP over a raw byte stream.
//!
//! RFCOMM is a byte stream, not a message transport. One `recv` may return
//! half a BMAP frame, one frame, or several frames glued together, and the
//! boundaries the sender wrote are not preserved. This module is the layer
//! that turns that stream back into whole frames, and it is deliberately free
//! of sockets so every awkward case can be unit-tested.
//!
//! It implements the upstream [`bmap::Transport`] trait, so upstream's
//! `BmapConnection` — codec, model configs, reply validation — runs on top of
//! it unchanged. What this adds over upstream's Linux transport is reassembly:
//! upstream does a single `recv` 200 ms after sending and parses whatever
//! arrived, which silently drops a reply split across two reads.

use bmap::error::{BmapError, BmapResult};
use bmap::Transport;
use parking_lot::Mutex;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Size of the fixed BMAP header: `[fblock, function, flags|operator, length]`.
pub const HEADER_LEN: usize = 4;

/// What a single receive attempt produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recv {
    /// The peer sent these bytes. Never empty.
    Data(Vec<u8>),
    /// Nothing arrived within the timeout. Normal, not an error.
    Timeout,
    /// The peer closed the channel.
    Closed,
}

/// A raw, ordered byte stream: the socket with everything BMAP removed.
pub trait ByteStream {
    /// Transmit every byte, or fail.
    fn send(&mut self, bytes: &[u8]) -> io::Result<()>;

    /// Wait up to `timeout` for bytes. `Duration::ZERO` polls without blocking.
    fn recv(&mut self, timeout: Duration) -> io::Result<Recv>;
}

/// Reassembles BMAP frames from arbitrary chunks of a byte stream.
///
/// BMAP has no sync marker, so framing is purely positional: four header bytes
/// whose last byte is the payload length. That is also why stale input must be
/// discarded before each request — there is no way to resynchronise mid-stream.
#[derive(Debug, Default)]
pub struct FrameAssembler {
    buf: Vec<u8>,
}

impl FrameAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Removes and returns the next complete frame, header included.
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        if self.buf.len() < HEADER_LEN {
            return None;
        }
        let total = HEADER_LEN + self.buf[3] as usize;
        if self.buf.len() < total {
            return None;
        }
        Some(self.buf.drain(..total).collect())
    }

    /// Bytes held that do not yet form a complete frame.
    pub fn pending(&self) -> &[u8] {
        &self.buf
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }
}

/// Timeouts and delays. Every wait in the transport is bounded by one of these.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Pause between sending and the first read. Upstream documents this as
    /// required for the device to prepare its reply.
    pub settle: Duration,
    /// Longest wait for the reply to a request.
    pub reply_timeout: Duration,
    /// Silence that ends a multi-frame (drained) reply.
    pub drain_gap: Duration,
    /// Hard ceiling on a drained reply, however chatty the device is.
    pub drain_limit: Duration,
}

impl Timing {
    /// Values taken from upstream's transports (200 ms settle, 3 s reply,
    /// 500 ms drain gap).
    pub const fn standard() -> Self {
        Self {
            settle: Duration::from_millis(200),
            reply_timeout: Duration::from_secs(3),
            drain_gap: Duration::from_millis(500),
            drain_limit: Duration::from_secs(6),
        }
    }

    /// No real waiting. For unit tests driving a scripted stream.
    pub const fn instant() -> Self {
        Self {
            settle: Duration::ZERO,
            reply_timeout: Duration::from_millis(40),
            drain_gap: Duration::from_millis(5),
            drain_limit: Duration::from_millis(80),
        }
    }
}

/// Direction of a logged wire event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireDir {
    Tx,
    Rx,
    /// Received but thrown away: arrived before the request it would have
    /// been mistaken for an answer to.
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireEvent {
    pub dir: WireDir,
    pub bytes: Vec<u8>,
}

impl WireEvent {
    pub fn render(&self) -> String {
        let tag = match self.dir {
            WireDir::Tx => "TX   ",
            WireDir::Rx => "RX   ",
            WireDir::Stale => "STALE",
        };
        format!("{tag} {}", hex(&self.bytes))
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Upper bound on chunks discarded before a request, so a device streaming
/// notifications cannot stall it. Same bound as upstream.
const MAX_STALE_CHUNKS: usize = 64;

/// Most wire events retained for diagnostics.
const WIRE_LOG_CAP: usize = 256;

struct Inner<S: ByteStream> {
    stream: S,
    assembler: FrameAssembler,
    log: Vec<WireEvent>,
}

impl<S: ByteStream> Inner<S> {
    fn record(&mut self, dir: WireDir, bytes: &[u8]) {
        if self.log.len() >= WIRE_LOG_CAP {
            self.log.remove(0);
        }
        self.log.push(WireEvent {
            dir,
            bytes: bytes.to_vec(),
        });
    }

    /// Throws away anything already waiting.
    ///
    /// A late reply or an unsolicited notification left in the buffer would
    /// otherwise be read as the answer to the next request.
    fn discard_stale(&mut self) {
        if !self.assembler.pending().is_empty() {
            let leftover = self.assembler.pending().to_vec();
            self.record(WireDir::Stale, &leftover);
            self.assembler.clear();
        }
        for _ in 0..MAX_STALE_CHUNKS {
            match self.stream.recv(Duration::ZERO) {
                Ok(Recv::Data(bytes)) => self.record(WireDir::Stale, &bytes),
                // A dead socket is left for the send to report.
                Ok(Recv::Timeout) | Ok(Recv::Closed) | Err(_) => break,
            }
        }
    }
}

/// A BMAP transport over any [`ByteStream`].
///
/// Cloning yields another handle to the same stream, so a `BmapConnection` can
/// own one handle while the session keeps another for diagnostics.
pub struct StreamTransport<S: ByteStream> {
    inner: Arc<Mutex<Inner<S>>>,
    timing: Timing,
}

impl<S: ByteStream> std::fmt::Debug for StreamTransport<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately opaque: the stream is a live socket and the wire log
        // is device data that does not belong in incidental debug output.
        f.debug_struct("StreamTransport").finish_non_exhaustive()
    }
}

impl<S: ByteStream> Clone for StreamTransport<S> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            timing: self.timing,
        }
    }
}

impl<S: ByteStream> StreamTransport<S> {
    pub fn new(stream: S, timing: Timing) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                stream,
                assembler: FrameAssembler::new(),
                log: Vec::new(),
            })),
            timing,
        }
    }

    /// Returns and clears the wire log.
    pub fn take_wire_log(&self) -> Vec<WireEvent> {
        std::mem::take(&mut self.inner.lock().log)
    }

    /// Reads and discards anything the device sends unprompted on connect.
    ///
    /// Some models greet a new connection; upstream's Windows fork observed
    /// this on the QC Ultra. Bounded by `drain_gap`.
    pub fn drain_greeting(&self) {
        let mut inner = self.inner.lock();
        if let Ok(Recv::Data(bytes)) = inner.stream.recv(self.timing.drain_gap) {
            inner.record(WireDir::Stale, &bytes);
        }
        inner.discard_stale();
    }

    fn exchange(&self, packet: &[u8], drain: bool) -> BmapResult<Vec<u8>> {
        if packet.len() < HEADER_LEN {
            return Err(BmapError::InvalidArg(
                "BMAP packet shorter than its header".into(),
            ));
        }
        let want = (packet[0], packet[1]);
        let mut inner = self.inner.lock();

        inner.discard_stale();

        inner
            .stream
            .send(packet)
            .map_err(|e| BmapError::Connection(format!("Send failed: {e}")))?;
        inner.record(WireDir::Tx, packet);

        if !self.timing.settle.is_zero() {
            std::thread::sleep(self.timing.settle);
        }

        let started = Instant::now();
        let reply_deadline = started + self.timing.reply_timeout;
        let drain_deadline = started + self.timing.reply_timeout + self.timing.drain_limit;

        let mut frames: Vec<u8> = Vec::new();
        let mut unrelated = 0usize;
        let mut matched = false;

        loop {
            // Hand over every frame that is now complete.
            while let Some(frame) = inner.assembler.next_frame() {
                let from_requested = (frame[0], frame[1]) == want;
                if !from_requested {
                    unrelated += 1;
                }
                // A drained request is answered from a *different* address:
                // START [31.1] "get all modes" is replied to with a series of
                // [31.6] STATUS frames. So when draining, any frame begins the
                // reply; otherwise only one from the address we asked does.
                if from_requested || drain {
                    matched = true;
                }
                frames.extend_from_slice(&frame);
            }

            // A plain request is answered once a frame from the address we
            // asked has arrived whole. Anything still pending belongs to a
            // later frame and is left for the next request to discard.
            if matched && !drain {
                return Ok(frames);
            }

            let now = Instant::now();
            let wait = if matched {
                // Draining: keep going until the device falls silent.
                if now >= drain_deadline {
                    return Ok(frames);
                }
                self.timing.drain_gap.min(drain_deadline - now)
            } else {
                if now >= reply_deadline {
                    break;
                }
                reply_deadline - now
            };

            match inner.stream.recv(wait) {
                Ok(Recv::Data(bytes)) => {
                    inner.record(WireDir::Rx, &bytes);
                    inner.assembler.push(&bytes);
                }
                Ok(Recv::Timeout) => {
                    if matched {
                        // Silence after a drained reply: it is complete.
                        return Ok(frames);
                    }
                    // Otherwise loop; the deadline check above ends the wait.
                }
                Ok(Recv::Closed) => {
                    return Err(BmapError::Connection(
                        "The device closed the control channel".into(),
                    ));
                }
                Err(e) => {
                    return Err(BmapError::Connection(format!("Receive failed: {e}")));
                }
            }
        }

        // The deadline passed without a whole frame from the requested address.
        let ms = self.timing.reply_timeout.as_millis();
        let pending = inner.assembler.pending();
        let detail = if !pending.is_empty() {
            let have = pending.len();
            if have >= HEADER_LEN {
                format!(
                    "; an incomplete frame was received ({have} of {} bytes)",
                    HEADER_LEN + pending[3] as usize
                )
            } else {
                format!("; an incomplete frame header was received ({have} of {HEADER_LEN} bytes)")
            }
        } else if unrelated > 0 {
            format!("; {unrelated} unrelated frame(s) arrived instead")
        } else {
            String::new()
        };
        Err(BmapError::Timeout(format!(
            "No reply from [{}.{}] within {ms} ms{detail}",
            want.0, want.1
        )))
    }
}

impl<S: ByteStream> Transport for StreamTransport<S> {
    fn send_recv(&self, packet: &[u8]) -> BmapResult<Vec<u8>> {
        self.exchange(packet, false)
    }

    fn send_recv_drain(&self, packet: &[u8]) -> BmapResult<Vec<u8>> {
        self.exchange(packet, true)
    }
}

// ---------------------------------------------------------------------------
// Test support
// ---------------------------------------------------------------------------

/// A scripted byte stream for tests: no sockets, no real time.
///
/// Each `send` triggers the next scripted reaction, which queues chunks to be
/// returned by subsequent `recv` calls — one chunk per call, so a test controls
/// exactly where the stream is split.
#[cfg(test)]
pub mod testing {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Debug, Clone)]
    pub enum Chunk {
        Data(Vec<u8>),
        /// One `recv` reports a timeout before the next chunk.
        Silence,
        Close,
    }

    #[derive(Default)]
    pub struct ScriptState {
        /// Reactions consumed one per `send`.
        pub reactions: VecDeque<Vec<Chunk>>,
        pub incoming: VecDeque<Chunk>,
        pub sent: Vec<Vec<u8>>,
        pub fail_send: bool,
    }

    /// Cheap handle so a test can inspect what was sent after the transport
    /// has taken ownership of the stream.
    #[derive(Clone, Default)]
    pub struct ScriptedStream {
        pub state: Arc<Mutex<ScriptState>>,
    }

    impl ScriptedStream {
        pub fn new() -> Self {
            Self::default()
        }

        /// Queue the chunks the device emits in response to the next send.
        pub fn on_send(self, chunks: Vec<Chunk>) -> Self {
            self.state.lock().reactions.push_back(chunks);
            self
        }

        /// Bytes already waiting before anything is sent.
        pub fn preloaded(self, chunks: Vec<Chunk>) -> Self {
            self.state.lock().incoming.extend(chunks);
            self
        }

        pub fn sent(&self) -> Vec<Vec<u8>> {
            self.state.lock().sent.clone()
        }
    }

    impl ByteStream for ScriptedStream {
        fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
            let mut st = self.state.lock();
            if st.fail_send {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "scripted send failure"));
            }
            st.sent.push(bytes.to_vec());
            if let Some(reaction) = st.reactions.pop_front() {
                st.incoming.extend(reaction);
            }
            Ok(())
        }

        fn recv(&mut self, _timeout: Duration) -> io::Result<Recv> {
            let mut st = self.state.lock();
            match st.incoming.pop_front() {
                Some(Chunk::Data(b)) => Ok(Recv::Data(b)),
                Some(Chunk::Silence) | None => Ok(Recv::Timeout),
                Some(Chunk::Close) => Ok(Recv::Closed),
            }
        }
    }

    pub fn data(bytes: &[u8]) -> Chunk {
        Chunk::Data(bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    const GET_MODE: [u8; 4] = [0x1F, 0x03, 0x01, 0x00];
    const STATUS_AWARE: [u8; 5] = [0x1F, 0x03, 0x03, 0x01, 0x01];

    fn transport(stream: ScriptedStream) -> StreamTransport<ScriptedStream> {
        StreamTransport::new(stream, Timing::instant())
    }

    // --- FrameAssembler -----------------------------------------------------

    #[test]
    fn assembler_yields_nothing_until_the_header_is_complete() {
        let mut a = FrameAssembler::new();
        a.push(&[0x1F, 0x03, 0x03]);
        assert!(a.next_frame().is_none());
        assert_eq!(a.pending().len(), 3);
    }

    #[test]
    fn assembler_waits_for_the_whole_payload() {
        let mut a = FrameAssembler::new();
        a.push(&[0x1F, 0x03, 0x03, 0x01]); // header promises one payload byte
        assert!(a.next_frame().is_none());
        a.push(&[0x01]);
        assert_eq!(a.next_frame().unwrap(), STATUS_AWARE);
        assert!(a.pending().is_empty());
    }

    #[test]
    fn assembler_splits_concatenated_frames() {
        let mut a = FrameAssembler::new();
        // PROCESSING then RESULT, exactly as the device emits after a switch.
        a.push(&[0x1F, 0x03, 0x07, 0x00, 0x1F, 0x03, 0x06, 0x01, 0x00]);
        assert_eq!(a.next_frame().unwrap(), [0x1F, 0x03, 0x07, 0x00]);
        assert_eq!(a.next_frame().unwrap(), [0x1F, 0x03, 0x06, 0x01, 0x00]);
        assert!(a.next_frame().is_none());
    }

    #[test]
    fn assembler_keeps_a_trailing_partial_frame() {
        let mut a = FrameAssembler::new();
        a.push(&[0x1F, 0x03, 0x07, 0x00, 0x1F, 0x03]);
        assert_eq!(a.next_frame().unwrap(), [0x1F, 0x03, 0x07, 0x00]);
        assert!(a.next_frame().is_none());
        assert_eq!(a.pending(), [0x1F, 0x03]);
    }

    #[test]
    fn assembler_handles_a_zero_length_frame() {
        let mut a = FrameAssembler::new();
        a.push(&[0x1F, 0x03, 0x07, 0x00]);
        assert_eq!(a.next_frame().unwrap(), [0x1F, 0x03, 0x07, 0x00]);
    }

    #[test]
    fn assembler_reassembles_byte_by_byte() {
        let mut a = FrameAssembler::new();
        for (i, b) in STATUS_AWARE.iter().enumerate() {
            a.push(&[*b]);
            if i < STATUS_AWARE.len() - 1 {
                assert!(a.next_frame().is_none(), "frame emitted early at byte {i}");
            }
        }
        assert_eq!(a.next_frame().unwrap(), STATUS_AWARE);
    }

    // --- StreamTransport ----------------------------------------------------

    #[test]
    fn whole_reply_in_one_read() {
        let s = ScriptedStream::new().on_send(vec![data(&STATUS_AWARE)]);
        let t = transport(s.clone());
        assert_eq!(t.send_recv(&GET_MODE).unwrap(), STATUS_AWARE);
        assert_eq!(s.sent(), vec![GET_MODE.to_vec()]);
    }

    /// The case upstream's single-`recv` transport loses.
    #[test]
    fn reply_split_across_two_reads_is_reassembled() {
        let s = ScriptedStream::new().on_send(vec![
            data(&[0x1F, 0x03, 0x03]),
            data(&[0x01, 0x01]),
        ]);
        assert_eq!(transport(s).send_recv(&GET_MODE).unwrap(), STATUS_AWARE);
    }

    #[test]
    fn reply_split_inside_the_header_with_a_gap_is_reassembled() {
        let s = ScriptedStream::new().on_send(vec![
            data(&[0x1F]),
            Chunk::Silence,
            data(&[0x03, 0x03, 0x01]),
            Chunk::Silence,
            data(&[0x01]),
        ]);
        assert_eq!(transport(s).send_recv(&GET_MODE).unwrap(), STATUS_AWARE);
    }

    #[test]
    fn concatenated_frames_in_one_read_are_all_returned() {
        let mut both = vec![0x1F, 0x03, 0x07, 0x00];
        both.extend_from_slice(&[0x1F, 0x03, 0x06, 0x01, 0x00]);
        let s = ScriptedStream::new().on_send(vec![data(&both)]);
        let start = [0x1F, 0x03, 0x05, 0x02, 0x00, 0x00];
        assert_eq!(transport(s).send_recv(&start).unwrap(), both);
    }

    /// An unsolicited frame arriving first must not be taken for the reply,
    /// and must not end the wait.
    #[test]
    fn waits_past_an_unrelated_frame_for_the_real_reply() {
        let battery = [0x02, 0x02, 0x03, 0x01, 0x50];
        let s = ScriptedStream::new().on_send(vec![data(&battery), data(&STATUS_AWARE)]);
        let got = transport(s).send_recv(&GET_MODE).unwrap();
        let mut expected = battery.to_vec();
        expected.extend_from_slice(&STATUS_AWARE);
        assert_eq!(got, expected);
    }

    #[test]
    fn silence_is_a_timeout_not_a_reply() {
        let s = ScriptedStream::new();
        let err = transport(s).send_recv(&GET_MODE).unwrap_err();
        assert!(matches!(err, BmapError::Timeout(_)), "got {err:?}");
    }

    #[test]
    fn only_unrelated_frames_is_a_timeout_that_says_so() {
        let s = ScriptedStream::new().on_send(vec![data(&[0x02, 0x02, 0x03, 0x01, 0x50])]);
        match transport(s).send_recv(&GET_MODE).unwrap_err() {
            BmapError::Timeout(msg) => assert!(msg.contains("unrelated"), "{msg}"),
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    /// A reply that never completes is reported as incomplete, never handed on
    /// as if it were whole.
    #[test]
    fn truncated_reply_is_a_timeout_that_says_so() {
        let s = ScriptedStream::new().on_send(vec![data(&[0x1F, 0x03, 0x03, 0x01])]);
        match transport(s).send_recv(&GET_MODE).unwrap_err() {
            BmapError::Timeout(msg) => assert!(msg.contains("incomplete frame"), "{msg}"),
            other => panic!("expected Timeout, got {other:?}"),
        }
    }

    #[test]
    fn closed_channel_is_a_connection_error() {
        let s = ScriptedStream::new().on_send(vec![Chunk::Close]);
        let err = transport(s).send_recv(&GET_MODE).unwrap_err();
        assert!(matches!(err, BmapError::Connection(_)), "got {err:?}");
    }

    #[test]
    fn failed_send_is_a_connection_error_and_nothing_is_logged_as_sent() {
        let s = ScriptedStream::new();
        s.state.lock().fail_send = true;
        let t = transport(s);
        let err = t.send_recv(&GET_MODE).unwrap_err();
        assert!(matches!(err, BmapError::Connection(_)), "got {err:?}");
        assert!(t.take_wire_log().iter().all(|e| e.dir != WireDir::Tx));
    }

    /// A late frame left over from the previous exchange must not be read as
    /// the answer to this one.
    #[test]
    fn stale_input_is_discarded_before_a_request() {
        let late_quiet = [0x1F, 0x03, 0x06, 0x01, 0x00];
        let s = ScriptedStream::new()
            .preloaded(vec![data(&late_quiet)])
            .on_send(vec![data(&STATUS_AWARE)]);
        let t = transport(s);
        assert_eq!(t.send_recv(&GET_MODE).unwrap(), STATUS_AWARE);
        let log = t.take_wire_log();
        assert!(log
            .iter()
            .any(|e| e.dir == WireDir::Stale && e.bytes == late_quiet));
    }

    #[test]
    fn a_partial_frame_left_pending_does_not_corrupt_the_next_exchange() {
        // First exchange: reply plus the first two bytes of some later frame.
        let mut first = STATUS_AWARE.to_vec();
        first.extend_from_slice(&[0x02, 0x02]);
        let s = ScriptedStream::new()
            .on_send(vec![data(&first)])
            .on_send(vec![data(&[0x1F, 0x03, 0x03, 0x01, 0x00])]);
        let t = transport(s);
        assert_eq!(t.send_recv(&GET_MODE).unwrap(), STATUS_AWARE);
        // Second exchange must start clean, not glue onto the leftover bytes.
        assert_eq!(
            t.send_recv(&GET_MODE).unwrap(),
            [0x1F, 0x03, 0x03, 0x01, 0x00]
        );
    }

    #[test]
    fn drain_collects_frames_until_silence() {
        let a = [0x1F, 0x06, 0x03, 0x01, 0x00];
        let b = [0x1F, 0x06, 0x03, 0x01, 0x01];
        let s = ScriptedStream::new().on_send(vec![data(&a), data(&b), Chunk::Silence]);
        let get_all = [0x1F, 0x06, 0x05, 0x00];
        let got = transport(s).send_recv_drain(&get_all).unwrap();
        let mut expected = a.to_vec();
        expected.extend_from_slice(&b);
        assert_eq!(got, expected);
    }

    /// "Get all modes" is sent to [31.1] and answered from [31.6]. Requiring
    /// the reply address to match would time out on a perfectly good reply.
    #[test]
    fn drain_accepts_replies_from_a_different_address() {
        let a = [0x1F, 0x06, 0x03, 0x01, 0x00];
        let b = [0x1F, 0x06, 0x03, 0x01, 0x01];
        let s = ScriptedStream::new().on_send(vec![data(&a), data(&b), Chunk::Silence]);
        let get_all_modes = [0x1F, 0x01, 0x05, 0x00];
        let got = transport(s).send_recv_drain(&get_all_modes).unwrap();
        assert_eq!(got.len(), a.len() + b.len());
    }

    #[test]
    fn rejects_a_packet_shorter_than_a_header() {
        let err = transport(ScriptedStream::new())
            .send_recv(&[0x1F, 0x03])
            .unwrap_err();
        assert!(matches!(err, BmapError::InvalidArg(_)));
    }

    #[test]
    fn greeting_is_drained_and_not_mistaken_for_a_reply() {
        let greeting = [0xFF, 0x55, 0x02, 0x01, 0x00];
        let s = ScriptedStream::new()
            .preloaded(vec![data(&greeting)])
            .on_send(vec![data(&STATUS_AWARE)]);
        let t = transport(s);
        t.drain_greeting();
        assert_eq!(t.send_recv(&GET_MODE).unwrap(), STATUS_AWARE);
    }

    #[test]
    fn wire_log_records_both_directions() {
        let s = ScriptedStream::new().on_send(vec![data(&STATUS_AWARE)]);
        let t = transport(s);
        t.send_recv(&GET_MODE).unwrap();
        let log = t.take_wire_log();
        assert_eq!(log[0].render(), "TX    1F 03 01 00");
        assert_eq!(log[1].render(), "RX    1F 03 03 01 01");
        assert!(t.take_wire_log().is_empty(), "taking the log clears it");
    }
}
