//! Virtual USB device transport: USB/IP serving and native attachment.
mod attachment;
pub use attachment::{Attachment, Error};
pub mod server;
pub mod session;

#[cfg(windows)]
pub mod usbip_win;
#[cfg(target_os = "linux")]
pub mod vhci;
/// Failure attaching or detaching the virtual device.
#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    /// OS I/O failure.
    #[error("USB attach I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Driver or platform failure, with a hint.
    #[error("USB attach: {0}")]
    Platform(String),
    /// Shutdown cancelled the attach.
    #[error("USB attach cancelled")]
    Cancelled,
}
