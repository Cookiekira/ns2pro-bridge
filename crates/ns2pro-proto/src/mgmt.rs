//! Linux Bluetooth management wire format (little endian, independent of I/O).
use crate::{
    Address, LeAddressType,
    advertisement::{self, AdvertisementMode},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("malformed Bluetooth management packet")]
pub struct Malformed;
fn word(p: &[u8], n: usize) -> Result<u16, Malformed> {
    p.get(n..)
        .and_then(<[u8]>::first_chunk)
        .copied()
        .map(u16::from_le_bytes)
        .ok_or(Malformed)
}
pub const READ_INDEX_LIST: u16 = 3;
pub const READ_INFO: u16 = 4;
pub const SET_POWERED: u16 = 5;
pub const DISCONNECT: u16 = 0x14;
pub const GET_CONNECTIONS: u16 = 0x15;
pub const START_DISCOVERY: u16 = 0x23;
pub const STOP_DISCOVERY: u16 = 0x24;
pub const DEVICE_FOUND: u16 = 0x12;
pub const LE_DISCOVERY: u8 = 6;
pub const BUSY: u8 = 10;
/// Management also represents classic Bluetooth connections (wire value 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressType {
    BrEdr,
    Le(LeAddressType),
}
impl AddressType {
    /// Decodes a management address type.
    ///
    /// # Errors
    ///
    /// [`Malformed`] for a code that is neither BR/EDR nor a known LE type.
    pub fn from_code(code: u8) -> Result<Self, Malformed> {
        if code == 0 {
            Ok(Self::BrEdr)
        } else {
            LeAddressType::from_mgmt(code)
                .map(Self::Le)
                .ok_or(Malformed)
        }
    }
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::BrEdr => 0,
            Self::Le(kind) => kind.to_mgmt(),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device {
    pub address: Address,
    pub address_type: AddressType,
}
impl Device {
    /// Decodes a little-endian address followed by its address type.
    ///
    /// # Errors
    ///
    /// [`Malformed`] when `p` is shorter than 7 bytes or the type is unknown.
    pub fn decode(p: &[u8]) -> Result<Self, Malformed> {
        let mut bytes: [u8; 6] = p
            .get(..6)
            .ok_or(Malformed)?
            .try_into()
            .map_err(|_| Malformed)?;
        bytes.reverse();
        let address_type = AddressType::from_code(*p.get(6).ok_or(Malformed)?)?;
        Ok(Self {
            address: Address::from_be_bytes(bytes),
            address_type,
        })
    }
    #[must_use]
    pub fn encode(self) -> [u8; 7] {
        let mut p = [0; 7];
        p[..6].copy_from_slice(&self.address.to_le_bytes());
        p[6] = self.address_type.code();
        p
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame<'a> {
    pub opcode: u16,
    pub index: u16,
    pub payload: &'a [u8],
}
impl<'a> Frame<'a> {
    /// Splits a management frame into its header fields and payload.
    ///
    /// # Errors
    ///
    /// [`Malformed`] when the header is short or its length field disagrees with `p`.
    pub fn decode(p: &'a [u8]) -> Result<Self, Malformed> {
        let &[o0, o1, i0, i1, l0, l1, ref payload @ ..] = p else {
            return Err(Malformed);
        };
        if usize::from(u16::from_le_bytes([l0, l1])) != payload.len() {
            return Err(Malformed);
        }
        Ok(Self {
            opcode: u16::from_le_bytes([o0, o1]),
            index: u16::from_le_bytes([i0, i1]),
            payload,
        })
    }
}
/// Encodes a management command frame.
///
/// # Errors
///
/// [`Malformed`] when `payload` does not fit the 16-bit length field.
pub fn command(opcode: u16, index: u16, payload: &[u8]) -> Result<Vec<u8>, Malformed> {
    let len = u16::try_from(payload.len()).map_err(|_| Malformed)?;
    let mut out = Vec::with_capacity(6 + payload.len());
    out.extend(opcode.to_le_bytes());
    out.extend(index.to_le_bytes());
    out.extend(len.to_le_bytes());
    out.extend(payload);
    Ok(out)
}
#[derive(Debug, PartialEq, Eq)]
pub struct Completion<'a> {
    pub command: u16,
    pub status: u8,
    pub data: &'a [u8],
}
/// Extracts a Command Complete or Command Status event; other events yield `None`.
///
/// # Errors
///
/// [`Malformed`] when a completion payload is shorter than its 3-byte header.
pub const fn completion(frame: Frame<'_>) -> Result<Option<Completion<'_>>, Malformed> {
    match frame.opcode {
        1 | 2 => {
            let &[c0, c1, status, ref data @ ..] = frame.payload else {
                return Err(Malformed);
            };
            Ok(Some(Completion {
                command: u16::from_le_bytes([c0, c1]),
                status,
                data,
            }))
        }
        _ => Ok(None),
    }
}
/// Decodes a Read Controller Index List reply.
///
/// # Errors
///
/// [`Malformed`] when the length disagrees with the index count.
pub fn indexes(p: &[u8]) -> Result<Vec<u16>, Malformed> {
    let count = usize::from(word(p, 0)?);
    if p.len() != 2 + 2 * count {
        return Err(Malformed);
    }
    (0..count).map(|i| word(p, 2 + i * 2)).collect()
}
/// Decodes a Get Connections reply.
///
/// # Errors
///
/// [`Malformed`] when the length disagrees with the connection count or an
/// address type is unknown.
pub fn connections(p: &[u8]) -> Result<Vec<Device>, Malformed> {
    let count = usize::from(word(p, 0)?);
    if p.len() != 2 + 7 * count {
        return Err(Malformed);
    }
    p.get(2..)
        .ok_or(Malformed)?
        .as_chunks::<7>()
        .0
        .iter()
        .map(|p| Device::decode(p))
        .collect()
}
#[derive(Debug, Clone, Copy)]
pub struct AdapterInfo {
    pub address: Address,
    pub supported: u32,
    pub current: u32,
}
impl AdapterInfo {
    #[must_use]
    pub const fn supports_le(&self) -> bool {
        self.supported & (1 << 9) != 0
    }
    #[must_use]
    pub const fn powered(&self) -> bool {
        self.current & 1 != 0
    }
}
/// Decodes a Read Controller Information reply.
///
/// # Errors
///
/// [`Malformed`] unless `p` is exactly 280 bytes.
pub fn adapter_info(p: &[u8]) -> Result<AdapterInfo, Malformed> {
    if p.len() != 280 {
        return Err(Malformed);
    }
    // Address (LE), version, manufacturer, supported and current settings.
    let Some(
        &[
            a0,
            a1,
            a2,
            a3,
            a4,
            a5,
            _,
            _,
            _,
            s0,
            s1,
            s2,
            s3,
            c0,
            c1,
            c2,
            c3,
        ],
    ) = p.first_chunk::<17>()
    else {
        return Err(Malformed);
    };
    Ok(AdapterInfo {
        address: Address::from_be_bytes([a5, a4, a3, a2, a1, a0]),
        supported: u32::from_le_bytes([s0, s1, s2, s3]),
        current: u32::from_le_bytes([c0, c1, c2, c3]),
    })
}
#[derive(Debug)]
pub struct Found<'a> {
    pub device: Device,
    pub rssi: i8,
    pub flags: u32,
    pub eir: &'a [u8],
}
/// Decodes a Device Found event.
///
/// # Errors
///
/// [`Malformed`] when the length disagrees with the EIR length field or the
/// address type is unknown.
pub fn found(p: &[u8]) -> Result<Found<'_>, Malformed> {
    // Device (7 bytes), RSSI, flags, EIR length, EIR.
    let &[
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        rssi,
        f0,
        f1,
        f2,
        f3,
        e0,
        e1,
        ref eir @ ..,
    ] = p
    else {
        return Err(Malformed);
    };
    if eir.len() != usize::from(u16::from_le_bytes([e0, e1])) {
        return Err(Malformed);
    }
    Ok(Found {
        device: Device::decode(p)?,
        rssi: i8::from_le_bytes([rssi]),
        flags: u32::from_le_bytes([f0, f1, f2, f3]),
        eir,
    })
}
/// Returns the first controller advertisement in an EIR block.
///
/// # Errors
///
/// [`Malformed`] when an EIR structure overruns the block.
pub fn controller_advertisement(mut eir: &[u8]) -> Result<Option<AdvertisementMode>, Malformed> {
    let mut result = None;
    // Each EIR structure is a length byte followed by that many bytes (type first).
    while let Some((&length, rest)) = eir.split_first() {
        if length == 0 {
            break;
        }
        let (structure, next) = rest
            .split_at_checked(usize::from(length))
            .ok_or(Malformed)?;
        if let &[0xff, c0, c1, ref data @ ..] = structure {
            result = result.or_else(|| advertisement::parse(u16::from_le_bytes([c0, c1]), data));
        }
        eir = next;
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn management_roundtrip_and_truncation() {
        let p = command(START_DISCOVERY, 3, &[6]).unwrap();
        let f = Frame::decode(&p).unwrap();
        assert_eq!(
            f,
            Frame {
                opcode: START_DISCOVERY,
                index: 3,
                payload: &[6]
            }
        );
        for n in 0..p.len() {
            assert!(Frame::decode(&p[..n]).is_err());
        }
        assert!(Frame::decode(&[1, 0, 0, 0, 0, 0, 1]).is_err());
    }
    #[test]
    fn classic_and_le_management_codes_roundtrip() {
        for code in 0..=u8::MAX {
            let mut bytes = [0; 7];
            bytes[6] = code;
            if code <= 2 {
                assert_eq!(Device::decode(&bytes).unwrap().encode(), bytes);
            } else {
                assert_eq!(Device::decode(&bytes), Err(Malformed));
            }
        }
    }
    #[test]
    fn device_address_order() {
        let d = Device {
            address: "01:02:03:04:05:06".parse().unwrap(),
            address_type: AddressType::Le(LeAddressType::Random),
        };
        assert_eq!(Device::decode(&d.encode()).unwrap(), d);
    }
    #[test]
    fn only_fresh_valid_manufacturer_identity() {
        let ad = [13, 0xff, 0x53, 5, 0, 0, 0, 0x7e, 5, 0x69, 0x20, 0, 0, 0x81];
        assert_eq!(
            controller_advertisement(&ad).unwrap(),
            Some(AdvertisementMode::Wake)
        );
        for n in 1..ad.len() {
            assert!(controller_advertisement(&ad[..n]).is_err());
        }
        let mut p = vec![0; 14];
        p[6] = 2;
        p[12] = u8::try_from(ad.len()).unwrap();
        p.extend(ad);
        assert_eq!(
            found(&p).unwrap().device.address_type,
            AddressType::Le(LeAddressType::Random)
        );
        p.pop();
        assert!(found(&p).is_err());
    }
    #[test]
    fn connections_validate_count_length_and_address_type() {
        let device = Device {
            address: "01:02:03:04:05:06".parse().unwrap(),
            address_type: AddressType::Le(LeAddressType::Random),
        };
        let mut payload = vec![1, 0];
        payload.extend(device.encode());
        assert_eq!(connections(&payload).unwrap(), [device]);
        assert_eq!(connections(&[0, 0]).unwrap(), []);
        for n in 0..payload.len() {
            assert!(connections(&payload[..n]).is_err());
        }
        payload.push(0);
        assert!(connections(&payload).is_err());
        payload.pop();
        payload[8] = 3;
        assert!(connections(&payload).is_err());
    }
    #[test]
    fn completion_status_layout() {
        let p = [0x23, 0, 10];
        let c = completion(Frame {
            opcode: 2,
            index: 0,
            payload: &p,
        })
        .unwrap()
        .unwrap();
        assert_eq!(c.command, START_DISCOVERY);
        assert_eq!(c.status, BUSY);
    }
}
