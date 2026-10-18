//! Cross-platform core of the Kalkan SmartMonitor **1.3** watercooler driver.
//!
//! Rust reimplementation of the behavior observed in the original Windows app
//! (see `docs/protocol.md`). Builds for Windows, macOS and Linux.
//!
//! The 1.3 talks to the cooler **over a serial port only** (USB-serial), unlike
//! the 1.2, which also had an HID path. The device is found by VID/PID and
//! opened at 1 Mbps, 8N1, no flow control. There is no handshake.

pub mod device;
pub mod protocol;
pub mod selection;
pub mod sensors;
pub mod settings;
pub mod transport;

pub use device::{
    available_serial_ports, enumerate, find_display_port, is_display_port, looks_like_display,
    HidDeviceInfo, SerialDeviceInfo, DISPLAY_IDS,
};
pub use protocol::{build_frame, scramble, KalkanBuilder, PacketBuilder, FRAME_LEN};
pub use selection::{
    fan_options, select_fan, select_temp, temp_options, FanSource, SourceOption, TempSource,
};
pub use sensors::{default_source, Reading, SensorKind, SensorSource, SensorValue, SysinfoSource};
pub use settings::Settings;
pub use transport::{HidTransport, SerialTransport, Transport};

use anyhow::{anyhow, Result};
use std::time::Duration;

/// Baud rate of the 1.3 display (`QSerialPort::setBaudRate(1000000, ...)`).
pub const BAUD: u32 = 1_000_000;

/// Finds and opens the 1.3 display on the first serial port whose VID/PID matches.
///
/// Reproduces `Sysctl::connect`/`Transport::open` (`FUN_140008300` →
/// `FUN_1400083d0`): enumerates the ports, matches by (VID, PID) against
/// [`DISPLAY_IDS`], and opens at 1 Mbps / 8N1 / no flow control.
pub fn open_display() -> Result<SerialTransport> {
    let port = find_display_port()?.ok_or_else(|| {
        anyhow!(
            "no Kalkan 1.3 display found (looking for VID:PID {})",
            DISPLAY_IDS
                .iter()
                .map(|(v, p)| format!("{v:04x}:{p:04x}"))
                .collect::<Vec<_>>()
                .join(" or ")
        )
    })?;
    SerialTransport::open_display(&port)
}

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
