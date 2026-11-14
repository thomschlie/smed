//! smed: driver daemon for the SpaceMouse Enterprise (USB 256f:c633).
//!
//! smed owns the device through hidraw, keeps the rotation lock and its LED, and reports
//! motion and every key (3Dx ID, short and long press) to clients over its JSON-lines API
//! (docs/api.md). The binary is `main.rs`; the parts live here so they can be tested without
//! a device.

pub mod core;
pub mod daemon;
pub mod device;
pub mod init;
pub mod protocol;
