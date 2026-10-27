//! Latest-value state shared by callbacks, USB and controller sessions.
use crate::stats::Statistics;
use ns2pro_usb::{InputSample, Output};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
/// Progress of reading the controller identity (SPI block 0x13000).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Identity {
    /// Not read yet.
    #[default]
    Pending,
    /// None means the read failed and USB should use the neutral fallback.
    Resolved(Option<[u8; 64]>),
}
impl Identity {
    /// The identity block, or `None` while pending or after a failed read.
    #[must_use]
    pub const fn block(self) -> Option<[u8; 64]> {
        match self {
            Self::Pending => None,
            Self::Resolved(block) => block,
        }
    }
}
/// The latest rumble request for the physical controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rumble {
    /// Left actuator rumble data.
    pub left: [u8; 16],
    /// Right actuator rumble data.
    pub right: [u8; 16],
}
/// Latest-value channels between the BLE session, the USB device and diagnostics.
/// Clones share the same channels.
#[derive(Clone, Debug)]
pub struct Channels {
    /// Latest controller input.
    pub input: watch::Sender<InputSample>,
    /// Player LED mask requested by the host.
    pub leds: watch::Sender<u8>,
    /// Latest host rumble; `None` when nothing is pending.
    pub rumble: watch::Sender<Option<Rumble>>,
    /// Identity block for the USB device.
    pub identity: watch::Sender<Identity>,
    /// Shared latency and throughput statistics.
    pub stats: Arc<Mutex<Statistics>>,
}
impl Default for Channels {
    fn default() -> Self {
        Self {
            input: watch::channel(InputSample::default()).0,
            leds: watch::channel(ns2pro_proto::command::DEFAULT_LED_PATTERN).0,
            rumble: watch::channel(None).0,
            identity: watch::channel(Identity::Pending).0,
            stats: Arc::new(Mutex::new(Statistics::default())),
        }
    }
}
impl Channels {
    /// Publishes a host request for the physical controller.
    pub fn output(&self, output: Output) {
        match output {
            Output::PlayerLeds(v) => {
                self.leds.send_replace(v);
            }
            Output::Rumble { left, right } => {
                self.rumble.send_replace(Some(Rumble { left, right }));
            }
        }
    }
    /// Releases all inputs and drops pending rumble, as after a disconnect.
    pub fn neutral(&self) {
        self.input.send_modify(|s| {
            s.state = ns2pro_proto::InputState::default();
            s.seq = s.seq.wrapping_add(1);
            s.motion_timestamp_us = 0;
        });
        self.rumble.send_replace(None);
    }
}
