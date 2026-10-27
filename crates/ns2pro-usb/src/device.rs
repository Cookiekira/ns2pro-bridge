//! Complete `NS2Pro` USB device model, ported from VIIPER's GPL implementation.
use crate::descriptors::{
    CONFIG_DESC_FULL, DEVICE_DESC, HID_DESC, HID_REPORT_DESC, MSOS_COMPAT_ID, MSOS_EXT_PROPS,
    STRING_0, STRING_1, STRING_2, STRING_3, STRING_4, STRING_5, STRING_6, STRING_EE,
};
use ns2pro_proto::{
    InputState,
    input::{COMMON_BUTTON_LAYOUT, encode_buttons, pack_stick12},
};
use std::collections::VecDeque;
/// Controller input stamped for the USB reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputSample {
    /// Decoded input.
    pub state: InputState,
    /// Wrapping microsecond timestamp of the sample; the bridge uses time since the
    /// connection's input subscription started, not a controller clock.
    pub motion_timestamp_us: u32,
    /// Increments with every new input; identifies it in latency statistics.
    pub seq: u64,
}
/// A host request for the physical controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// Rumble for both actuators.
    Rumble {
        /// Left actuator rumble data.
        left: [u8; 16],
        /// Right actuator rumble data.
        right: [u8; 16],
    },
    /// Player LED mask.
    PlayerLeds(u8),
}
/// The emulated controller's USB behaviour: reports, features and bulk commands.
#[derive(Debug)]
pub struct Ns2ProDevice {
    report: u8,
    features: u8,
    mask: u8,
    counter: u32,
    pro_counter: u8,
    bulk: VecDeque<Vec<u8>>,
    configuration: u8,
    identity: Option<[u8; 64]>,
}
impl Default for Ns2ProDevice {
    fn default() -> Self {
        Self {
            report: 9,
            features: 3,
            mask: 0,
            counter: 0,
            pro_counter: 0,
            bulk: VecDeque::new(),
            configuration: 1,
            identity: None,
        }
    }
}
impl Ns2ProDevice {
    /// Serves `identity` as SPI block 0x13000; `None` uses [`flash_block`].
    pub const fn set_identity(&mut self, identity: Option<[u8; 64]>) {
        self.identity = identity;
    }
    fn flash_block(&self, address: u32) -> [u8; 64] {
        if address == 0x13000 {
            self.identity.unwrap_or_else(|| flash_block(address))
        } else {
            flash_block(address)
        }
    }
    /// The next input report, in the format (0x05 or 0x09) the host selected.
    pub fn interrupt_in(&mut self, sample: &InputSample) -> [u8; 64] {
        self.report_for(self.report, sample)
    }
    fn report_for(&mut self, id: u8, sample: &InputSample) -> [u8; 64] {
        let s = &sample.state;
        let mut b = [0; 64];
        if id == 5 {
            b[0] = 5;
            self.counter = self.counter.wrapping_add(1);
            b[1..5].copy_from_slice(&self.counter.to_le_bytes());
            encode_buttons(s.buttons, &COMMON_BUTTON_LAYOUT, &mut b[5..9]);
            b[11..14].copy_from_slice(&pack_stick12(s.left_stick.x, s.left_stick.y));
            b[14..17].copy_from_slice(&pack_stick12(s.right_stick.x, s.right_stick.y));
            b[0x20..0x22].copy_from_slice(&3800u16.to_le_bytes());
            b[0x22] = if s.charging { 0x34 } else { 0x20 };
            b[0x2a] = 1;
            if self.features & 4 != 0 {
                b[0x2b..0x2f].copy_from_slice(&sample.motion_timestamp_us.to_le_bytes());
                for (field, v) in b[0x31..0x3D]
                    .as_chunks_mut::<2>()
                    .0
                    .iter_mut()
                    .zip(s.accel.iter().chain(&s.gyro))
                {
                    *field = v.to_le_bytes();
                }
            }
        } else {
            b[0] = 9;
            self.pro_counter = self.pro_counter.wrapping_add(1);
            b[1] = self.pro_counter;
            b[2] = (s.battery_level.min(9) << 2)
                | u8::from(s.external_power)
                | (u8::from(s.charging) << 1);
            // Canonical layout is ordered exactly as the Pro report's 21 button bits
            // (headset is absent).
            let [low, middle, high, _] = (s.buttons.0 & 0x1F_FFFF).to_le_bytes();
            b[3] = low;
            b[4] = middle;
            b[5] = high;
            b[6..9].copy_from_slice(&pack_stick12(s.left_stick.x, s.left_stick.y));
            b[9..12].copy_from_slice(&pack_stick12(s.right_stick.x, s.right_stick.y));
            b[12] = if self.features & 0x20 != 0 {
                0x38
            } else {
                0x30
            };
        }
        b
    }
    /// Decodes a rumble report from the host.
    pub fn interrupt_out(&mut self, data: &[u8]) -> Option<Output> {
        let p = match data.split_first() {
            Some((&2, report)) => report,
            _ if data.len() == 32 => data,
            _ => return None,
        };
        let (left, rest) = p.split_first_chunk::<16>()?;
        let right = rest.first_chunk::<16>()?;
        Some(Output::Rumble {
            left: *left,
            right: *right,
        })
    }
    /// Handles a host command, queues its bulk response, and returns any request
    /// for the physical controller.
    pub fn bulk_out(&mut self, out: &[u8]) -> Option<Output> {
        let &[cmd, _, seq, sub, _, _, _, _, ..] = out else {
            return None;
        };
        let flags = out.get(8).copied().unwrap_or(0);
        let mut r = vec![cmd, 1, seq, sub, 0x10, 0x78, 0, 0];
        let mut event = None;
        match cmd {
            2 if sub == 1 => {
                if let Some(&[a0, a1, a2, a3]) = out.get(12..16) {
                    let a = u32::from_le_bytes([a0, a1, a2, a3]);
                    r.extend([0x40, 0, 0, 0]);
                    r.extend(a.to_le_bytes());
                    r.extend(self.flash_block(a));
                }
            }
            3 => match sub {
                3 | 0x0d => r.extend([1, 0, 0, 0]),
                0x0a if [5, 9].contains(&flags) => self.report = flags,
                _ => {}
            },
            9 if sub == 7 => {
                if out.len() > 8 {
                    event = Some(Output::PlayerLeds(flags));
                }
            }
            0x0c => {
                let masked = if self.mask == 0 {
                    flags
                } else {
                    flags & self.mask
                };
                match sub {
                    1 => {
                        let when = |flag, value| if flags & flag != 0 { value } else { 0 };
                        r.extend([
                            0,
                            0,
                            0,
                            0,
                            when(1, 7),
                            when(2, 7),
                            when(4, 1),
                            0,
                            when(0x10, 3),
                            when(0x20, 3),
                            0,
                            0,
                        ]);
                    }
                    2 => self.mask = flags,
                    3 => {
                        self.mask = 0;
                        self.features = 0;
                    }
                    4 => self.features |= masked,
                    5 => self.features &= !masked,
                    _ => {}
                }
                if sub != 1 {
                    r.extend([0; 4]);
                }
            }
            _ => {}
        }
        self.bulk.push_back(r);
        event
    }
    /// Takes all remaining bytes of the next queued bulk response.
    pub fn pop_bulk_in(&mut self) -> Option<Vec<u8>> {
        self.read_bulk_in(usize::MAX)
    }
    /// Complete at most one host transfer while retaining an unread response tail.
    pub fn read_bulk_in(&mut self, max_length: usize) -> Option<Vec<u8>> {
        if max_length == 0 {
            return Some(Vec::new());
        }
        let mut response = self.bulk.pop_front()?;
        if response.len() > max_length {
            let remaining = response.split_off(max_length);
            self.bulk.push_front(remaining);
        }
        Some(response)
    }
    /// Handles a control transfer and returns the IN data plus any host output.
    ///
    /// # Errors
    ///
    /// A negative errno to report as the URB status: `-EPIPE` (STALL) for
    /// unsupported requests. Unknown descriptors are deliberately rejected.
    pub fn control(
        &mut self,
        setup: [u8; 8],
        out: &[u8],
        sample: &InputSample,
    ) -> Result<(Vec<u8>, Option<Output>), i32> {
        let [ty, req, vl, vh, il, ih, ll, lh] = setup;
        let value = u16::from_le_bytes([vl, vh]);
        let index = u16::from_le_bytes([il, ih]);
        let len = usize::from(u16::from_le_bytes([ll, lh]));
        let mut event = None;
        let mut data = match (ty, req) {
            (0x80..=0x82, 0) => vec![0; 2],
            (0, 9) => {
                self.configuration = vl;
                vec![]
            }
            (0x80, 8) => vec![self.configuration],
            (0, 5) | (0x00..=0x02, 1 | 3) | (0x01, 11) | (0x21, 10) => vec![],
            (0x81, 10) => vec![0],
            (0x80 | 0x81, 6) => match (vh, vl, index) {
                (1, 0, _) => DEVICE_DESC.to_vec(),
                (2, 0, _) => CONFIG_DESC_FULL.to_vec(),
                (0x21, 0, 0) => HID_DESC.to_vec(),
                (0x22, 0, 0) => HID_REPORT_DESC.to_vec(),
                (3, 0, _) => STRING_0.to_vec(),
                (3, 1, _) => STRING_1.to_vec(),
                (3, 2, _) => STRING_2.to_vec(),
                (3, 3, _) => STRING_3.to_vec(),
                (3, 4, _) => STRING_4.to_vec(),
                (3, 5, _) => STRING_5.to_vec(),
                (3, 6, _) => STRING_6.to_vec(),
                (3, 0xee, _) => STRING_EE.to_vec(),
                _ => return Err(-32),
            },
            (0xc0, 0x20) if index == 4 => MSOS_COMPAT_ID.to_vec(),
            (0xc1, 0x20) if index == 5 || value == 5 => MSOS_EXT_PROPS.to_vec(),
            (0xa1, 1) if vh == 1 && [0, 5, 9].contains(&vl) => self
                .report_for(if vl == 0 { self.report } else { vl }, sample)
                .to_vec(),
            (0x21, 9) if vh == 2 && vl == 2 => {
                event = self.interrupt_out(out);
                vec![]
            }
            (0xa1, 1) => return Err(-32),
            // Audio class compatibility requests from the original USB model.
            _ if ty & 0x60 == 0x20
                && [1, 2].contains(&(ty & 0x1f))
                && [0x81, 0x82, 0x83, 0x84].contains(&req) =>
            {
                vec![0; len]
            }
            _ if ty & 0x60 == 0x20 && [1, 2].contains(&(ty & 0x1f)) && req == 1 => vec![],
            _ => return Err(-32),
        };
        data.truncate(len);
        Ok((data, event))
    }
}
/// Synthetic SPI flash: a bridge identity at 0x13000, centered factory stick
/// calibration at 0x13080 and 0x130C0, zeros elsewhere.
#[must_use]
pub fn flash_block(address: u32) -> [u8; 64] {
    let mut b = [0; 64];
    match address {
        0x13000 => b[2..18].copy_from_slice(b"NS2PRO-BRIDGE-00"),
        0x13080 | 0x130c0 => {
            b[40..43].copy_from_slice(&pack_stick12(2048, 2048));
            b[43..46].copy_from_slice(&pack_stick12(2047, 2047));
            b[46..49].copy_from_slice(&pack_stick12(2048, 2048));
        }
        _ => {}
    }
    b
}
