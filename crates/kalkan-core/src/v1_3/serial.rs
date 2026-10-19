//! Firmware 1.3 serial link: display USB IDs, port discovery and opening, and
//! the macOS / Linux hints shown when the port is missing or not accessible.

use crate::device::{available_serial_ports, SerialDeviceInfo};
use crate::transport::SerialTransport;
use anyhow::{anyhow, Result};
use std::time::Duration;

/// Baud rate of the 1.3 display (`QSerialPort::setBaudRate(1000000, ...)`).
pub const BAUD: u32 = 1_000_000;

/// (VID, PID) pairs of the 1.3 display, in the order the original tries them.
///
/// From `Sysctl::connect` (`FUN_140008300`): first `1F3A:0008`, otherwise
/// `1A86:484A` (CH340 family — a common USB-serial bridge).
pub const IDS: &[(u16, u16)] = &[(0x1F3A, 0x0008), (0x1A86, 0x484A)];

/// Finds the name of the first serial port whose VID/PID matches [`IDS`].
///
/// Respects the order of [`IDS`] (same as the original: tries `1F3A:0008`
/// before `1A86:484A`). On macOS, where every port is listed twice (`cu.` and
/// `tty.`), the callout `cu.` device is preferred.
pub fn find_display_port() -> Result<Option<String>> {
    Ok(pick_display_port(&available_serial_ports()?))
}

/// The display's port among `ports`, by [`IDS`] order, `cu.` first.
fn pick_display_port(ports: &[SerialDeviceInfo]) -> Option<String> {
    IDS.iter().find_map(|&(vid, pid)| {
        ports
            .iter()
            .filter(|p| p.vendor_id == Some(vid) && p.product_id == Some(pid))
            .min_by_key(|p| !p.port_name.starts_with("/dev/cu."))
            .map(|p| p.port_name.clone())
    })
}

/// Finds and opens the 1.3 display on the first serial port whose VID/PID matches.
///
/// Reproduces `Sysctl::connect`/`Transport::open` (`FUN_140008300` →
/// `FUN_1400083d0`): enumerates the ports, matches by (VID, PID) against
/// [`IDS`], and opens at 1 Mbps / 8N1 / no flow control.
pub fn open_display() -> Result<SerialTransport> {
    let port = find_display_port()?.ok_or_else(|| {
        anyhow!(
            "no Kalkan 1.3 display found (looking for VID:PID {}){}",
            IDS.iter()
                .map(|(v, p)| format!("{v:04x}:{p:04x}"))
                .collect::<Vec<_>>()
                .join(" or "),
            not_found_hint(),
        )
    })?;
    SerialTransport::open_display(&port)
}

impl SerialTransport {
    /// Opens the port with the 1.3 display's exact parameters.
    ///
    /// From `FUN_1400083d0`: 1 Mbps, 8 data bits, no parity, 1 stop bit, no flow
    /// control (the `serialport` defaults are already 8N1/no flow control, but we
    /// spell it out to match the original byte for byte).
    pub fn open_display(path: &str) -> Result<Self> {
        let path = callout_path(path); // macOS: cu., not tty.
        let path = path.as_str();
        let port = serialport::new(path, BAUD)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|e| anyhow!(open_error_message(path, &e)))?;
        Ok(Self {
            port,
            label: format!("serial {path} @ {} (Kalkan 1.3)", BAUD),
        })
    }
}

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
/// the [`IDS`] (`"idVendor" = 6790` / `"idProduct" = 18506`).
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
        IDS.iter().any(|&(dv, dp)| v == Some(dv as u32) && p == Some(dp as u32))
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


/// Message for a failed attempt to open the display's port. On Linux a
/// permission error gets the fix: the udev rule or the `dialout` group.
fn open_error_message(path: &str, e: &serialport::Error) -> String {
    let denied = matches!(e.kind, serialport::ErrorKind::Io(std::io::ErrorKind::PermissionDenied))
        || e.description.to_lowercase().contains("permission denied");
    if denied && cfg!(target_os = "linux") {
        permission_message(path)
    } else {
        format!("failed to open 1.3 display on {path}: {e}")
    }
}

/// How to get access to the serial port on Linux.
fn permission_message(path: &str) -> String {
    format!(
        "Permission denied opening {path}. Install the udev rule \
         (60-kalkan-monitor.rules, included in the .deb) or add your user to the \
         \"dialout\" group (\"uucp\" on Arch): sudo usermod -aG dialout $USER. \
         Then replug the display (and log in again if you used the group)."
    )
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

    #[test]
    fn permission_error_explains_the_fix() {
        let e = serialport::Error::new(
            serialport::ErrorKind::Io(std::io::ErrorKind::PermissionDenied),
            "Permission denied",
        );
        let msg = open_error_message("/dev/ttyUSB0", &e);
        if cfg!(target_os = "linux") {
            assert!(msg.contains("udev") && msg.contains("dialout") && msg.contains("/dev/ttyUSB0"));
        } else {
            assert!(msg.starts_with("failed to open 1.3 display on"));
        }
        assert!(permission_message("/dev/ttyACM0").contains("replug"));
    }

    #[test]
    fn other_errors_keep_the_plain_message() {
        let e = serialport::Error::new(serialport::ErrorKind::NoDevice, "gone");
        assert!(open_error_message("COM5", &e).starts_with("failed to open 1.3 display on COM5"));
    }
}
