//! smed: driver daemon for the SpaceMouse Enterprise (USB 256f:c633).
//!
//! smed owns the device through hidraw, keeps the rotation lock and its LED, and reports
//! motion and every key (3Dx ID, short and long press) to clients over its JSON-lines API
//! (docs/api.md), and draws the pages clients send on the LCD. It serves the spacenavd/libspnav
//! socket protocol too (docs/spnav.md). The binary is `main.rs`; the
//! parts live here so they can be tested without a device.

pub mod config;
pub mod core;
pub mod daemon;
pub mod device;
pub mod frame;
pub mod init;
pub mod lcd;
pub mod protocol;
pub mod render;
pub mod spnav;
pub mod usb;
