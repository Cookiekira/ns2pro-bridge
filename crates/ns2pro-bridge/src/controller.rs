//! One controller session, command serialization, initialization and output routing.
use crate::{
    ble::{GattLink, LinkError},
    channels::{Channels, Identity},
};
use ns2pro_proto::{
    Address,
    calibration::{self, Calibration},
    command,
    gatt::{Characteristic, INIT_PAYLOAD},
    input::parse_common_report,
    pairing::{Pairing, PairingError},
};
use std::{convert::Infallible, time::Duration};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
/// Controller command, pairing or link failure.
#[derive(Debug, thiserror::Error)]
pub enum ControllerError {
    /// The GATT link failed.
    #[error(transparent)]
    Link(#[from] LinkError),
    /// A command response was malformed or answered another command.
    #[error(transparent)]
    Response(#[from] command::ResponseError),
    /// Pairing setup or exchange failed; see [`PairingError`].
    #[error(transparent)]
    Pairing(#[from] PairingError),
    /// Shutdown cancelled the operation.
    #[error("controller operation cancelled")]
    Cancelled,
    /// The OS random source failed while generating pairing keys.
    #[error("random key generation: {0}")]
    Random(String),
}
/// Owns the link from acquisition through teardown, including failed initialization.
pub(crate) struct Connection<'a, L: GattLink> {
    link: L,
    channels: &'a Channels,
}
impl<'a, L: GattLink> Connection<'a, L> {
    pub(crate) fn new(link: L, channels: &'a Channels) -> Self {
        channels.rumble.send_replace(None);
        Self { link, channels }
    }
    pub(crate) async fn initialize(&self, flags: u8) -> Result<Controller<'_, L>, ControllerError> {
        let mut controller = Controller::new(&self.link).await?;
        controller.initialize(self.channels, flags).await?;
        Ok(controller)
    }
    pub(crate) fn established(&self) {
        let report = self.link.connection_report();
        crate::sync::lock(&self.channels.stats).connected(report);
    }
    pub(crate) async fn disconnect(self) {
        crate::sync::lock(&self.channels.stats).disconnected();
        // Revoke callbacks before taking the watch lock for the final neutral sample.
        // Input callbacks check this same token under that lock.
        self.link.closed().cancel();
        self.channels.neutral();
        self.link.close().await;
    }
}
/// Serialized command channel to one controller over a [`GattLink`].
pub struct Controller<'a, L: GattLink> {
    link: &'a L,
    responses: mpsc::Receiver<Vec<u8>>,
}
impl<L: GattLink> std::fmt::Debug for Controller<'_, L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Controller").finish_non_exhaustive()
    }
}
impl<'a, L: GattLink> Controller<'a, L> {
    /// Sends the init payload and subscribes to command responses.
    ///
    /// # Errors
    ///
    /// [`ControllerError::Link`] when either GATT operation fails.
    pub async fn new(link: &'a L) -> Result<Self, ControllerError> {
        let (tx, rx) = mpsc::channel(8);
        link.write(Characteristic::Init, &INIT_PAYLOAD).await?;
        link.subscribe(
            Characteristic::CommandResponse,
            Box::new(move |data| {
                let _ = tx.try_send(data.to_vec());
            }),
        )
        .await?;
        Ok(Self {
            link,
            responses: rx,
        })
    }
    async fn command(&mut self, request: &[u8]) -> Result<Vec<u8>, ControllerError> {
        // Exclusive ownership serializes commands without a runtime lock.
        let responses = &mut self.responses;
        while responses.try_recv().is_ok() {}
        #[expect(
            clippy::indexing_slicing,
            reason = "requests come from command::command, which writes an 8-byte header"
        )]
        let (cmd, sub) = (request[0], request[3]);
        let closed = self.link.closed();
        let transaction = async {
            self.link.write(Characteristic::Command, request).await?;
            let response = responses.recv().await.ok_or(LinkError::Closed)?;
            command::response_payload(&response, cmd, sub)?;
            Ok(response)
        };
        tokio::select! {
            () = closed.cancelled() => Err(LinkError::Closed.into()),
            result = tokio::time::timeout(Duration::from_secs(2), transaction) => {
                result.map_err(|_| ControllerError::Link(LinkError::Timeout))?
            }
        }
    }
    /// Applies LEDs, reads calibration and identity, enables `flags` and starts
    /// publishing input. Calibration and identity failures fall back to defaults.
    ///
    /// # Errors
    ///
    /// When an LED or feature command fails or the input subscription fails.
    pub async fn initialize(
        &mut self,
        channels: &Channels,
        flags: u8,
    ) -> Result<(), ControllerError> {
        self.command(&command::set_player_leds(*channels.leds.borrow()))
            .await?;
        let mut blocks = Vec::new();
        let mut calibration = Calibration::default();
        for address in [
            calibration::PRIMARY_FACTORY_ADDRESS,
            calibration::SECONDARY_FACTORY_ADDRESS,
            calibration::USER_ADDRESS,
        ] {
            match self
                .command(&command::spi_read(address, calibration::BLOCK_SIZE))
                .await
                .and_then(|r| Ok(command::spi_read_data(&r, address)?.to_vec()))
            {
                Ok(b) => blocks.push(b),
                Err(e) => {
                    tracing::warn!(%e,"calibration read failed; using uncalibrated fallback");
                    break;
                }
            }
        }
        if let [primary, secondary, user] = blocks.as_slice() {
            calibration =
                Calibration::from_blocks(primary, secondary, Some(user)).unwrap_or_default();
        }
        let identity = match self
            .command(&command::spi_read(0x13000, 0x40))
            .await
            .and_then(|r| {
                command::spi_read_data(&r, 0x13000)?
                    .try_into()
                    .map_err(|_| command::ResponseError::ShortSpi.into())
            }) {
            Ok(block) => Some(block),
            Err(e) => {
                tracing::warn!(%e,"identity read failed; using neutral fallback");
                None
            }
        };
        channels.identity.send_replace(Identity::Resolved(identity));
        self.command(&command::set_feature_mask(0xff)).await?;
        self.command(&command::enable_features(flags)).await?;
        let channels = channels.clone();
        let origin = std::time::Instant::now();
        let closed = self.link.closed();
        self.link
            .subscribe(
                Characteristic::Input,
                Box::new(
                    move |bytes| match parse_common_report(bytes, &calibration) {
                        Ok(state) => {
                            channels.input.send_modify(|sample| {
                                // Check under the watch lock so an in-flight WinRT callback cannot
                                // publish stale held input after Connection::disconnect's neutral reset.
                                if closed.is_cancelled() {
                                    return;
                                }
                                sample.state = state;
                                sample.seq = sample.seq.wrapping_add(1);
                                #[expect(
                                    clippy::cast_possible_truncation,
                                    clippy::as_conversions,
                                    reason = "motion timestamp is a wrapping 32-bit microsecond counter"
                                )]
                                let timestamp = origin.elapsed().as_micros() as u32;
                                sample.motion_timestamp_us = timestamp;
                                crate::sync::lock(&channels.stats).receipt(sample.seq);
                            });
                        }
                        Err(error) => tracing::warn!(%error,"discarding malformed input"),
                    },
                ),
            )
            .await?;
        Ok(())
    }
    /// Runs the pairing exchange for `host`.
    ///
    /// # Errors
    ///
    /// On link failure, a rejected stage or an LTK confirmation mismatch.
    pub async fn pair(
        &mut self,
        host: Address,
        key: [u8; 16],
        challenge: [u8; 16],
    ) -> Result<(), ControllerError> {
        let mut state = Pairing::new(host, key, challenge)?;
        while let Some(request) = state.next_command() {
            state.on_response(&self.command(&request).await?)?;
        }
        Ok(())
    }
    /// Forwards rumble and LED changes until cancelled or the link closes.
    ///
    /// # Errors
    ///
    /// Always: [`ControllerError::Cancelled`], or the link failure that ended it.
    pub async fn run(
        &mut self,
        channels: &Channels,
        cancel: &CancellationToken,
    ) -> Result<(), ControllerError> {
        let mut leds = channels.leds.subscribe();
        let mut rumble = channels.rumble.subscribe();
        let closed = self.link.closed();
        // Leading edge: the first fresh output writes immediately; only latest pending state survives.
        let link = self.link;
        let rumble_task = async {
            let mut next = tokio::time::Instant::now();
            loop {
                if tokio::time::Instant::now() < next {
                    tokio::time::sleep_until(next).await;
                }
                let value = *rumble.borrow_and_update();
                if let Some(v) = value {
                    link.write(
                        Characteristic::Vibration,
                        &command::rumble_packet(&v.left, &v.right),
                    )
                    .await?;
                    crate::sync::lock(&channels.stats).rumble_written();
                    next = tokio::time::Instant::now() + Duration::from_millis(20);
                }
                rumble.changed().await.map_err(|_| LinkError::Closed)?;
            }
        };
        let led_task = async {
            loop {
                let v = *leds.borrow_and_update();
                self.command(&command::set_player_leds(v)).await?;
                leds.changed().await.map_err(|_| LinkError::Closed)?;
            }
        };
        let result: Result<Infallible, ControllerError> = tokio::select! {
            () = cancel.cancelled() => Err(ControllerError::Cancelled),
            () = closed.cancelled() => Err(LinkError::Closed.into()),
            result = rumble_task => result,
            result = led_task => result,
        };
        result.map(|never| match never {})
    }
}
