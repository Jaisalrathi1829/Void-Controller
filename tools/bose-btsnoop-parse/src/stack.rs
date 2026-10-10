//! HCI → L2CAP → RFCOMM decoding.
//!
//! Enough of the stack to recover application payloads from a snoop log, and
//! no more. The goal is to answer one question: what bytes does Bose Music
//! exchange with the headphones over the vendor RFCOMM channel?
//!
//! Note the endianness trap: the btsnoop container is big-endian, everything
//! inside HCI is little-endian, and Bluetooth addresses inside HCI events are
//! additionally stored least-significant-byte first.

use crate::btsnoop::{Direction, Record};
use std::collections::HashMap;

// --- H4 packet types ---
const H4_ACL: u8 = 0x02;
const H4_EVENT: u8 = 0x04;

// --- HCI events ---
const EVT_CONNECTION_COMPLETE: u8 = 0x03;
const EVT_DISCONNECTION_COMPLETE: u8 = 0x05;

// --- L2CAP ---
const L2CAP_CID_SIGNALLING: u16 = 0x0001;
const L2CAP_SIG_CONNECTION_REQUEST: u8 = 0x02;
const L2CAP_SIG_CONNECTION_RESPONSE: u8 = 0x03;
const L2CAP_PSM_RFCOMM: u16 = 0x0003;

// --- RFCOMM frame types, with the P/F bit masked off ---
const RFCOMM_SABM: u8 = 0x2F;
const RFCOMM_UA: u8 = 0x63;
const RFCOMM_DM: u8 = 0x0F;
const RFCOMM_DISC: u8 = 0x43;
const RFCOMM_UIH: u8 = 0xEF;
/// Poll/Final. On a UIH frame for a data DLCI this signals a leading credit byte.
const RFCOMM_PF: u8 = 0x10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RfcommFrameKind {
    Sabm,
    Ua,
    Dm,
    Disc,
    Uih,
    Unknown(u8),
}

impl RfcommFrameKind {
    pub fn label(self) -> String {
        match self {
            Self::Sabm => "SABM".into(),
            Self::Ua => "UA".into(),
            Self::Dm => "DM".into(),
            Self::Disc => "DISC".into(),
            Self::Uih => "UIH".into(),
            Self::Unknown(c) => format!("UNKNOWN(0x{c:02X})"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RfcommFrame {
    pub timestamp_us: i64,
    pub direction: Direction,
    /// Remote Bluetooth address, if the connection was seen being established.
    pub address: Option<String>,
    pub dlci: u8,
    pub kind: RfcommFrameKind,
    /// Application payload, with any credit byte already removed.
    pub payload: Vec<u8>,
}

/// Reassembles fragmented ACL data and decodes what it can.
#[derive(Default)]
pub struct Decoder {
    /// HCI connection handle -> remote address.
    handles: HashMap<u16, String>,
    /// Partially received L2CAP PDUs, keyed by handle.
    pending: HashMap<u16, Vec<u8>>,
    /// L2CAP CIDs known to carry RFCOMM, keyed by handle.
    rfcomm_cids: HashMap<u16, Vec<u16>>,
    /// L2CAP signalling identifier -> source CID, for RFCOMM connect requests
    /// awaiting a response.
    pending_rfcomm: HashMap<(u16, u8), u16>,
}

fn le_u16(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

/// Formats a 6-byte little-endian address as conventional big-endian hex.
fn format_addr(b: &[u8]) -> String {
    let mut s = String::new();
    for i in (0..6).rev() {
        s.push_str(&format!("{:02X}", b[i]));
    }
    s
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one snoop record, returning any RFCOMM frames it completed.
    pub fn push(&mut self, rec: &Record) -> Vec<RfcommFrame> {
        if rec.data.is_empty() {
            return Vec::new();
        }
        match rec.data[0] {
            H4_EVENT => {
                self.handle_event(&rec.data[1..]);
                Vec::new()
            }
            H4_ACL => self.handle_acl(rec),
            _ => Vec::new(),
        }
    }

    fn handle_event(&mut self, ev: &[u8]) {
        if ev.len() < 2 {
            return;
        }
        let code = ev[0];
        let params = &ev[2..];

        match code {
            // Connection Complete: status, handle(2), bdaddr(6), ...
            EVT_CONNECTION_COMPLETE if params.len() >= 9 => {
                if params[0] == 0x00 {
                    let handle = le_u16(&params[1..3]) & 0x0FFF;
                    self.handles.insert(handle, format_addr(&params[3..9]));
                }
            }
            // Disconnection Complete: status, handle(2), reason
            EVT_DISCONNECTION_COMPLETE if params.len() >= 3 => {
                let handle = le_u16(&params[1..3]) & 0x0FFF;
                self.handles.remove(&handle);
                self.pending.remove(&handle);
                self.rfcomm_cids.remove(&handle);
            }
            _ => {}
        }
    }

    fn handle_acl(&mut self, rec: &Record) -> Vec<RfcommFrame> {
        let d = &rec.data;
        if d.len() < 5 {
            return Vec::new();
        }
        let header = le_u16(&d[1..3]);
        let handle = header & 0x0FFF;
        let pb = (header >> 12) & 0x03;
        let len = le_u16(&d[3..5]) as usize;
        let body = &d[5..];
        if body.len() < len {
            return Vec::new();
        }
        let body = &body[..len];

        // pb == 1 is a continuation of the previous PDU on this handle.
        let buf = self.pending.entry(handle).or_default();
        if pb == 1 {
            buf.extend_from_slice(body);
        } else {
            buf.clear();
            buf.extend_from_slice(body);
        }

        // An L2CAP B-frame is complete once we have length + 4 header bytes.
        if buf.len() < 4 {
            return Vec::new();
        }
        let l2cap_len = le_u16(&buf[0..2]) as usize;
        if buf.len() < l2cap_len + 4 {
            return Vec::new();
        }

        let cid = le_u16(&buf[2..4]);
        let payload = buf[4..4 + l2cap_len].to_vec();
        buf.clear();

        if cid == L2CAP_CID_SIGNALLING {
            self.handle_signalling(handle, &payload);
            return Vec::new();
        }

        let is_rfcomm = self
            .rfcomm_cids
            .get(&handle)
            .map(|v| v.contains(&cid))
            .unwrap_or(false);
        if !is_rfcomm {
            return Vec::new();
        }

        self.decode_rfcomm(rec, handle, &payload)
    }

    /// Watches L2CAP signalling for connections whose PSM is RFCOMM, so we
    /// know which CIDs to decode. Guessing by structure would misclassify
    /// other protocols; this only trusts an actual successful handshake.
    fn handle_signalling(&mut self, handle: u16, data: &[u8]) {
        let mut off = 0;
        while off + 4 <= data.len() {
            let code = data[off];
            let ident = data[off + 1];
            let len = le_u16(&data[off + 2..off + 4]) as usize;
            let start = off + 4;
            if start + len > data.len() {
                break;
            }
            let params = &data[start..start + len];

            match code {
                L2CAP_SIG_CONNECTION_REQUEST if params.len() >= 4 => {
                    let psm = le_u16(&params[0..2]);
                    let source_cid = le_u16(&params[2..4]);
                    if psm == L2CAP_PSM_RFCOMM {
                        self.pending_rfcomm.insert((handle, ident), source_cid);
                    }
                }
                L2CAP_SIG_CONNECTION_RESPONSE if params.len() >= 6 => {
                    let dest_cid = le_u16(&params[0..2]);
                    let source_cid = le_u16(&params[2..4]);
                    let result = le_u16(&params[4..6]);
                    if result == 0 {
                        if let Some(req_cid) = self.pending_rfcomm.remove(&(handle, ident)) {
                            let cids = self.rfcomm_cids.entry(handle).or_default();
                            // Both directions use different CIDs; track both.
                            for c in [dest_cid, source_cid, req_cid] {
                                if c != 0 && !cids.contains(&c) {
                                    cids.push(c);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            off = start + len;
        }
    }

    fn decode_rfcomm(&self, rec: &Record, handle: u16, data: &[u8]) -> Vec<RfcommFrame> {
        if data.len() < 3 {
            return Vec::new();
        }
        let dlci = data[0] >> 2;
        let control = data[1];
        let kind = match control & !RFCOMM_PF {
            RFCOMM_SABM => RfcommFrameKind::Sabm,
            RFCOMM_UA => RfcommFrameKind::Ua,
            RFCOMM_DM => RfcommFrameKind::Dm,
            RFCOMM_DISC => RfcommFrameKind::Disc,
            RFCOMM_UIH => RfcommFrameKind::Uih,
            other => RfcommFrameKind::Unknown(other),
        };

        // Length is 1 or 2 bytes depending on the EA bit.
        let (length, mut off) = if data[2] & 0x01 == 1 {
            ((data[2] >> 1) as usize, 3)
        } else {
            if data.len() < 4 {
                return Vec::new();
            }
            ((((data[3] as usize) << 7) | ((data[2] >> 1) as usize)), 4)
        };

        // A UIH frame on a data DLCI with P/F set carries a credit octet between
        // the length field and the information field. It belongs to flow
        // control, not to the application, so it is skipped.
        //
        // The length field does NOT count it. An earlier version of this
        // parser subtracted one from `length` here as well, which silently
        // dropped the final byte of every credit-carrying frame: Bose's
        // six-byte mode switch `1F 03 05 02 00 00` was reported as the
        // five-byte `1F 03 05 02 00`, and `00 01 01 00` as `00 01 01`. Frames
        // replayed from that output were therefore incomplete, and the device
        // — correctly treating RFCOMM as a byte stream — sat waiting for the
        // rest of a frame that never came.
        if kind == RfcommFrameKind::Uih && dlci != 0 && control & RFCOMM_PF != 0 {
            off += 1;
        }

        if off + length > data.len() {
            return Vec::new();
        }

        vec![RfcommFrame {
            timestamp_us: rec.timestamp_us,
            direction: rec.direction,
            address: self.handles.get(&handle).cloned(),
            dlci,
            kind,
            payload: data[off..off + length].to_vec(),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(data: Vec<u8>, dir: Direction) -> Record {
        Record {
            timestamp_us: 0,
            direction: dir,
            data,
        }
    }

    /// Builds an HCI Connection Complete event for a given handle/address.
    fn connection_complete(handle: u16, addr_le: [u8; 6]) -> Record {
        let mut d = vec![H4_EVENT, EVT_CONNECTION_COMPLETE, 11, 0x00];
        d.extend_from_slice(&handle.to_le_bytes());
        d.extend_from_slice(&addr_le);
        d.extend_from_slice(&[0x01, 0x00]);
        rec(d, Direction::Received)
    }

    /// Wraps an L2CAP payload in a single unfragmented ACL packet.
    fn acl(handle: u16, cid: u16, payload: &[u8], dir: Direction) -> Record {
        let mut l2cap = Vec::new();
        l2cap.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        l2cap.extend_from_slice(&cid.to_le_bytes());
        l2cap.extend_from_slice(payload);

        let mut d = vec![H4_ACL];
        d.extend_from_slice(&(handle | (2 << 12)).to_le_bytes());
        d.extend_from_slice(&(l2cap.len() as u16).to_le_bytes());
        d.extend_from_slice(&l2cap);
        rec(d, dir)
    }

    fn sig_connect_rfcomm(handle: u16, ident: u8, src_cid: u16, dst_cid: u16) -> Vec<Record> {
        let mut req = vec![L2CAP_SIG_CONNECTION_REQUEST, ident, 4, 0];
        req.extend_from_slice(&L2CAP_PSM_RFCOMM.to_le_bytes());
        req.extend_from_slice(&src_cid.to_le_bytes());

        let mut resp = vec![L2CAP_SIG_CONNECTION_RESPONSE, ident, 8, 0];
        resp.extend_from_slice(&dst_cid.to_le_bytes());
        resp.extend_from_slice(&src_cid.to_le_bytes());
        resp.extend_from_slice(&0u16.to_le_bytes()); // success
        resp.extend_from_slice(&0u16.to_le_bytes());

        vec![
            acl(handle, L2CAP_CID_SIGNALLING, &req, Direction::Sent),
            acl(handle, L2CAP_CID_SIGNALLING, &resp, Direction::Received),
        ]
    }

    /// RFCOMM UIH frame with a 1-byte length and no credit byte.
    fn uih(dlci: u8, payload: &[u8]) -> Vec<u8> {
        let mut f = vec![(dlci << 2) | 0x03, RFCOMM_UIH];
        f.push(((payload.len() as u8) << 1) | 0x01);
        f.extend_from_slice(payload);
        f.push(0x00); // FCS, not validated here
        f
    }

    #[test]
    fn addresses_are_byte_reversed_from_hci() {
        // HCI stores 2E:F0:F9:BC:58:E4 little-endian for address E458BCF9F02E.
        assert_eq!(
            format_addr(&[0x2E, 0xF0, 0xF9, 0xBC, 0x58, 0xE4]),
            "E458BCF9F02E"
        );
    }

    #[test]
    fn learns_the_address_for_a_handle() {
        let mut d = Decoder::new();
        d.push(&connection_complete(0x0C, [0x2E, 0xF0, 0xF9, 0xBC, 0x58, 0xE4]));
        assert_eq!(d.handles.get(&0x0C).map(String::as_str), Some("E458BCF9F02E"));
    }

    /// Traffic on a CID that was never negotiated as RFCOMM must be ignored,
    /// rather than guessed at structurally.
    #[test]
    fn ignores_cids_that_are_not_rfcomm() {
        let mut d = Decoder::new();
        d.push(&connection_complete(0x0C, [0; 6]));
        let frames = d.push(&acl(0x0C, 0x0041, &uih(3, b"hello"), Direction::Sent));
        assert!(frames.is_empty());
    }

    #[test]
    fn decodes_a_uih_payload_after_an_rfcomm_handshake() {
        let mut d = Decoder::new();
        d.push(&connection_complete(0x0C, [0x2E, 0xF0, 0xF9, 0xBC, 0x58, 0xE4]));
        for r in sig_connect_rfcomm(0x0C, 1, 0x0040, 0x0041) {
            d.push(&r);
        }
        let frames = d.push(&acl(0x0C, 0x0041, &uih(3, b"hello"), Direction::Sent));
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].kind, RfcommFrameKind::Uih);
        assert_eq!(frames[0].dlci, 3);
        assert_eq!(frames[0].payload, b"hello");
        assert_eq!(frames[0].address.as_deref(), Some("E458BCF9F02E"));
    }

    /// The credit byte is flow control and must not be reported as payload.
    #[test]
    fn strips_the_credit_byte_from_uih_with_pf_set() {
        let mut d = Decoder::new();
        d.push(&connection_complete(0x0C, [0; 6]));
        for r in sig_connect_rfcomm(0x0C, 1, 0x0040, 0x0041) {
            d.push(&r);
        }
        // Wire layout: address, control, LENGTH (information field only),
        // CREDIT octet, information, FCS. The length field does not count the
        // credit octet.
        let info = b"data";
        let mut f = vec![(3 << 2) | 0x03, RFCOMM_UIH | RFCOMM_PF];
        f.push(((info.len() as u8) << 1) | 0x01);
        f.push(0x07); // credits
        f.extend_from_slice(info);
        f.push(0x00); // FCS

        let frames = d.push(&acl(0x0C, 0x0041, &f, Direction::Sent));
        assert_eq!(frames[0].payload, b"data");
    }

    /// Regression test for the bug that broke Experiment 4.
    ///
    /// The previous fixture for the credit-byte case was built with the same
    /// wrong assumption as the code under test — that the length field counts
    /// the credit octet — so the test passed while the parser dropped the last
    /// byte of every such frame. This one uses the real Bose mode-switch frame.
    #[test]
    fn credit_carrying_frame_keeps_its_final_payload_byte() {
        let mut d = Decoder::new();
        d.push(&connection_complete(0x0C, [0; 6]));
        for r in sig_connect_rfcomm(0x0C, 1, 0x0040, 0x0041) {
            d.push(&r);
        }
        // START [31.3] quiet, silent: six bytes of BMAP.
        let bmap = [0x1F, 0x03, 0x05, 0x02, 0x00, 0x00];
        let mut f = vec![(16 << 2) | 0x03, RFCOMM_UIH | RFCOMM_PF]; // DLCI 16
        f.push(((bmap.len() as u8) << 1) | 0x01);
        f.push(0x01); // one credit granted
        f.extend_from_slice(&bmap);
        f.push(0x9A); // FCS (not validated)

        let frames = d.push(&acl(0x0C, 0x0041, &f, Direction::Sent));
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].dlci, 16);
        assert_eq!(
            frames[0].payload, bmap,
            "the announce byte must survive credit stripping"
        );
    }

    /// A credit-only frame: zero-length information field, credit octet present.
    #[test]
    fn credit_only_frame_has_an_empty_payload() {
        let mut d = Decoder::new();
        d.push(&connection_complete(0x0C, [0; 6]));
        for r in sig_connect_rfcomm(0x0C, 1, 0x0040, 0x0041) {
            d.push(&r);
        }
        let f = vec![(3 << 2) | 0x03, RFCOMM_UIH | RFCOMM_PF, 0x01, 0x21, 0x00];
        let frames = d.push(&acl(0x0C, 0x0041, &f, Direction::Received));
        assert_eq!(frames.len(), 1);
        assert!(frames[0].payload.is_empty());
    }

    #[test]
    fn reassembles_a_fragmented_l2cap_pdu() {
        let mut d = Decoder::new();
        d.push(&connection_complete(0x0C, [0; 6]));
        for r in sig_connect_rfcomm(0x0C, 1, 0x0040, 0x0041) {
            d.push(&r);
        }

        let frame = uih(3, b"fragmented payload");
        let mut l2cap = Vec::new();
        l2cap.extend_from_slice(&(frame.len() as u16).to_le_bytes());
        l2cap.extend_from_slice(&0x0041u16.to_le_bytes());
        l2cap.extend_from_slice(&frame);

        let (a, b) = l2cap.split_at(6);

        let mut first = vec![H4_ACL];
        first.extend_from_slice(&(0x0Cu16 | (2 << 12)).to_le_bytes());
        first.extend_from_slice(&(a.len() as u16).to_le_bytes());
        first.extend_from_slice(a);
        assert!(d.push(&rec(first, Direction::Sent)).is_empty());

        let mut second = vec![H4_ACL];
        second.extend_from_slice(&(0x0Cu16 | (1 << 12)).to_le_bytes()); // pb=1
        second.extend_from_slice(&(b.len() as u16).to_le_bytes());
        second.extend_from_slice(b);
        let frames = d.push(&rec(second, Direction::Sent));

        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].payload, b"fragmented payload");
    }

    #[test]
    fn disconnection_forgets_the_handle() {
        let mut d = Decoder::new();
        d.push(&connection_complete(0x0C, [0x2E, 0xF0, 0xF9, 0xBC, 0x58, 0xE4]));
        let mut ev = vec![H4_EVENT, EVT_DISCONNECTION_COMPLETE, 4, 0x00];
        ev.extend_from_slice(&0x0Cu16.to_le_bytes());
        ev.push(0x13);
        d.push(&rec(ev, Direction::Received));
        assert!(d.handles.is_empty());
    }
}
