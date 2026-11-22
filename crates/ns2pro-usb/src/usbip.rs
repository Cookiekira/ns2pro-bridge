//! USB/IP management and transfer framing (network byte order).
/// A USB/IP frame the bridge rejects.
#[derive(Debug, thiserror::Error)]
#[error("invalid USB/IP frame: {0}")]
pub struct UsbipError(pub &'static str);
/// Largest transfer the bridge accepts.
pub const MAX_TRANSFER: usize = 1024 * 1024;
/// USB/IP management protocol version (1.1.1).
const USBIP_VERSION: [u8; 2] = [1, 0x11];
const CMD_SUBMIT: u32 = 1;
const CMD_UNLINK: u32 = 2;
const RET_SUBMIT: u32 = 3;
const RET_UNLINK: u32 = 4;
const OP_REQ_IMPORT: u16 = 0x8003;
const OP_REQ_DEVLIST: u16 = 0x8005;
const OP_REP_DEVLIST: u16 = 0x0005;
const OP_REPLY_MASK: u16 = 0x7fff;
/// `number_of_packets` value (-1) that marks a submit as not isochronous.
const NON_ISOCHRONOUS: u32 = u32::MAX;
/// A decoded `USBIP_CMD_SUBMIT` or `USBIP_CMD_UNLINK` header.
#[derive(Clone, Copy, Debug)]
pub struct Urb {
    /// 1 for submit, 2 for unlink.
    pub command: u32,
    /// Request sequence number, echoed in the reply.
    pub seq: u32,
    /// Device ID, echoed in the reply; not validated.
    pub devid: u32,
    /// 0 for OUT, 1 for IN.
    pub direction: u32,
    /// Endpoint number.
    pub ep: u32,
    /// Transfer buffer length; 0 for unlink.
    pub length: usize,
    /// Control setup packet.
    pub setup: [u8; 8],
    /// For unlink, the sequence number to cancel.
    pub unlink_seq: u32,
}
impl Urb {
    /// Decodes a `USBIP_CMD_SUBMIT` or `USBIP_CMD_UNLINK` header.
    ///
    /// # Errors
    ///
    /// For other commands, oversize transfers, invalid endpoints and
    /// isochronous submissions.
    pub fn decode(h: &[u8; 48]) -> Result<Self, UsbipError> {
        let (words, _) = h.as_chunks::<4>();
        let &[
            command,
            seq,
            devid,
            direction,
            ep,
            unlink_seq,
            length,
            _,
            packets,
            _,
            s0,
            s1,
        ] = words
        else {
            unreachable!("48 bytes are 12 words");
        };
        let word = u32::from_be_bytes;
        let command = word(command);
        if !matches!(command, CMD_SUBMIT | CMD_UNLINK) {
            return Err(UsbipError("unsupported command"));
        }
        let length = if command == CMD_SUBMIT {
            usize::try_from(word(length)).unwrap_or(usize::MAX)
        } else {
            0
        };
        let (direction, ep) = (word(direction), word(ep));
        // A non-isochronous submit carries number_of_packets 0 (Linux vhci-hcd) or
        // 0xffffffff (-1), the protocol's "not isochronous" marker that usbip-win2 sends.
        let isochronous = command == CMD_SUBMIT && !matches!(word(packets), 0 | NON_ISOCHRONOUS);
        if length > MAX_TRANSFER || direction > 1 || ep > 15 || isochronous {
            return Err(UsbipError(
                "invalid length, endpoint or isochronous request",
            ));
        }
        let [a0, a1, a2, a3] = s0;
        let [b0, b1, b2, b3] = s1;
        Ok(Self {
            command,
            seq: word(seq),
            devid: word(devid),
            direction,
            ep,
            length,
            setup: [a0, a1, a2, a3, b0, b1, b2, b3],
            unlink_seq: word(unlink_seq),
        })
    }
    /// Encodes the header. `unlink_seq` is written only for unlink, fields that
    /// [`Self::decode`] does not keep are zero, and `length` keeps its low 32 bits.
    #[must_use]
    pub fn header(self) -> [u8; 48] {
        let unlink_seq = if self.command == CMD_UNLINK {
            self.unlink_seq
        } else {
            0
        };
        let words = [
            self.command,
            self.seq,
            self.devid,
            self.direction,
            self.ep,
            unlink_seq,
            wire_length(self.length),
            0,
            0,
            0,
        ];
        let mut h = [0; 48];
        let bytes = words
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .chain(self.setup);
        for (slot, byte) in h.iter_mut().zip(bytes) {
            *slot = byte;
        }
        h
    }
}
/// USB/IP length fields carry the low 32 bits of a length.
#[expect(
    clippy::cast_possible_truncation,
    clippy::as_conversions,
    reason = "the wire field is 32 bits; decode bounds lengths by MAX_TRANSFER"
)]
const fn wire_length(length: usize) -> u32 {
    length as u32
}
/// Appends a `USBIP_RET_SUBMIT` (or `USBIP_RET_UNLINK`) reply for `urb` to `out`.
pub fn reply_into(out: &mut Vec<u8>, urb: Urb, status: i32, payload: &[u8], actual: usize) {
    // Only a submit reply carries actual_length.
    let command = if urb.command == CMD_UNLINK {
        RET_UNLINK
    } else {
        RET_SUBMIT
    };
    let actual = if urb.command == CMD_SUBMIT {
        wire_length(actual)
    } else {
        0
    };
    let words = [
        command,
        urb.seq,
        urb.devid,
        urb.direction,
        urb.ep,
        status.cast_unsigned(),
        actual,
        0,
        0,
        0,
        0,
        0,
    ];
    out.extend(words.into_iter().flat_map(u32::to_be_bytes));
    out.extend_from_slice(payload);
}
/// A management header retains validity separately so malformed imports still
/// consume their busid before receiving the same failure reply.
#[derive(Clone, Copy, Debug)]
pub struct ManagementRequest {
    op: u16,
    valid: bool,
}
impl ManagementRequest {
    /// Parses an 8-byte management header.
    #[must_use]
    pub fn parse(header: &[u8; 8]) -> Self {
        Self {
            op: u16::from_be_bytes([header[2], header[3]]),
            valid: header[..2] == USBIP_VERSION && header[4..] == [0; 4],
        }
    }
    /// Whether this is `OP_REQ_IMPORT`.
    #[must_use]
    pub const fn is_import(self) -> bool {
        self.op == OP_REQ_IMPORT
    }
    /// Whether the header is valid and `busid` names the exported device "1-1".
    #[must_use]
    pub fn valid_busid(self, busid: &[u8; 32]) -> bool {
        self.valid && busid[..3] == *b"1-1" && busid[3..].iter().all(|b| *b == 0)
    }
    /// Whether the connection stays open for another request (`OP_REQ_DEVLIST`).
    #[must_use]
    pub const fn keep_open(self) -> bool {
        self.valid && self.op == OP_REQ_DEVLIST
    }
    /// The reply to send; an import's `busid` decides whether it succeeds.
    #[must_use]
    pub fn reply(self, busid: Option<&[u8; 32]>) -> Vec<u8> {
        let valid = match self.op {
            OP_REQ_DEVLIST => self.valid,
            OP_REQ_IMPORT => busid.is_some_and(|busid| self.valid_busid(busid)),
            _ => false,
        };
        management_reply(self.op & OP_REPLY_MASK, valid)
    }
}
/// Fixed single-device export. Descriptors and interface classes match VIIPER.
#[must_use]
pub fn management_reply(op: u16, valid: bool) -> Vec<u8> {
    let [ver_hi, ver_lo] = USBIP_VERSION;
    let [op_hi, op_lo] = op.to_be_bytes();
    let mut b = vec![ver_hi, ver_lo, op_hi, op_lo, 0, 0, 0, u8::from(!valid)];
    if !valid {
        return b;
    }
    if op == OP_REP_DEVLIST {
        b.extend(1u32.to_be_bytes());
    }
    let mut dev = [0; 312];
    let path = b"/sys/devices/pci0000:00/0000:00:08.1/0000:00:04:00.3/usb1/1-1";
    for (slot, byte) in dev.iter_mut().zip(path) {
        *slot = *byte;
    }
    dev[256..259].copy_from_slice(b"1-1");
    dev[288..292].copy_from_slice(&1u32.to_be_bytes());
    dev[292..296].copy_from_slice(&1u32.to_be_bytes());
    dev[296..300].copy_from_slice(&2u32.to_be_bytes());
    dev[300..].copy_from_slice(&[5, 0x7e, 0x20, 0x69, 2, 0, 0xef, 2, 1, 1, 1, 2]);
    b.extend(dev);
    if op == OP_REP_DEVLIST {
        b.extend([3, 0, 0, 0, 0xff, 0, 0, 0]);
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn oversized_lengths_keep_their_low_32_bits() {
        let urb = Urb {
            command: CMD_SUBMIT,
            seq: 0,
            devid: 0,
            direction: 0,
            ep: 0,
            length: 0x1_0000_0005,
            setup: [0; 8],
            unlink_seq: 0,
        };
        assert_eq!(urb.header()[24..28], 5u32.to_be_bytes());
        let mut out = Vec::new();
        reply_into(&mut out, urb, 0, &[], 0x1_0000_0007);
        assert_eq!(out[24..28], 7u32.to_be_bytes());
    }

    #[test]
    fn non_isochronous_submits_accept_both_wire_markers() {
        let submit = |packets: u32| {
            let mut h = [0; 48];
            h[..4].copy_from_slice(&CMD_SUBMIT.to_be_bytes());
            h[12..16].copy_from_slice(&1u32.to_be_bytes()); // IN
            h[24..28].copy_from_slice(&18u32.to_be_bytes()); // device descriptor length
            h[32..36].copy_from_slice(&packets.to_be_bytes());
            Urb::decode(&h)
        };
        // Linux vhci-hcd sends 0; usbip-win2 sends -1 (0xffffffff).
        for packets in [0, NON_ISOCHRONOUS] {
            let urb = submit(packets).expect("non-isochronous submit");
            assert_eq!(
                (urb.command, urb.direction, urb.length),
                (CMD_SUBMIT, 1, 18)
            );
        }
        for packets in [1, 8, 1024, NON_ISOCHRONOUS - 1] {
            assert!(submit(packets).is_err(), "isochronous {packets}");
        }
    }

    #[test]
    fn management_headers_and_import_busids_preserve_reply_contract() {
        const OP_REP_IMPORT: u16 = OP_REQ_IMPORT & OP_REPLY_MASK;
        let mut busid = [0; 32];
        busid[..3].copy_from_slice(b"1-1");
        for (op, reply_op) in [
            (OP_REQ_DEVLIST, OP_REP_DEVLIST),
            (OP_REQ_IMPORT, OP_REP_IMPORT),
            (0x8006, 6),
            (OP_REP_DEVLIST, OP_REP_DEVLIST),
        ] {
            let mut header = [USBIP_VERSION[0], USBIP_VERSION[1], 0, 0, 0, 0, 0, 0];
            header[2..4].copy_from_slice(&op.to_be_bytes());
            for invalid in [None, Some(0), Some(1), Some(4), Some(7)] {
                let mut header = header;
                if let Some(offset) = invalid {
                    header[offset] ^= 1;
                }
                let request = ManagementRequest::parse(&header);
                let valid = invalid.is_none() && matches!(op, OP_REQ_DEVLIST | OP_REQ_IMPORT);
                assert_eq!(request.is_import(), op == OP_REQ_IMPORT);
                assert_eq!(
                    request.keep_open(),
                    invalid.is_none() && op == OP_REQ_DEVLIST
                );
                assert_eq!(
                    request.reply(Some(&busid)),
                    management_reply(reply_op, valid)
                );
                if op == OP_REQ_IMPORT {
                    assert_eq!(request.valid_busid(&busid), valid);
                    assert_eq!(request.reply(None), management_reply(OP_REP_IMPORT, false));
                    for offset in [0, 2, 3, 31] {
                        let mut bad_busid = busid;
                        bad_busid[offset] ^= 1;
                        assert!(!request.valid_busid(&bad_busid));
                        assert_eq!(
                            request.reply(Some(&bad_busid)),
                            management_reply(OP_REP_IMPORT, false)
                        );
                    }
                }
            }
        }
    }
}
