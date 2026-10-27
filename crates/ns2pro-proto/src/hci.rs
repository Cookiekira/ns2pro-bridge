//! HCI LE connection wire format (little endian, independent of I/O).
use std::io;

/// Parameters for an LE Connection Update request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionParameters {
    /// Minimum connection interval, in 1.25 ms units.
    pub min: u16,
    /// Maximum connection interval, in 1.25 ms units.
    pub max: u16,
    /// Peripheral latency, in connection events.
    pub latency: u16,
    /// Supervision timeout, in 10 ms units.
    pub timeout: u16,
}
impl ConnectionParameters {
    /// 7.5 ms interval, no peripheral latency, 2 s supervision timeout.
    pub const LOW_LATENCY: Self = Self {
        min: 6,
        max: 6,
        latency: 0,
        timeout: 200,
    };
    /// The minimum interval in milliseconds.
    #[must_use]
    pub fn interval_ms(self) -> f64 {
        f64::from(self.min) * 1.25
    }

    /// Encode an HCI LE Connection Update with no connection-event length hint.
    #[must_use]
    pub fn encode_update(self, handle: u16) -> [u8; 18] {
        let fields = [handle, self.min, self.max, self.latency, self.timeout, 0, 0];
        let bytes = [1, 0x13, 0x20, 14]
            .into_iter()
            .chain(fields.into_iter().flat_map(u16::to_le_bytes));
        let mut p = [0; 18];
        for (slot, byte) in p.iter_mut().zip(bytes) {
            *slot = byte;
        }
        p
    }
}

/// Connection parameters reported by the controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parameters {
    /// Connection interval, in 1.25 ms units (6..=3200).
    pub interval: u16,
    /// Peripheral latency, in connection events (0..=499).
    pub latency: u16,
    /// Supervision timeout, in 10 ms units (10..=3200).
    pub supervision: u16,
}
impl Parameters {
    fn parse(p: &[u8]) -> io::Result<Self> {
        let &[i0, i1, l0, l1, s0, s1] = p else {
            return Err(io::Error::other("short LE interval parameters"));
        };
        let result = Self {
            interval: u16::from_le_bytes([i0, i1]),
            latency: u16::from_le_bytes([l0, l1]),
            supervision: u16::from_le_bytes([s0, s1]),
        };
        if !(6..=3200).contains(&result.interval)
            || result.latency > 499
            || !(10..=3200).contains(&result.supervision)
        {
            return Err(io::Error::other("invalid LE interval parameters"));
        }
        Ok(result)
    }
}

/// The HCI events the bridge observes.
#[derive(Debug)]
pub enum Event<'a> {
    /// LE Connection Complete (legacy, enhanced or enhanced v2) with success status.
    Connected {
        /// Connection handle.
        handle: u16,
        /// 0 when the local controller is central, 1 when peripheral.
        role: u8,
        /// HCI peer address type.
        address_type: u8,
        /// Peer address, least significant byte first.
        address: &'a [u8],
        /// Initial connection parameters.
        parameters: Parameters,
    },
    /// Disconnection Complete with success status, for this handle.
    Disconnected(u16),
    /// LE Connection Update Complete: handle, status, and the new parameters on success.
    Updated(u16, u8, Option<Parameters>),
    /// Command Status or Command Complete for LE Connection Update, carrying its status.
    CommandStatus(u8),
    /// Any other event, or a connection event with a failure status.
    Other,
}
/// Parses an HCI event packet, including its `0x04` packet indicator.
///
/// # Errors
///
/// When the framing length is inconsistent or an LE meta event is truncated.
pub fn parse(p: &[u8]) -> io::Result<Event<'_>> {
    let &[4, event, length, ref params @ ..] = p else {
        return Err(io::Error::other("malformed HCI event framing"));
    };
    if params.len() != usize::from(length) {
        return Err(io::Error::other("malformed HCI event framing"));
    }
    // LE Set/Update Connection command opcode 0x2013, little endian.
    Ok(match (event, params) {
        (5, &[0, h0, h1, _reason]) => Event::Disconnected(u16::from_le_bytes([h0, h1])),
        (0x0f, &[status, _, 0x13, 0x20]) | (0x0e, &[_, 0x13, 0x20, status, ..]) => {
            Event::CommandStatus(status)
        }
        (0x3e, []) => return Err(io::Error::other("missing LE subevent")),
        (0x3e, &[subevent, ..]) => le_meta(subevent, params)?,
        _ => Event::Other,
    })
}
/// `params` starts with the subevent code.
fn le_meta(subevent: u8, params: &[u8]) -> io::Result<Event<'_>> {
    Ok(match subevent {
        // Legacy, enhanced, and enhanced-v2 share this identity prefix.
        1 | 0x0a | 0x29 => {
            let (minimum, offset) = match subevent {
                1 => (19, 12),
                0x0a => (31, 24),
                _ => (34, 24),
            };
            if params.len() < minimum {
                return Err(io::Error::other("short LE connection event"));
            }
            #[expect(
                clippy::indexing_slicing,
                reason = "every minimum covers the identity prefix and offset + 6"
            )]
            let (status, handle, role, address_type, address, parameters) = (
                params[1],
                u16::from_le_bytes([params[2], params[3]]),
                params[4],
                params[5],
                &params[6..12],
                &params[offset..offset + 6],
            );
            if status != 0 {
                return Ok(Event::Other);
            }
            Event::Connected {
                handle,
                role,
                address_type,
                address,
                parameters: Parameters::parse(parameters)?,
            }
        }
        3 => {
            let &[_, status, h0, h1, ref parameters @ ..] = params else {
                return Err(io::Error::other("short LE update event"));
            };
            if parameters.len() != 6 {
                return Err(io::Error::other("short LE update event"));
            }
            Event::Updated(
                u16::from_le_bytes([h0, h1]),
                status,
                if status == 0 {
                    Some(Parameters::parse(parameters)?)
                } else {
                    None
                },
            )
        }
        _ => Event::Other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_interval_command_bytes() {
        assert_eq!(
            ConnectionParameters::LOW_LATENCY.encode_update(0x201),
            [
                1, 0x13, 0x20, 14, 1, 2, 6, 0, 6, 0, 0, 0, 200, 0, 0, 0, 0, 0
            ]
        );
    }
}
