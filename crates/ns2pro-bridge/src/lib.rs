//! Bridges a Switch 2 Pro Controller over BLE to a virtual USB device.
//!
//! The library exists for the `ns2pro-bridge` binary and its integration tests; it
//! is not a stable API.
pub mod ble;
pub mod cache;
pub mod channels;
pub mod cli;
pub mod controller;
pub mod session;
pub mod stats;
mod sync;
pub mod usb;

pub mod app;
mod rt;
