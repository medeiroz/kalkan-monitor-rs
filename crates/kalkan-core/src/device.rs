//! Device discovery — serial ports (firmware 1.3) and HID (firmware 1.2,
//! implemented in `v1_2::hid`).
//!
//! The 1.3 talks over a **USB serial port**. The original app finds the cooler
//! by enumerating `QSerialPortInfo::availablePorts()` and matching by VID/PID
//! (`FUN_1400083d0`), trying two known pairs (see [`DISPLAY_IDS`]).

use anyhow::Result;
use serialport::SerialPortType;

// Firmware 1.3 items, kept reachable at their historical paths.
pub use crate::v1_3::serial::{
    callout_path, find_display_port, ioreg_lists_display, macos_hint, not_found_hint,
};
pub use crate::v1_3::IDS as DISPLAY_IDS;

/// A serial port seen in the system enumeration (cross-platform).
#[derive(Debug, Clone)]
pub struct SerialDeviceInfo {
    /// Port name (e.g. `COM5` on Windows, `/dev/ttyUSB0` on Linux).
    pub port_name: String,
    /// VID/PID when the port is USB (serialport only exposes them for USB ports).
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
}

impl SerialDeviceInfo {
    /// `true` if the VID/PID matches any pair in [`DISPLAY_IDS`].
    pub fn is_display(&self) -> bool {
        match (self.vendor_id, self.product_id) {
            (Some(v), Some(p)) => DISPLAY_IDS.contains(&(v, p)),
            _ => false,
        }
    }

    /// The supported-display profile (firmware/bus) this port matches, if any.
    pub fn profile(&self) -> Option<&'static crate::Profile> {
        crate::profile_for(self.vendor_id?, self.product_id?).filter(|p| p.bus == crate::Bus::Serial)
    }
}

/// Lists all serial ports on the system, with VID/PID when USB.
pub fn available_serial_ports() -> Result<Vec<SerialDeviceInfo>> {
    let ports = serialport::available_ports()?;
    Ok(ports
        .into_iter()
        .map(|p| {
            let (vendor_id, product_id, manufacturer, product, serial_number) = match p.port_type {
                SerialPortType::UsbPort(info) => (
                    Some(info.vid),
                    Some(info.pid),
                    info.manufacturer,
                    info.product,
                    info.serial_number,
                ),
                _ => (None, None, None, None, None),
            };
            SerialDeviceInfo {
                port_name: p.port_name,
                vendor_id,
                product_id,
                manufacturer,
                product,
                serial_number,
            }
        })
        .collect())
}

/// `true` if the port described by `info` matches [`DISPLAY_IDS`].
pub fn is_display_port(info: &SerialDeviceInfo) -> bool {
    info.is_display()
}

// -- HID (firmware 1.2) -------------------------------------------------------

// Firmware 1.2 HID items (implemented in `v1_2::hid`), kept reachable at their
// historical paths.
pub use crate::v1_2::hid::{enumerate, looks_like_display, HidDeviceInfo};
