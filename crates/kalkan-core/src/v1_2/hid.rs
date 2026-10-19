//! Firmware 1.2 USB HID link: the display's IDs, HID enumeration, candidate
//! filtering and the [`HidTransport`].
//!
//! The vendor app opens the cooler with `hid_open(0x0416, 0x1100, NULL)`
//! (`FUN_140002cb0` → `FUN_140001a70`): first device with those IDs, no serial
//! number, no usage-page or interface filter. `0416` is a generic Nuvoton
//! vendor ID shared by many unrelated devices, so this app enumerates, keeps
//! only `0416:1100`, opens each candidate **by path** and only uses one whose
//! handshake succeeds (see [`super::display`]).
//!
//! All HID I/O is blocking and must run off the UI thread (the UI runs it on
//! its device worker thread).
//!
//! macOS: hidapi opens HID devices **exclusively** by default, so the official
//! app (or any other program holding the device) makes the open fail, and the
//! other way round. No TCC permission is needed for a vendor-defined HID
//! device. Windows: HID handles are shared, so two apps writing at once would
//! interleave frames; the official software must be closed.

use crate::transport::Transport;
use anyhow::{anyhow, Result};
use std::ffi::CString;
use std::time::Duration;

/// VID of the 1.2 display (generic Nuvoton ID; hardcoded in the vendor exe).
pub const VID: u16 = 0x0416;
/// PID of the 1.2 display (hardcoded in the vendor exe).
pub const PID: u16 = 0x1100;
/// (VID, PID) pairs of the 1.2 display.
pub const IDS: &[(u16, u16)] = &[(VID, PID)];

/// An HID device seen in the system enumeration (cross-platform).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HidDeviceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
    /// Platform path, as hidapi reports it (opened with `open_path`).
    pub path: String,
    /// HID usage page of this top-level collection (0 when unknown).
    pub usage_page: u16,
    /// USB interface number (-1 when unknown).
    pub interface_number: i32,
}

impl HidDeviceInfo {
    /// `true` if this device has the 1.2 display's VID:PID.
    pub fn is_display(&self) -> bool {
        IDS.contains(&(self.vendor_id, self.product_id))
    }

    /// The supported-display profile (firmware/bus) this device matches, if any.
    pub fn profile(&self) -> Option<&'static crate::Profile> {
        crate::profile_for(self.vendor_id, self.product_id)
            .filter(|p| p.bus == crate::Bus::Hid)
    }
}

/// Lists all HID devices on the system (also used by `kalkan hid`).
pub fn enumerate() -> Result<Vec<HidDeviceInfo>> {
    let api = hidapi::HidApi::new()?;
    Ok(api.device_list().map(info).collect())
}

fn info(d: &hidapi::DeviceInfo) -> HidDeviceInfo {
    HidDeviceInfo {
        vendor_id: d.vendor_id(),
        product_id: d.product_id(),
        manufacturer: d.manufacturer_string().map(str::to_owned),
        product: d.product_string().map(str::to_owned),
        serial_number: d.serial_number().map(str::to_owned),
        path: d.path().to_string_lossy().into_owned(),
        usage_page: d.usage_page(),
        interface_number: d.interface_number(),
    }
}

/// The 1.2 display candidates among `devices`, in the order they are tried.
///
/// Only `0416:1100` entries with a path are kept. A device can show up once
/// per top-level collection / interface; the real unit's layout is unknown,
/// so vendor-defined usage pages (`0xFF00..`) are tried first, then the rest
/// in enumeration order (the order the vendor's `hid_open` would see).
pub fn candidates(devices: &[HidDeviceInfo]) -> Vec<HidDeviceInfo> {
    let mut c: Vec<HidDeviceInfo> =
        devices.iter().filter(|d| d.is_display() && !d.path.is_empty()).cloned().collect();
    c.sort_by_key(|d| d.usage_page < 0xFF00); // stable: keeps enumeration order otherwise
    c
}

/// Enumerates the HID devices and returns the 1.2 display candidates.
pub fn find_candidates() -> Result<Vec<HidDeviceInfo>> {
    Ok(candidates(&enumerate()?))
}

/// Heuristic to highlight "cooler display" candidates in an HID list.
pub fn looks_like_display(info: &HidDeviceInfo) -> bool {
    if info.is_display() {
        return true;
    }
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

/// USB HID transport (hidapi).
///
/// Reverse-engineering note: the original calls `hid_set_nonblocking(dev, 0)`
/// right before each `hid_write`, i.e. a **blocking** write; this transport
/// stays in blocking mode. Reads always use `hid_read_timeout`.
pub struct HidTransport {
    device: hidapi::HidDevice,
    label: String,
}

impl HidTransport {
    /// Opens the device at `path` (as listed by [`enumerate`]).
    pub fn open_path(api: &hidapi::HidApi, path: &str, label: String) -> Result<Self> {
        let c = CString::new(path).map_err(|_| anyhow!("invalid HID path {path:?}"))?;
        let device = api.open_path(&c).map_err(|e| anyhow!(open_error_message(path, &e)))?;
        device.set_blocking_mode(true)?; // same as hid_set_nonblocking(dev, 0)
        Ok(Self { device, label })
    }

    /// Opens the first device whose (vid, pid) matches, like the vendor's
    /// `hid_open(vid, pid, NULL)`. Diagnostics only; the app opens by path.
    pub fn open(api: &hidapi::HidApi, vid: u16, pid: u16) -> Result<Self> {
        let device = api
            .open(vid, pid)
            .map_err(|e| anyhow!("failed to open HID {vid:04x}:{pid:04x}: {e}"))?;
        device.set_blocking_mode(true)?;
        Ok(Self { device, label: format!("HID {vid:04x}:{pid:04x}") })
    }
}

impl Transport for HidTransport {
    fn write(&mut self, data: &[u8]) -> Result<usize> {
        // hidapi takes byte 0 as the report ID. The 1.2 frames start with
        // 0x01, written as-is exactly like the vendor app does.
        Ok(self.device.write(data)?)
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize> {
        let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        Ok(self.device.read_timeout(buf, ms)?)
    }

    fn description(&self) -> String {
        self.label.clone()
    }
}

/// Message for a failed open. On Linux a permission error gets the fix
/// (the hidraw udev rule), not the serial `dialout` advice.
fn open_error_message(path: &str, e: &hidapi::HidError) -> String {
    let text = e.to_string();
    let denied = text.to_lowercase().contains("permission denied");
    if denied && cfg!(target_os = "linux") {
        permission_message(path)
    } else {
        format!("failed to open 1.2 display (HID {VID:04x}:{PID:04x}) at {path}: {text}")
    }
}

/// How to get access to the hidraw node on Linux.
fn permission_message(path: &str) -> String {
    format!(
        "Permission denied opening the 1.2 display (HID {VID:04x}:{PID:04x}, {path}). \
         Install the udev rule (60-kalkan-monitor.rules, included in the .deb) that grants \
         access to its /dev/hidraw* node, then run \"sudo udevadm control --reload-rules && \
         sudo udevadm trigger --subsystem-match=hidraw\" or replug the display."
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn dev(vid: u16, pid: u16, path: &str, usage_page: u16) -> HidDeviceInfo {
        HidDeviceInfo {
            vendor_id: vid,
            product_id: pid,
            path: path.into(),
            usage_page,
            interface_number: 0,
            ..Default::default()
        }
    }

    #[test]
    fn candidates_keep_only_the_display_ids() {
        let list = [
            dev(0x0416, 0x5020, "a", 0xFF00), // other Nuvoton device
            dev(0x046d, 0xc077, "b", 0x0001), // a mouse
            dev(0x0416, 0x1100, "c", 0x0001),
            dev(0x0416, 0x1100, "", 0xFF00), // no path: cannot be opened by path
        ];
        let c = candidates(&list);
        assert_eq!(c.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(), ["c"]);
        assert!(candidates(&list[..2]).is_empty());
        assert!(candidates(&[]).is_empty());
    }

    #[test]
    fn candidates_try_vendor_usage_pages_first_then_enumeration_order() {
        let list = [
            dev(0x0416, 0x1100, "kbd", 0x0001),
            dev(0x0416, 0x1100, "vendor", 0xFF00),
            dev(0x0416, 0x1100, "cons", 0x000C),
            dev(0x0416, 0x1100, "vendor2", 0xFFA0),
        ];
        let order: Vec<_> = candidates(&list).into_iter().map(|d| d.path).collect();
        assert_eq!(order, ["vendor", "vendor2", "kbd", "cons"]);
    }

    #[test]
    fn display_marks_and_profile() {
        let d = dev(VID, PID, "p", 0);
        assert!(d.is_display() && looks_like_display(&d));
        assert_eq!(d.profile().map(|p| p.firmware), Some(crate::Firmware::V1_2));
        let other = dev(0x0416, 0x5020, "p", 0);
        assert!(!other.is_display() && other.profile().is_none());
    }

    #[test]
    fn linux_permission_message_mentions_hidraw_not_dialout() {
        let m = permission_message("/dev/hidraw3");
        assert!(m.contains("hidraw") && m.contains("udev") && m.contains("/dev/hidraw3"));
        assert!(!m.contains("dialout"));
    }
}
