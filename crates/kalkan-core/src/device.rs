//! Device discovery — serial ports (1.3) and HID (legacy/diagnostics).
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
        crate::profile_for(self.vendor_id?, self.product_id?)
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

// -- HID (legacy 1.2 / diagnostics) ------------------------------------------

/// An HID device seen in the system enumeration (cross-platform).
#[derive(Debug, Clone)]
pub struct HidDeviceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
    pub path: String,
}

/// Lists all HID devices on the system.
///
/// The 1.3 does not use HID (the transport is serial); this stays as a
/// diagnostic tool, useful when inspecting unknown hardware.
pub fn enumerate() -> Result<Vec<HidDeviceInfo>> {
    let api = hidapi::HidApi::new()?;
    Ok(api
        .device_list()
        .map(|d| HidDeviceInfo {
            vendor_id: d.vendor_id(),
            product_id: d.product_id(),
            manufacturer: d.manufacturer_string().map(str::to_owned),
            product: d.product_string().map(str::to_owned),
            serial_number: d.serial_number().map(str::to_owned),
            path: d.path().to_string_lossy().into_owned(),
        })
        .collect())
}

/// Heuristic to highlight "cooler display" candidates in an HID list.
pub fn looks_like_display(info: &HidDeviceInfo) -> bool {
    let hay = format!(
        "{} {}",
        info.manufacturer.as_deref().unwrap_or(""),
        info.product.as_deref().unwrap_or("")
    )
    .to_lowercase();
    ["lcd", "screen", "display", "monitor", "vtf", "com"]
        .iter()
        .any(|k| hay.contains(k))
}
