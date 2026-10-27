//! Deterministic host pairing. Randomness is supplied by the caller.
use crate::{
    Address,
    command::{ResponseError, command, response_payload},
};
use aes::{
    Aes128,
    cipher::{BlockCipherEncrypt, KeyInit},
};
#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error(transparent)]
    Response(#[from] ResponseError),
    #[error("pairing rejected at stage {0}")]
    Rejected(u8),
    #[error("LTK confirmation mismatch")]
    Confirmation,
    #[error("host address cannot be zero")]
    ZeroAddress,
}
#[derive(Debug)]
pub struct Pairing {
    host: Address,
    key: [u8; 16],
    challenge: [u8; 16],
    ltk: [u8; 16],
    stage: u8,
}
impl Pairing {
    /// # Errors
    ///
    /// [`PairingError::ZeroAddress`] when `host` is zero.
    pub const fn new(
        host: Address,
        key: [u8; 16],
        challenge: [u8; 16],
    ) -> Result<Self, PairingError> {
        if host.value() == 0 {
            return Err(PairingError::ZeroAddress);
        }
        Ok(Self {
            host,
            key,
            challenge,
            ltk: [0; 16],
            stage: 0,
        })
    }
    /// # Panics
    ///
    /// Never: [`Self::new`] rejects a zero host, so `host - 1` is a valid address.
    #[must_use]
    #[expect(clippy::expect_used, reason = "the host is nonzero by construction")]
    pub fn next_command(&self) -> Option<Vec<u8>> {
        let (sub, payload) = match self.stage {
            0 => {
                let mut p = vec![0, 2];
                p.extend(self.host.to_le_bytes());
                let previous = Address::new(self.host.value() - 1).expect("nonzero host");
                p.extend(previous.to_le_bytes());
                (1, p)
            }
            1 => (4, [&[0][..], &self.key].concat()),
            2 => (2, [&[0][..], &self.challenge].concat()),
            3 => (3, vec![0]),
            _ => return None,
        };
        Some(command(0x15, sub, &payload))
    }
    /// Advances pairing with the controller's reply to [`Self::next_command`].
    ///
    /// # Errors
    ///
    /// [`PairingError::Response`] for a malformed reply, [`PairingError::Rejected`]
    /// when the controller refuses a stage or pairing already finished, and
    /// [`PairingError::Confirmation`] when the derived LTK does not match.
    pub fn on_response(&mut self, reply: &[u8]) -> Result<(), PairingError> {
        // (subcommand, minimum payload length) per stage.
        let (sub, required) = [(1, 9), (4, 17), (2, 17), (3, 1)]
            .get(usize::from(self.stage))
            .copied()
            .ok_or(PairingError::Rejected(self.stage))?;
        let p = response_payload(reply, 0x15, sub)?;
        let Some((&1, data)) = p.split_first().filter(|_| p.len() >= required) else {
            return Err(PairingError::Rejected(sub));
        };
        let block = data.first_chunk::<16>();
        match (self.stage, block) {
            (1, Some(peer)) => {
                for ((ltk, key), peer) in self.ltk.iter_mut().zip(self.key).zip(peer) {
                    *ltk = key ^ peer;
                }
            }
            (2, _) if block != Some(&confirmation(self.ltk, self.challenge)) => {
                return Err(PairingError::Confirmation);
            }
            _ => {}
        }
        self.stage += 1;
        Ok(())
    }
}
#[must_use]
pub fn confirmation(mut ltk: [u8; 16], mut challenge: [u8; 16]) -> [u8; 16] {
    ltk.reverse();
    challenge.reverse();
    let cipher = Aes128::new(&ltk.into());
    let mut block = challenge.into();
    cipher.encrypt_block(&mut block);
    block.into()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_and_reject() {
        let mut p = Pairing::new(
            Address::new(0x1122_3344_5566).unwrap(),
            [0x11; 16],
            [0x22; 16],
        )
        .unwrap();
        assert_eq!(
            &p.next_command().unwrap()[8..],
            &[
                0, 2, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0x65, 0x55, 0x44, 0x33, 0x22, 0x11
            ]
        );
        for sub in [1, 4, 2, 3] {
            let mut r = vec![0x15, 0, 0, sub, 0, 0, 0, 0, 1];
            r.extend(if sub == 2 {
                confirmation([0x22; 16], [0x22; 16])
            } else {
                [0x33; 16]
            });
            p.on_response(&r).unwrap();
        }
        assert!(p.next_command().is_none());
        let mut p = Pairing::new(Address::new(1).unwrap(), [0; 16], [0; 16]).unwrap();
        assert!(p.on_response(&[0x15, 0, 0, 1, 0, 0, 0, 0, 0]).is_err());
    }
}
