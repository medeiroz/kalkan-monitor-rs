//! Firmware 1.3: serial 1 Mbaud, 5-byte frame.

pub mod display;
pub mod protocol;
pub mod serial;

pub use display::V1_3Display;
pub use serial::{open_display, BAUD, IDS};
