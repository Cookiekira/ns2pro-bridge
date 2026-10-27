//! Controller input state and the BLE common-report decoder.

use std::ops::BitOr;

use crate::calibration::{Calibration, StickCalibration};

/// Button bit set in the bridge's canonical layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Buttons(pub u32);

impl Buttons {
    /// No buttons.
    pub const NONE: Self = Self(0);
    /// B face button.
    pub const B: Self = Self(1 << 0);
    /// A face button.
    pub const A: Self = Self(1 << 1);
    /// Y face button.
    pub const Y: Self = Self(1 << 2);
    /// X face button.
    pub const X: Self = Self(1 << 3);
    /// R shoulder button.
    pub const R: Self = Self(1 << 4);
    /// ZR trigger.
    pub const ZR: Self = Self(1 << 5);
    /// Plus button.
    pub const PLUS: Self = Self(1 << 6);
    /// Right stick click.
    pub const RIGHT_STICK: Self = Self(1 << 7);
    /// D-pad down.
    pub const DOWN: Self = Self(1 << 8);
    /// D-pad right.
    pub const RIGHT: Self = Self(1 << 9);
    /// D-pad left.
    pub const LEFT: Self = Self(1 << 10);
    /// D-pad up.
    pub const UP: Self = Self(1 << 11);
    /// L shoulder button.
    pub const L: Self = Self(1 << 12);
    /// ZL trigger.
    pub const ZL: Self = Self(1 << 13);
    /// Minus button.
    pub const MINUS: Self = Self(1 << 14);
    /// Left stick click.
    pub const LEFT_STICK: Self = Self(1 << 15);
    /// Home button.
    pub const HOME: Self = Self(1 << 16);
    /// Capture button.
    pub const CAPTURE: Self = Self(1 << 17);
    /// GR rear grip button.
    pub const GR: Self = Self(1 << 18);
    /// GL rear grip button.
    pub const GL: Self = Self(1 << 19);
    /// C button.
    pub const C: Self = Self(1 << 20);
    /// Headset connected; reported as a button bit.
    pub const HEADSET: Self = Self(1 << 21);

    /// Whether every button in `other` is pressed.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Buttons {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// Where a button lives in a packed report: `(button, byte index, mask)`.
pub type ButtonBit = (Buttons, usize, u8);

/// Four-byte button layout shared by the BLE common report and USB report 0x05.
pub const COMMON_BUTTON_LAYOUT: [ButtonBit; 22] = [
    (Buttons::Y, 0, 0x01),
    (Buttons::X, 0, 0x02),
    (Buttons::B, 0, 0x04),
    (Buttons::A, 0, 0x08),
    (Buttons::R, 0, 0x40),
    (Buttons::ZR, 0, 0x80),
    (Buttons::MINUS, 1, 0x01),
    (Buttons::PLUS, 1, 0x02),
    (Buttons::RIGHT_STICK, 1, 0x04),
    (Buttons::LEFT_STICK, 1, 0x08),
    (Buttons::HOME, 1, 0x10),
    (Buttons::CAPTURE, 1, 0x20),
    (Buttons::C, 1, 0x40),
    (Buttons::DOWN, 2, 0x01),
    (Buttons::UP, 2, 0x02),
    (Buttons::RIGHT, 2, 0x04),
    (Buttons::LEFT, 2, 0x08),
    (Buttons::L, 2, 0x40),
    (Buttons::ZL, 2, 0x80),
    (Buttons::GR, 3, 0x01),
    (Buttons::GL, 3, 0x02),
    (Buttons::HEADSET, 3, 0x10),
];

/// Reads `layout`'s buttons from `raw`; bytes beyond `raw` read as released.
#[must_use]
pub fn decode_buttons(raw: &[u8], layout: &[ButtonBit]) -> Buttons {
    layout
        .iter()
        .filter(|(_, index, mask)| raw.get(*index).is_some_and(|byte| byte & mask != 0))
        .fold(Buttons::NONE, |acc, (button, _, _)| acc | *button)
}

/// Sets the layout bits of `buttons` in `out`; like [`decode_buttons`], bits
/// beyond `out` are ignored.
pub fn encode_buttons(buttons: Buttons, layout: &[ButtonBit], out: &mut [u8]) {
    for (button, index, mask) in layout {
        if buttons.contains(*button)
            && let Some(byte) = out.get_mut(*index)
        {
            *byte |= mask;
        }
    }
}

/// Smallest 12-bit stick value.
pub const STICK_MIN: u16 = 0;
/// 12-bit stick value at rest.
pub const STICK_CENTER: u16 = 0x0800;
/// Largest 12-bit stick value.
pub const STICK_MAX: u16 = 0x0FFF;
/// Battery level when full.
pub const BATTERY_MAX: u8 = 9;

/// One stick position, 12 bits per axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stick {
    /// Horizontal axis, `STICK_MIN..=STICK_MAX`.
    pub x: u16,
    /// Vertical axis, `STICK_MIN..=STICK_MAX`.
    pub y: u16,
}

impl Stick {
    /// Both axes at rest.
    pub const CENTER: Self = Self {
        x: STICK_CENTER,
        y: STICK_CENTER,
    };
}

/// Unpacks two 12-bit values from three bytes.
#[must_use]
pub fn unpack_stick12(raw: [u8; 3]) -> (u16, u16) {
    let x = u16::from(raw[0]) | (u16::from(raw[1] & 0x0F) << 8);
    let y = (u16::from(raw[1]) >> 4) | (u16::from(raw[2]) << 4);
    (x, y)
}

/// Packs two 12-bit values (clamped) into three bytes.
#[must_use]
pub fn pack_stick12(x: u16, y: u16) -> [u8; 3] {
    let x = x.min(STICK_MAX);
    let y = y.min(STICK_MAX);
    // y <= 0x0FFF, so `y << 4` keeps all 12 bits: low nibble in byte 0, rest in byte 1.
    let [x_lo, x_hi] = x.to_le_bytes();
    let [y_lo, y_hi] = (y << 4).to_le_bytes();
    [x_lo, (x_hi & 0x0F) | y_lo, y_hi]
}

/// Decoded controller input, as forwarded to the virtual USB device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputState {
    /// Pressed buttons.
    pub buttons: Buttons,
    /// Calibrated left stick.
    pub left_stick: Stick,
    /// Calibrated right stick.
    pub right_stick: Stick,
    /// Accelerometer X, Y, Z.
    pub accel: [i16; 3],
    /// Gyroscope X, Y, Z.
    pub gyro: [i16; 3],
    /// 0..=9.
    pub battery_level: u8,
    /// The battery is charging.
    pub charging: bool,
    /// Reported to the host as external power; [`parse_common_report`] always sets it.
    pub external_power: bool,
}

impl Default for InputState {
    /// Neutral sticks, no buttons, full battery on external power.
    fn default() -> Self {
        Self {
            buttons: Buttons::NONE,
            left_stick: Stick::CENTER,
            right_stick: Stick::CENTER,
            accel: [0; 3],
            gyro: [0; 3],
            battery_level: BATTERY_MAX,
            charging: false,
            external_power: true,
        }
    }
}

/// A common report shorter than its 16-byte button and stick block; carries the length.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("short common report: {0} bytes")]
pub struct ShortReport(pub usize);

/// Decodes a BLE common input report (notified on [`crate::gatt::Characteristic::Input`]).
///
/// # Errors
///
/// [`ShortReport`] when `data` is shorter than the 16-byte button and stick block.
pub fn parse_common_report(
    data: &[u8],
    calibration: &Calibration,
) -> Result<InputState, ShortReport> {
    let Some(&[_, _, _, _, b0, b1, b2, b3, _, _, l0, l1, l2, r0, r1, r2]) =
        data.first_chunk::<0x10>()
    else {
        return Err(ShortReport(data.len()));
    };
    let stick = |raw: [u8; 3], cal: Option<&StickCalibration>| {
        let (x, y) = unpack_stick12(raw);
        normalize_stick(x, y, cal)
    };

    let mut state = InputState {
        buttons: decode_buttons(&[b0, b1, b2, b3], &COMMON_BUTTON_LAYOUT),
        left_stick: stick([l0, l1, l2], calibration.primary.as_ref()),
        right_stick: stick([r0, r1, r2], calibration.secondary.as_ref()),
        ..InputState::default()
    };

    if let Some(&[low, high]) = data.get(0x1F..0x21) {
        let millivolts = i32::from(u16::from_le_bytes([low, high]));
        if millivolts > 0 {
            let level = ((millivolts - 3200) * 9 / 800).clamp(0, i32::from(BATTERY_MAX));
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::as_conversions,
                reason = "clamped to 0..=BATTERY_MAX"
            )]
            let level = level as u8;
            state.battery_level = level;
            state.external_power = true;
        }
    }
    if let Some(&charge) = data.get(0x21) {
        state.charging = charge == 0x34;
    }
    if let Some(&[ax0, ax1, ay0, ay1, az0, az1, gx0, gx1, gy0, gy1, gz0, gz1]) =
        data.get(0x30..).and_then(<[u8]>::first_chunk::<12>)
    {
        let i16_le = |low, high| i16::from_le_bytes([low, high]);
        state.accel = [i16_le(ax0, ax1), i16_le(ay0, ay1), i16_le(az0, az1)];
        state.gyro = [i16_le(gx0, gx1), i16_le(gy0, gy1), i16_le(gz0, gz1)];
    }
    Ok(state)
}

fn normalize_stick(x: u16, y: u16, calibration: Option<&StickCalibration>) -> Stick {
    calibration.map_or_else(
        || Stick {
            x: x.min(STICK_MAX),
            y: y.min(STICK_MAX),
        },
        |c| Stick {
            x: normalize_axis(i32::from(x), c.center_x, c.max_x, c.min_x),
            y: normalize_axis(i32::from(y), c.center_y, c.max_y, c.min_y),
        },
    )
}

/// Maps a raw axis onto `0..=0x0FFF` around `0x0800` using per-direction spans.
fn normalize_axis(value: i32, center: i32, positive_span: i32, negative_span: i32) -> u16 {
    let (min, mid, max) = (
        i32::from(STICK_MIN),
        i32::from(STICK_CENTER),
        i32::from(STICK_MAX),
    );
    let out = if value >= center {
        if positive_span <= 0 {
            mid
        } else {
            mid + ((value - center) * (max - mid) + positive_span / 2) / positive_span
        }
    } else if negative_span <= 0 {
        mid
    } else {
        mid - ((center - value) * (mid - min) + negative_span / 2) / negative_span
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::as_conversions,
        reason = "clamped to STICK_MIN..=STICK_MAX"
    )]
    let out = out.clamp(min, max) as u16;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Vec<u8> {
        let mut r = vec![0u8; 0x3C];
        r[0x04] = 0x08; // A
        r[0x0A..0x0D].copy_from_slice(&pack_stick12(0x0800, 0x0800));
        r[0x0D..0x10].copy_from_slice(&pack_stick12(0x0800, 0x0800));
        r
    }

    #[test]
    fn stick12_round_trip() {
        for (x, y) in [(0, 0), (0x123, 0xABC), (0xFFF, 0x001), (0x800, 0x800)] {
            assert_eq!(unpack_stick12(pack_stick12(x, y)), (x, y));
        }
        assert_eq!(
            unpack_stick12(pack_stick12(0xFFFF, 0x1000)),
            (0x0FFF, 0x0FFF)
        );
    }

    #[test]
    fn decodes_buttons_and_neutral_sticks() {
        let state = parse_common_report(&report(), &Calibration::default()).unwrap();
        assert!(state.buttons.contains(Buttons::A));
        assert_eq!(state.buttons, Buttons::A);
        assert_eq!(state.left_stick, Stick::CENTER);
        assert_eq!(state.battery_level, BATTERY_MAX);
    }

    #[test]
    fn decodes_power_and_motion() {
        let mut r = report();
        r[0x1F..0x21].copy_from_slice(&3600u16.to_le_bytes());
        r[0x21] = 0x34;
        r[0x30..0x32].copy_from_slice(&(-5i16).to_le_bytes());
        r[0x3A..0x3C].copy_from_slice(&7i16.to_le_bytes());
        let state = parse_common_report(&r, &Calibration::default()).unwrap();
        assert_eq!(state.battery_level, 4);
        assert!(state.charging);
        assert_eq!(state.accel[0], -5);
        assert_eq!(state.gyro[2], 7);
    }

    #[test]
    fn battery_level_clamps_at_both_ends() {
        for (millivolts, level) in [
            (1, 0),
            (3200, 0),
            (4000, BATTERY_MAX),
            (25_955, BATTERY_MAX),
            (25_956, BATTERY_MAX),
            (u16::MAX, BATTERY_MAX),
        ] {
            let mut r = report();
            r.resize(0x21, 0);
            r[0x1F..0x21].copy_from_slice(&millivolts.to_le_bytes());
            let state = parse_common_report(&r, &Calibration::default()).unwrap();
            assert_eq!(state.battery_level, level, "{millivolts} mV");
        }
    }

    #[test]
    fn rejects_short_reports() {
        assert_eq!(
            parse_common_report(&[0; 15], &Calibration::default()),
            Err(ShortReport(15))
        );
    }

    #[test]
    fn calibration_scales_full_deflection() {
        let cal = StickCalibration {
            center_x: 2000,
            center_y: 2000,
            max_x: 1500,
            max_y: 1500,
            min_x: 1500,
            min_y: 1500,
        };
        let calibration = Calibration {
            primary: Some(cal),
            secondary: None,
        };
        let mut r = report();
        r[0x0A..0x0D].copy_from_slice(&pack_stick12(3500, 500));
        let state = parse_common_report(&r, &calibration).unwrap();
        assert_eq!(
            state.left_stick,
            Stick {
                x: STICK_MAX,
                y: STICK_MIN
            }
        );
    }

    #[test]
    fn button_layout_round_trips() {
        let all = COMMON_BUTTON_LAYOUT
            .iter()
            .fold(Buttons::NONE, |acc, (b, _, _)| acc | *b);
        let mut raw = [0u8; 4];
        encode_buttons(all, &COMMON_BUTTON_LAYOUT, &mut raw);
        assert_eq!(decode_buttons(&raw, &COMMON_BUTTON_LAYOUT), all);
    }
}
