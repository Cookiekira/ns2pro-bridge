//! Controller command framing on [`crate::gatt::Characteristic::Command`].

use std::fmt::Write as _;

/// Feature flag for button reports.
pub const FEATURE_BUTTONS: u8 = 0x01;
/// Feature flag for stick reports.
pub const FEATURE_STICKS: u8 = 0x02;
/// Feature flag for accelerometer and gyroscope reports.
pub const FEATURE_IMU: u8 = 0x04;
/// Buttons, sticks and IMU.
pub const DEFAULT_FEATURE_FLAGS: u8 = FEATURE_BUTTONS | FEATURE_STICKS | FEATURE_IMU;

/// Player LED pattern applied when the host has not chosen one.
pub const DEFAULT_LED_PATTERN: u8 = 0x06;

/// Length of a [`rumble_packet`].
pub const RUMBLE_PACKET_LEN: usize = 42;

/// `[cmd, 0x91, seq, sub, 0, len, 0, 0, payload..]`.
///
/// # Panics
///
/// If `payload` is longer than 255 bytes; every caller passes a fixed-size payload.
#[must_use]
#[expect(
    clippy::expect_used,
    reason = "documented precondition, like slice indexing; callers pass at most 17 bytes"
)]
pub fn command(cmd: u8, sub: u8, payload: &[u8]) -> Vec<u8> {
    let len = u8::try_from(payload.len()).expect("command payload fits in one byte");
    let mut out = vec![cmd, 0x91, 0x01, sub, 0x00, len, 0x00, 0x00];
    out.extend_from_slice(payload);
    out
}

/// A controller command response that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResponseError {
    /// Shorter than the 8-byte header; carries the response.
    #[error("short controller response: {}", hex(.0))]
    Short(Vec<u8>),
    /// Answers a different command or subcommand; carries the response.
    #[error("unexpected controller response: {}", hex(.0))]
    Unexpected(Vec<u8>),
    /// An SPI read returned data for another flash address.
    #[error("SPI read for 0x{want:X} returned 0x{got:X}")]
    SpiAddress {
        /// Address that was requested.
        want: u32,
        /// Address the reply carries.
        got: u32,
    },
    /// An SPI reply is shorter than its header or its declared data length.
    #[error("short SPI payload")]
    ShortSpi,
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02X}");
            out
        })
}

/// Validates the response header and returns the payload after it.
///
/// # Errors
///
/// [`ResponseError::Short`] for a response shorter than the header, and
/// [`ResponseError::Unexpected`] when `cmd` or `sub` do not match.
pub fn response_payload(response: &[u8], cmd: u8, sub: u8) -> Result<&[u8], ResponseError> {
    let &[got_cmd, _, _, got_sub, _, _, _, _, ref payload @ ..] = response else {
        return Err(ResponseError::Short(response.to_vec()));
    };
    if got_cmd != cmd || got_sub != sub {
        return Err(ResponseError::Unexpected(response.to_vec()));
    }
    Ok(payload)
}

/// Sets the player LEDs to `mask`.
#[must_use]
pub fn set_player_leds(mask: u8) -> Vec<u8> {
    command(0x09, 0x07, &[mask, 0, 0, 0, 0, 0, 0, 0])
}

/// Restricts later feature changes to `mask`.
#[must_use]
pub fn set_feature_mask(mask: u8) -> Vec<u8> {
    command(0x0C, 0x02, &[mask, 0, 0, 0])
}

/// Enables the reports in `flags`, limited by any [`set_feature_mask`] mask.
#[must_use]
pub fn enable_features(flags: u8) -> Vec<u8> {
    command(0x0C, 0x04, &[flags, 0, 0, 0])
}

/// Reads `size` bytes of SPI flash at `address`.
#[must_use]
pub fn spi_read(address: u32, size: u8) -> Vec<u8> {
    let mut payload = [0u8; 8];
    payload[0] = size;
    payload[1] = 0x7E;
    payload[4..8].copy_from_slice(&address.to_le_bytes());
    command(0x02, 0x04, &payload)
}

/// Extracts the data of a [`spi_read`] response.
///
/// # Errors
///
/// Any [`response_payload`] error, [`ResponseError::ShortSpi`] for a truncated
/// SPI header, and [`ResponseError::SpiAddress`] when another address was read.
pub fn spi_read_data(response: &[u8], address: u32) -> Result<&[u8], ResponseError> {
    let payload = response_payload(response, 0x02, 0x04)?;
    let &[len, _, _, _, a0, a1, a2, a3, ref data @ ..] = payload else {
        return Err(ResponseError::ShortSpi);
    };
    let got = u32::from_le_bytes([a0, a1, a2, a3]);
    if got != address {
        return Err(ResponseError::SpiAddress { want: address, got });
    }
    data.get(..usize::from(len)).ok_or(ResponseError::ShortSpi)
}

/// `[0, left[16], right[16], 0..]` for [`crate::gatt::Characteristic::Vibration`].
#[must_use]
pub fn rumble_packet(left: &[u8; 16], right: &[u8; 16]) -> [u8; RUMBLE_PACKET_LEN] {
    let mut packet = [0u8; RUMBLE_PACKET_LEN];
    for (slot, byte) in packet.iter_mut().skip(1).zip(left.iter().chain(right)) {
        *slot = *byte;
    }
    packet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_commands() {
        assert_eq!(
            set_player_leds(0x09),
            [0x09, 0x91, 1, 0x07, 0, 8, 0, 0, 0x09, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            enable_features(0x07),
            [0x0C, 0x91, 1, 0x04, 0, 4, 0, 0, 0x07, 0, 0, 0]
        );
        assert_eq!(
            spi_read(0x13080, 0x40)[8..],
            [0x40, 0x7E, 0, 0, 0x80, 0x30, 0x01, 0x00]
        );
    }

    #[test]
    fn validates_responses() {
        assert_eq!(
            response_payload(&[0x0C, 0, 0, 0x04, 0, 0, 0, 0, 9], 0x0C, 0x04),
            Ok(&[9u8][..])
        );
        assert!(matches!(
            response_payload(&[0x0C, 0, 0], 0x0C, 0x04),
            Err(ResponseError::Short(_))
        ));
        assert!(matches!(
            response_payload(&[0x0D, 0, 0, 0x04, 0, 0, 0, 0], 0x0C, 0x04),
            Err(ResponseError::Unexpected(_))
        ));
    }

    #[test]
    fn reads_spi_payload() {
        let mut response = vec![0x02, 0, 0, 0x04, 0, 0, 0, 0, 3, 0, 0, 0];
        response.extend_from_slice(&0x13080u32.to_le_bytes());
        response.extend_from_slice(&[7, 8, 9]);
        assert_eq!(spi_read_data(&response, 0x13080), Ok(&[7u8, 8, 9][..]));
        assert_eq!(
            spi_read_data(&response, 0x130C0),
            Err(ResponseError::SpiAddress {
                want: 0x130C0,
                got: 0x13080
            })
        );
        assert_eq!(
            spi_read_data(&response[..14], 0x13080),
            Err(ResponseError::ShortSpi)
        );
    }

    #[test]
    fn builds_rumble_packet() {
        let left: [u8; 16] = std::array::from_fn(|i| u8::try_from(i + 1).unwrap());
        let right: [u8; 16] = std::array::from_fn(|i| u8::try_from(i + 17).unwrap());
        let packet = rumble_packet(&left, &right);
        assert_eq!(packet[0], 0);
        assert_eq!(packet[1..17], left);
        assert_eq!(packet[17..33], right);
        assert!(packet[33..].iter().all(|&b| b == 0));
    }
}
