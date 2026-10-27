//! Native Bluetooth boundary. Futures deliberately need not be Send.
use ns2pro_proto::{Address, LeAddressType, gatt::Characteristic};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const SCAN_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Connection-time evidence, independent of notification receipt timing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConnectionReport {
    /// What the bridge asked the connection timing to be.
    pub target: ConnectionTarget,
    /// How the platform answered that request.
    pub preference: PreferenceOutcome,
    /// Negotiated interval in Bluetooth units of 1.25 ms.
    pub observed_interval: Option<u16>,
}
/// The connection timing the bridge requested.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConnectionTarget {
    /// Nothing requested.
    #[default]
    Unspecified,
    /// A fixed interval, in 1.25 ms units.
    Interval(u16),
    /// The platform's throughput-optimized preference, with no fixed interval.
    ThroughputOptimized,
}
/// The platform's answer to the connection-timing request.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreferenceOutcome {
    /// No request was made.
    #[default]
    NotRequested,
    /// The request could not be made or delivered.
    Unavailable,
    /// The controller or platform rejected the request.
    Refused,
    /// The outcome is unknown; the request may not even have been submitted.
    Unconfirmed,
    /// On Linux, the controller reported the requested interval in effect; on
    /// Windows, the preference request succeeded, which fixes no interval.
    Confirmed,
}

/// Called with each notified value on the platform's notification path; keep it short.
pub type NotificationSink = Box<dyn FnMut(&[u8]) + Send + 'static>;
/// An advertising controller found by [`Bluetooth::scan`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Discovered {
    /// Controller address.
    pub address: Address,
    /// Its LE address type.
    pub address_type: LeAddressType,
}
/// Adapter, discovery or connection failure.
#[derive(Debug, thiserror::Error)]
pub enum BleError {
    /// OS I/O failure.
    #[error("Bluetooth I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Platform API failure, with a hint where useful.
    #[error("Bluetooth: {0}")]
    Platform(String),
    /// Scanning or connection setup timed out.
    #[error("Bluetooth discovery timed out")]
    Timeout,
    /// Shutdown cancelled the operation.
    #[error("Bluetooth operation cancelled")]
    Cancelled,
}
impl From<ns2pro_proto::mgmt::Malformed> for BleError {
    fn from(error: ns2pro_proto::mgmt::Malformed) -> Self {
        Self::Platform(error.to_string())
    }
}
/// Failure on an established GATT link.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    /// OS I/O failure.
    #[error("GATT I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Platform GATT failure or protocol error.
    #[error("GATT: {0}")]
    Platform(String),
    /// A GATT transaction got no answer in time.
    #[error("GATT command timed out")]
    Timeout,
    /// The link closed or was disconnected.
    #[error("GATT link closed")]
    Closed,
}
/// A platform Bluetooth adapter.
pub trait Bluetooth {
    /// The link type [`Self::connect`] returns.
    type Link: GattLink;
    /// Wait for a fresh matching advertisement, including for a known target.
    fn scan(&self, target: Option<Address>) -> impl Future<Output = Result<Discovered, BleError>>;
    /// Connects to `target` and discovers the controller's characteristics.
    fn connect(&self, target: &Discovered) -> impl Future<Output = Result<Self::Link, BleError>>;
    /// The adapter's own address, used as the pairing host identity.
    fn adapter_address(&self) -> impl Future<Output = Result<Address, BleError>>;
}
/// An established GATT connection to the controller.
pub trait GattLink: 'static {
    /// What is known about the connection's timing.
    fn connection_report(&self) -> ConnectionReport;
    /// Writes `data` to `characteristic` without a response.
    fn write(
        &self,
        characteristic: Characteristic,
        data: &[u8],
    ) -> impl Future<Output = Result<(), LinkError>>;
    /// Enables notifications on `characteristic`, delivering each value to `sink`.
    fn subscribe(
        &self,
        characteristic: Characteristic,
        sink: NotificationSink,
    ) -> impl Future<Output = Result<(), LinkError>>;
    /// Cancelled once the link closes, from either side.
    fn closed(&self) -> CancellationToken;
    /// Idempotently release the link, including partially initialized sessions.
    fn close(&self) -> impl Future<Output = ()>;
}

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(windows)]
pub mod windows;
