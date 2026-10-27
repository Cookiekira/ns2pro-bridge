//! Recognises Switch 2 Pro Controller BLE advertisements.

const NINTENDO_COMPANY_ID: u16 = 0x0553;
const NINTENDO_VID: u16 = 0x057E;
const NS2PRO_PID: u16 = 0x2069;

/// Advertising mode recognised in a controller advertisement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvertisementMode {
    /// Pairing or reconnect advertising.
    StandardReconnect,
    /// Wake advertising after a button press on a paired controller.
    Wake,
}

/// Parses one manufacturer-specific data entry.
#[must_use]
pub fn parse(company_id: u16, data: &[u8]) -> Option<AdvertisementMode> {
    // VID at 3, PID at 5 (little endian), mode at 9; trailing bytes are ignored.
    let &[_, _, _, v0, v1, p0, p1, _, _, mode] = data.first_chunk::<10>()?;
    if company_id != NINTENDO_COMPANY_ID
        || u16::from_le_bytes([v0, v1]) != NINTENDO_VID
        || u16::from_le_bytes([p0, p1]) != NS2PRO_PID
    {
        return None;
    }
    match mode {
        0x00 => Some(AdvertisementMode::StandardReconnect),
        0x81 => Some(AdvertisementMode::Wake),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(mode: u8, vid: u16, pid: u16) -> [u8; 10] {
        let mut out = [0u8; 10];
        out[3..5].copy_from_slice(&vid.to_le_bytes());
        out[5..7].copy_from_slice(&pid.to_le_bytes());
        out[9] = mode;
        out
    }

    #[test]
    fn accepts_supported_modes() {
        assert_eq!(
            parse(0x0553, &data(0x00, 0x057E, 0x2069)),
            Some(AdvertisementMode::StandardReconnect)
        );
        assert_eq!(
            parse(0x0553, &data(0x81, 0x057E, 0x2069)),
            Some(AdvertisementMode::Wake)
        );
    }

    #[test]
    fn rejects_wrong_identity_or_mode() {
        assert_eq!(parse(0x0000, &data(0x00, 0x057E, 0x2069)), None);
        assert_eq!(parse(0x0553, &data(0x00, 0x0000, 0x2069)), None);
        assert_eq!(parse(0x0553, &data(0x00, 0x057E, 0x0000)), None);
        assert_eq!(parse(0x0553, &data(0x42, 0x057E, 0x2069)), None);
        assert_eq!(parse(0x0553, &data(0x00, 0x057E, 0x2069)[..9]), None);
    }
}
