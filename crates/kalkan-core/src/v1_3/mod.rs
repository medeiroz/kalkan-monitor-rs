//! Firmware 1.3: serial 1 Mbaud, 5-byte frame.

pub mod protocol;
pub mod serial;

pub use serial::{open_display, BAUD, IDS};
