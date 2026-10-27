//! USB/IP management and transfer framing (network byte order).
/// A USB/IP frame the bridge rejects.
#[derive(Debug, thiserror::Error)]
#[error("invalid USB/IP frame: {0}")]
pub struct UsbipError(pub &'static str);
/// Largest transfer the bridge accepts.
pub const MAX_TRANSFER: usize = 1024 * 1024;
/// A decoded `USBIP_CMD_SUBMIT` or `USBIP_CMD_UNLINK` header.
#[derive(Clone, Copy, Debug)]
pub struct Urb {
    /// 1 for submit, 2 for unlink.
    pub command: u32,
    /// Request sequence number, echoed in the reply.
    pub seq: u32,
    /// Device ID.
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
        if ![1, 2].contains(&command) {
            return Err(UsbipError("unsupported command"));
        }
        let length = if command == 1 {
            usize::try_from(word(length)).unwrap_or(usize::MAX)
        } else {
            0
        };
        let (direction, ep) = (word(direction), word(ep));
        if length > MAX_TRANSFER || direction > 1 || ep > 15 || (command == 1 && word(packets) != 0)
        {
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
    /// Encodes the header; the inverse of [`Self::decode`].
    #[must_use]
    pub fn header(self) -> [u8; 48] {
        let unlink_seq = if self.command == 2 {
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
    // CMD_UNLINK (2) is answered by RET_UNLINK (4), everything else by RET_SUBMIT (3);
    // only a submit reply carries actual_length.
    let command = if urb.command == 2 { 4 } else { 3 };
    let actual = if urb.command == 1 {
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
            valid: header[..2] == [1, 0x11] && header[4..] == [0; 4],
        }
    }
    /// Whether this is `OP_REQ_IMPORT`.
    #[must_use]
    pub const fn is_import(self) -> bool {
        self.op == 0x8003
    }
    /// Whether the header is valid and `busid` names the exported device "1-1".
    #[must_use]
    pub fn valid_busid(self, busid: &[u8; 32]) -> bool {
        self.valid && busid[..3] == *b"1-1" && busid[3..].iter().all(|b| *b == 0)
    }
    /// Whether the connection stays open for another request (`OP_REQ_DEVLIST`).
    #[must_use]
    pub const fn keep_open(self) -> bool {
        self.valid && self.op == 0x8005
    }
    /// The reply to send; an import's `busid` decides whether it succeeds.
    #[must_use]
    pub fn reply(self, busid: Option<&[u8; 32]>) -> Vec<u8> {
        let valid = match self.op {
            0x8005 => self.valid,
            0x8003 => busid.is_some_and(|busid| self.valid_busid(busid)),
            _ => false,
        };
        management_reply(self.op & 0x7fff, valid)
    }
}
/// Fixed single-device export. Descriptors and interface classes match VIIPER.
#[must_use]
pub fn management_reply(op: u16, valid: bool) -> Vec<u8> {
    let [op_hi, op_lo] = op.to_be_bytes();
    let mut b = vec![1, 0x11, op_hi, op_lo, 0, 0, 0, u8::from(!valid)];
    if !valid {
        return b;
    }
    if op == 5 {
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
    if op == 5 {
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
            command: 1,
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
    fn management_headers_and_import_busids_preserve_reply_contract() {
        let mut busid = [0; 32];
        busid[..3].copy_from_slice(b"1-1");
        for (op, reply_op) in [(0x8005u16, 5), (0x8003, 3), (0x8006, 6), (5, 5)] {
            let mut header = [1, 0x11, 0, 0, 0, 0, 0, 0];
            header[2..4].copy_from_slice(&op.to_be_bytes());
            for invalid in [None, Some(0), Some(1), Some(4), Some(7)] {
                let mut header = header;
                if let Some(offset) = invalid {
                    header[offset] ^= 1;
                }
                let request = ManagementRequest::parse(&header);
                let valid = invalid.is_none() && matches!(op, 0x8005 | 0x8003);
                assert_eq!(request.is_import(), op == 0x8003);
                assert_eq!(request.keep_open(), invalid.is_none() && op == 0x8005);
                assert_eq!(
                    request.reply(Some(&busid)),
                    management_reply(reply_op, valid)
                );
                if op == 0x8003 {
                    assert_eq!(request.valid_busid(&busid), valid);
                    assert_eq!(request.reply(None), management_reply(3, false));
                    for offset in [0, 2, 3, 31] {
                        let mut bad_busid = busid;
                        bad_busid[offset] ^= 1;
                        assert!(!request.valid_busid(&bad_busid));
                        assert_eq!(request.reply(Some(&bad_busid)), management_reply(3, false));
                    }
                }
            }
        }
    }
}
