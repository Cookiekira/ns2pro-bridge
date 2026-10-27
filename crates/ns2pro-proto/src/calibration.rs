//! Stick calibration stored in controller SPI flash.

use crate::input::unpack_stick12;

/// Factory calibration block for the left stick.
pub const PRIMARY_FACTORY_ADDRESS: u32 = 0x13080;
/// Factory calibration block for the right stick.
pub const SECONDARY_FACTORY_ADDRESS: u32 = 0x130C0;
/// User calibration block for both sticks.
pub const USER_ADDRESS: u32 = 0x1F_C040;
/// Bytes read from each calibration block.
pub const BLOCK_SIZE: u8 = 0x40;

const FACTORY_OFFSET: usize = 0x28;
const USER_MAGIC: [u8; 2] = [0xA2, 0xB2];
const USER_PRIMARY_OFFSET: usize = 0x00;
const USER_SECONDARY_OFFSET: usize = 0x20;
const USER_BLOCK_MIN_LEN: usize = 0x2B;

/// Center and per-direction spans of one stick, in raw 12-bit units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StickCalibration {
    /// Raw X at rest.
    pub center_x: i32,
    /// Raw Y at rest.
    pub center_y: i32,
    /// Raw X span from rest to full deflection toward larger values.
    pub max_x: i32,
    /// Raw Y span from rest to full deflection toward larger values.
    pub max_y: i32,
    /// Raw X span from rest to full deflection toward smaller values.
    pub min_x: i32,
    /// Raw Y span from rest to full deflection toward smaller values.
    pub min_y: i32,
}

impl StickCalibration {
    /// Decodes nine bytes: center, positive span, negative span.
    #[must_use]
    pub fn decode(raw: &[u8]) -> Option<Self> {
        let (center, rest) = raw.split_first_chunk::<3>()?;
        let (positive, rest) = rest.split_first_chunk::<3>()?;
        let negative = rest.first_chunk::<3>()?;
        let pair = |bytes: &[u8; 3]| {
            let (x, y) = unpack_stick12(*bytes);
            (i32::from(x), i32::from(y))
        };
        let ((center_x, center_y), (max_x, max_y), (min_x, min_y)) =
            (pair(center), pair(positive), pair(negative));
        Some(Self {
            center_x,
            center_y,
            max_x,
            max_y,
            min_x,
            min_y,
        })
    }
}

/// Calibration for both sticks; `None` leaves that stick uncalibrated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Calibration {
    /// Left stick.
    pub primary: Option<StickCalibration>,
    /// Right stick.
    pub secondary: Option<StickCalibration>,
}

impl Calibration {
    /// Combines factory blocks with an optional user block; user data wins when its magic is present.
    #[must_use]
    pub fn from_blocks(
        primary_factory: &[u8],
        secondary_factory: &[u8],
        user: Option<&[u8]>,
    ) -> Option<Self> {
        let mut calibration = Self {
            primary: Some(StickCalibration::decode(
                primary_factory.get(FACTORY_OFFSET..)?,
            )?),
            secondary: Some(StickCalibration::decode(
                secondary_factory.get(FACTORY_OFFSET..)?,
            )?),
        };
        if let Some(user) = user.filter(|u| u.len() >= USER_BLOCK_MIN_LEN) {
            let user_stick = |offset: usize| {
                let (magic, stick) = user.get(offset..)?.split_first_chunk::<2>()?;
                (*magic == USER_MAGIC)
                    .then(|| StickCalibration::decode(stick))
                    .flatten()
            };
            if let Some(stick) = user_stick(USER_PRIMARY_OFFSET) {
                calibration.primary = Some(stick);
            }
            if let Some(stick) = user_stick(USER_SECONDARY_OFFSET) {
                calibration.secondary = Some(stick);
            }
        }
        Some(calibration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::pack_stick12;

    fn encoded(center: u16, max: u16, min: u16) -> [u8; 9] {
        let mut out = [0u8; 9];
        out[0..3].copy_from_slice(&pack_stick12(center, center));
        out[3..6].copy_from_slice(&pack_stick12(max, max));
        out[6..9].copy_from_slice(&pack_stick12(min, min));
        out
    }

    fn factory(center: u16) -> Vec<u8> {
        let mut block = vec![0u8; 0x40];
        block[FACTORY_OFFSET..FACTORY_OFFSET + 9].copy_from_slice(&encoded(center, 1000, 900));
        block
    }

    #[test]
    fn decodes_factory_blocks() {
        let calibration = Calibration::from_blocks(&factory(2000), &factory(2100), None).unwrap();
        assert_eq!(calibration.primary.unwrap().center_x, 2000);
        assert_eq!(calibration.secondary.unwrap().center_y, 2100);
        assert_eq!(calibration.primary.unwrap().min_x, 900);
    }

    #[test]
    fn user_block_overrides_when_magic_present() {
        let mut user = vec![0u8; 0x40];
        user[0x20..0x22].copy_from_slice(&USER_MAGIC);
        user[0x22..0x2B].copy_from_slice(&encoded(1800, 1100, 1200));
        let calibration =
            Calibration::from_blocks(&factory(2000), &factory(2100), Some(&user)).unwrap();
        assert_eq!(calibration.primary.unwrap().center_x, 2000);
        assert_eq!(calibration.secondary.unwrap().center_x, 1800);
    }

    #[test]
    fn short_factory_block_is_rejected() {
        assert_eq!(
            Calibration::from_blocks(&[0; 0x30], &factory(2000), None),
            None
        );
    }
}
