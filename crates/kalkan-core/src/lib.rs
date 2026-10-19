//! Cross-platform core of the driver for the **Kalkan Alpha 240 / 360 /
//! 360-W** and **Delta V2 240 / 360** AIO watercoolers (Kalkan PC SmartMonitor
//! **1.3** protocol). The Alpha models have a round LCD that shows only the CPU
//! temperature; Delta V2 is compatible by protocol but not yet hardware-tested.
//!
//! Rust reimplementation of the behavior observed in the original Windows app
//! (see `docs/v1.3/protocol.md`). Builds for Windows, macOS and Linux.
//!
//! The 1.3 talks to the cooler **over a serial port only** (USB-serial), unlike
//! the 1.2, which also had an HID path. The device is found by VID/PID and
//! opened at 1 Mbps, 8N1, no flow control. There is no handshake.

pub mod device;
pub mod firmware;
pub mod hwmon;
pub mod macos;
pub mod naming;
pub mod selection;
pub mod sensors;
pub mod settings;
pub mod transport;
pub mod v1_3;

/// Stable path for the firmware 1.3 protocol (`kalkan_core::protocol::...`).
pub mod protocol {
    pub use crate::v1_3::protocol::*;
}

pub use device::{
    available_serial_ports, enumerate, find_display_port, is_display_port, looks_like_display,
    HidDeviceInfo, SerialDeviceInfo, DISPLAY_IDS,
};
pub use firmware::{open, profile_for, profiles, Bus, Display, Fields, Firmware, Profile, Sent};
pub use hwmon::LinuxSource;
pub use macos::MacSource;
pub use v1_3::protocol::{build_frame, scramble, KalkanBuilder, PacketBuilder, FRAME_LEN};
pub use selection::{
    fan_options, same_layout, select_fan, select_temp, temp_options, FanSource, MaxTracker,
    SourceOption, TempSource,
};
pub use sensors::{default_source, Reading, SensorKind, SensorSource, SensorValue, SysinfoSource};
pub use settings::{CoolerModel, Settings};
pub use transport::{HidTransport, SerialTransport, Transport};
pub use v1_3::{open_display, BAUD};

use anyhow::Result;
use std::time::Duration;

/// Driver loop: reads sensors, builds packets and sends them to the display at an interval.
///
/// Ties the three pieces together (`SensorSource` + `PacketBuilder` + `Transport`)
/// without coupling the UI. Runs until `should_stop()` returns true.
pub fn run_loop(
    sensors: &mut dyn SensorSource,
    builder: &mut dyn PacketBuilder,
    transport: &mut dyn Transport,
    interval: Duration,
    mut should_stop: impl FnMut() -> bool,
) -> Result<()> {
    while !should_stop() {
        let reading = sensors.read()?;
        for pkt in builder.build(&reading) {
            transport.write(&pkt)?;
        }
        std::thread::sleep(interval);
    }
    Ok(())
}
