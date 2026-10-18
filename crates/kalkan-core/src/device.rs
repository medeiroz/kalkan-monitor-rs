//! Device discovery — serial ports (1.3) and HID (legacy/diagnostics).
//!
//! The 1.3 talks over a **USB serial port**. The original app finds the cooler
//! by enumerating `QSerialPortInfo::availablePorts()` and matching by VID/PID
//! (`FUN_1400083d0`), trying two known pairs (see [`DISPLAY_IDS`]).

use anyhow::Result;
use serialport::SerialPortType;

/// (VID, PID) pairs of the 1.3 display, in the order the original tries them.
///
/// From `Sysctl::connect` (`FUN_140008300`): first `1F3A:0008`, otherwise
/// `1A86:484A` (CH340 family — a common USB-serial bridge).
pub const DISPLAY_IDS: &[(u16, u16)] = &[(0x1F3A, 0x0008), (0x1A86, 0x484A)];

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

/// Finds the name of the first serial port whose VID/PID matches [`DISPLAY_IDS`].
///
/// Respects the order of [`DISPLAY_IDS`] (same as the original: tries `1F3A:0008`
/// before `1A86:484A`). On macOS, where every port is listed twice (`cu.` and
/// `tty.`), the callout `cu.` device is preferred.
pub fn find_display_port() -> Result<Option<String>> {
    Ok(pick_display_port(&available_serial_ports()?))
}

/// The display's port among `ports`, by [`DISPLAY_IDS`] order, `cu.` first.
fn pick_display_port(ports: &[SerialDeviceInfo]) -> Option<String> {
    DISPLAY_IDS.iter().find_map(|&(vid, pid)| {
        ports
            .iter()
            .filter(|p| p.vendor_id == Some(vid) && p.product_id == Some(pid))
            .min_by_key(|p| !p.port_name.starts_with("/dev/cu."))
            .map(|p| p.port_name.clone())
    })
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

// -- macOS specifics -------------------------------------------------------------

/// The callout (`/dev/cu.*`) device for a dial-in (`/dev/tty.*`) one. macOS
/// lists both for every USB serial port; the `tty.` device waits for carrier
/// detect, so the display must be opened through `cu.`. Other names are
/// returned unchanged.
pub fn callout_path(port: &str) -> String {
    match port.strip_prefix("/dev/tty.") {
        Some(rest) => format!("/dev/cu.{rest}"),
        None => port.to_string(),
    }
}

/// Whether the `ioreg -p IOUSB -l` listing contains a USB device with one of
/// the [`DISPLAY_IDS`] (`"idVendor" = 6790` / `"idProduct" = 18506`).
pub fn ioreg_lists_display(out: &str) -> bool {
    fn value(line: &str, key: &str) -> Option<u32> {
        line.trim_start_matches(|c: char| c == '|' || c.is_whitespace())
            .strip_prefix(key)?
            .trim_start()
            .strip_prefix('=')?
            .trim()
            .parse()
            .ok()
    }
    let (mut vid, mut pid) = (None, None);
    let matches = |v: Option<u32>, p: Option<u32>| {
        DISPLAY_IDS.iter().any(|&(dv, dp)| v == Some(dv as u32) && p == Some(dp as u32))
    };
    for line in out.lines() {
        if line.contains("+-o ") {
            if matches(vid, pid) {
                return true;
            }
            (vid, pid) = (None, None);
        }
        vid = value(line, "\"idVendor\"").or(vid);
        pid = value(line, "\"idProduct\"").or(pid);
    }
    matches(vid, pid)
}

/// What to tell the user on macOS when no display port exists.
pub fn macos_hint(usb_listed: bool) -> String {
    if usb_listed {
        " The display is connected over USB, but macOS created no serial port for it: \
         its CH34x chip needs a driver. Recent macOS versions include one for the common \
         CH340/CH341 chips; older ones (or other variants) need WCH's CH34xVCPDriver \
         (wch-ic.com). After installing it, allow it in System Settings > Privacy & \
         Security, then replug the display."
            .into()
    } else {
        " Check the USB cable and replug the display. Its serial port appears as \
         /dev/cu.usbserial-* or /dev/cu.wchusbserial*; if the display is plugged in and \
         still missing, its CH34x chip may need WCH's CH34xVCPDriver (wch-ic.com) on \
         older macOS versions."
            .into()
    }
}

/// Extra text for the "display not found" error: empty except on macOS, where
/// it separates a missing cable from a missing CH34x driver by looking at the
/// USB bus. The `ioreg` call is cached for a few seconds, since the UI retries
/// every second while the display is absent.
pub fn not_found_hint() -> String {
    #[cfg(target_os = "macos")]
    {
        use std::sync::Mutex;
        use std::time::{Duration, Instant};
        static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        let listed = match *cache {
            Some((at, listed)) if at.elapsed() < Duration::from_secs(10) => listed,
            _ => {
                let listed = std::process::Command::new("ioreg")
                    .args(["-p", "IOUSB", "-l", "-w0"])
                    .output()
                    .map(|o| ioreg_lists_display(&String::from_utf8_lossy(&o.stdout)))
                    .unwrap_or(false);
                *cache = Some((Instant::now(), listed));
                listed
            }
        };
        macos_hint(listed)
    }
    #[cfg(not(target_os = "macos"))]
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(name: &str, vid: u16, pid: u16) -> SerialDeviceInfo {
        SerialDeviceInfo {
            port_name: name.into(),
            vendor_id: Some(vid),
            product_id: Some(pid),
            manufacturer: None,
            product: None,
            serial_number: None,
        }
    }

    #[test]
    fn display_port_prefers_callout_and_known_id_order() {
        let ports = [
            port("/dev/tty.usbserial-1410", 0x1A86, 0x484A),
            port("/dev/cu.usbserial-1410", 0x1A86, 0x484A),
            port("/dev/cu.usbmodem1", 0x1234, 0x5678),
        ];
        assert_eq!(pick_display_port(&ports).as_deref(), Some("/dev/cu.usbserial-1410"));
        // 1F3A:0008 wins over 1A86:484A, as in the original.
        let both = [port("COM3", 0x1A86, 0x484A), port("COM4", 0x1F3A, 0x0008)];
        assert_eq!(pick_display_port(&both).as_deref(), Some("COM4"));
        assert_eq!(pick_display_port(&ports[2..]), None);
    }

    #[test]
    fn dial_in_ports_become_callout_ports() {
        assert_eq!(callout_path("/dev/tty.usbserial-1410"), "/dev/cu.usbserial-1410");
        assert_eq!(callout_path("/dev/tty.wchusbserial1410"), "/dev/cu.wchusbserial1410");
        assert_eq!(callout_path("/dev/cu.usbmodem11201"), "/dev/cu.usbmodem11201");
        assert_eq!(callout_path("/dev/ttyUSB0"), "/dev/ttyUSB0");
        assert_eq!(callout_path("COM5"), "COM5");
    }

    const IOREG: &str = "\
+-o Root  <class IORegistryEntry, id 0x100000100>
  +-o AppleT8103USBXHCI@00000000  <class AppleT8103USBXHCI, id 0x1000002a2>
    +-o Keyboard@1  <class IOUSBHostDevice, id 0x1000003b4>
    | {
    |   \"idProduct\" = 1
    |   \"idVendor\" = 1452
    | }
    |
    +-o USB Serial@01100000  <class IOUSBHostDevice, id 0x1000003c0>
      {
        \"idProduct\" = 18506
        \"USB Product Name\" = \"USB Serial\"
        \"idVendor\" = 6790
      }
";

    #[test]
    fn finds_the_display_in_an_ioreg_listing() {
        assert!(ioreg_lists_display(IOREG));
        // The other known pair, 1F3A:0008 (7994:8).
        let other = IOREG.replace("18506", "8").replace("6790", "7994");
        assert!(ioreg_lists_display(&other));
        // Right vendor, wrong product; and the pair split across two devices.
        assert!(!ioreg_lists_display(&IOREG.replace("18506", "29987")));
        let split = "+-o A@1  <c>\n  \"idVendor\" = 6790\n+-o B@2  <c>\n  \"idProduct\" = 18506\n";
        assert!(!ioreg_lists_display(split));
        assert!(!ioreg_lists_display(""));
    }

    #[test]
    fn macos_hint_separates_driver_from_cable() {
        assert!(macos_hint(true).contains("needs a driver"));
        assert!(macos_hint(false).contains("USB cable"));
    }
}
