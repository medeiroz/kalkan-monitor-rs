//! Firmware-independent view of the cooler display: which firmware/bus a
//! device speaks ([`Profile`]) and the [`Display`] trait the app drives, so the
//! UI does not care how a frame reaches the hardware.

use crate::device::SerialDeviceInfo;
use crate::v1_2::HidDeviceInfo;
use anyhow::Result;
use std::fmt;

/// Firmware generation of the cooler's display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Firmware {
    V1_2,
    V1_3,
}

impl fmt::Display for Firmware {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Firmware::V1_2 => "1.2",
            Firmware::V1_3 => "1.3",
        })
    }
}

impl Firmware {
    /// Which products use this firmware, in user-friendly words.
    pub fn models_hint(&self) -> &'static str {
        match self {
            Firmware::V1_2 => "Alpha Rev.1 (sold before Jan/2026) \u{2014} experimental",
            Firmware::V1_3 => "Alpha Rev.2 (from Jan/2026) or Delta V2",
        }
    }
}

/// How a firmware reaches the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    Serial,
    Hid,
}

impl Bus {
    /// Short name for the UI/CLI.
    pub fn label(&self) -> &'static str {
        match self {
            Bus::Serial => "USB serial",
            Bus::Hid => "USB HID",
        }
    }
}

/// A known display: its firmware, bus and USB IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Profile {
    pub firmware: Firmware,
    pub bus: Bus,
    pub vid: u16,
    pub pid: u16,
}

const fn serial_1_3(ids: (u16, u16)) -> Profile {
    Profile { firmware: Firmware::V1_3, bus: Bus::Serial, vid: ids.0, pid: ids.1 }
}

/// Firmware 1.2 (Alpha Rev.1): USB HID `0416:1100`. Experimental.
const HID_1_2: Profile = Profile {
    firmware: Firmware::V1_2,
    bus: Bus::Hid,
    vid: crate::v1_2::VID,
    pid: crate::v1_2::PID,
};

static PROFILES: [Profile; 3] = [
    serial_1_3(crate::v1_3::IDS[0]),
    serial_1_3(crate::v1_3::IDS[1]),
    HID_1_2,
];

/// Every supported display, in the order they are tried: firmware 1.3 (USB
/// serial) first, then firmware 1.2 (USB HID).
pub fn profiles() -> &'static [Profile] {
    &PROFILES
}

/// The profile whose USB IDs match `vid:pid`, if any.
pub fn profile_for(vid: u16, pid: u16) -> Option<&'static Profile> {
    profiles().iter().find(|p| (p.vid, p.pid) == (vid, pid))
}

/// The three values shown on the display.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Fields {
    /// Temperature in °C.
    pub temp: u8,
    /// Fan speed in RPM.
    pub fan: u16,
    /// Usage in %. Unused by firmware 1.3.
    pub load: u8,
}

/// What a [`Display::send`] put on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    /// The frame before scrambling.
    pub raw: Vec<u8>,
    /// The bytes actually written to the device.
    pub wire: Vec<u8>,
    /// Frame counter, for firmware that has one (1.3 does not).
    pub index: Option<u8>,
}

/// An open display, whatever its firmware.
pub trait Display: Send {
    fn firmware(&self) -> Firmware;

    /// Human-readable channel name, for logs/UI.
    fn description(&self) -> String;

    /// Builds and sends one frame with `f`.
    fn send(&mut self, f: &Fields) -> Result<Sent>;
}

/// Where the display was found by [`find`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// A firmware 1.3 serial port (its name).
    Serial(String),
    /// Firmware 1.2 HID candidates (`0416:1100`), in the order to try them.
    Hid(Vec<HidDeviceInfo>),
}

/// No supported display is plugged in. The message lists both firmware
/// families and their USB IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotFound(pub String);

impl fmt::Display for NotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotFound {}

fn ids_text(ids: &[(u16, u16)]) -> String {
    ids.iter().map(|(v, p)| format!("{v:04x}:{p:04x}")).collect::<Vec<_>>().join(" or ")
}

/// The "nothing found" message. `hint` is the 1.3 serial hint (macOS CH34x
/// driver / cable, empty elsewhere); `hid_error` is set when HID enumeration
/// itself failed.
fn not_found_message(hint: &str, hid_error: Option<&str>) -> String {
    let hid = hid_error.map(|e| format!(" (HID enumeration failed: {e})")).unwrap_or_default();
    format!(
        "no supported Kalkan display found (firmware 1.3: USB serial {}; \
         firmware 1.2, experimental: USB HID {}){hid}{hint}",
        ids_text(crate::v1_3::IDS),
        ids_text(crate::v1_2::IDS),
    )
}

/// Picks the display to open from the system's device lists.
///
/// Priority is deterministic: a firmware 1.3 serial port wins (picked exactly
/// as before 1.2 support existed, see `v1_3::serial`), and `hid` is only
/// called when no 1.3 port is present, so 1.3 users never pay for an HID
/// enumeration. Then the 1.2 HID candidates. Only one cooler is expected to be
/// plugged in at a time; nothing else is done for the "both present" case.
///
/// Errors: when nothing is found, the serial enumeration error if there was
/// one (the pre-1.2 behavior), otherwise a [`NotFound`].
pub fn find(
    serial: Result<Vec<SerialDeviceInfo>>,
    hid: impl FnOnce() -> Result<Vec<HidDeviceInfo>>,
    hint: impl FnOnce() -> String,
) -> Result<Found> {
    if let Ok(ports) = &serial {
        if let Some(port) = crate::v1_3::serial::pick_display_port(ports) {
            return Ok(Found::Serial(port));
        }
    }
    let (candidates, hid_error) = match hid() {
        Ok(list) => (crate::v1_2::hid::candidates(&list), None),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };
    if !candidates.is_empty() {
        return Ok(Found::Hid(candidates));
    }
    serial?;
    Err(NotFound(not_found_message(&hint(), hid_error.as_deref())).into())
}

/// Finds and opens the supported display: firmware 1.3 (serial) first, then
/// firmware 1.2 (HID, after a successful handshake).
///
/// If a display is found but fails to open (e.g. permission denied, or a 1.2
/// handshake that fails) that error is returned; there is no fallback to the
/// other firmware. Blocking: call it off the UI thread.
pub fn open() -> Result<Box<dyn Display>> {
    let found = find(
        crate::device::available_serial_ports(),
        crate::v1_2::find_candidates,
        crate::v1_3::serial::not_found_hint,
    )?;
    Ok(match found {
        Found::Serial(port) => {
            let transport = crate::transport::SerialTransport::open_display(&port)?;
            Box::new(crate::v1_3::V1_3Display::new(transport))
        }
        Found::Hid(candidates) => Box::new(crate::v1_2::open_display(&candidates)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_have_no_duplicate_ids() {
        let ps = profiles();
        assert!(!ps.is_empty());
        for (i, a) in ps.iter().enumerate() {
            assert!(
                ps[i + 1..].iter().all(|b| (a.vid, a.pid) != (b.vid, b.pid)),
                "duplicate profile {:04x}:{:04x}",
                a.vid,
                a.pid
            );
        }
    }

    #[test]
    fn profiles_match_the_1_3_ids() {
        let serial: Vec<_> = profiles().iter().filter(|p| p.bus == Bus::Serial).collect();
        let ids: Vec<_> = serial.iter().map(|p| (p.vid, p.pid)).collect();
        assert_eq!(ids, crate::v1_3::IDS);
        assert!(serial.iter().all(|p| p.firmware == Firmware::V1_3));
    }

    #[test]
    fn profiles_have_the_1_2_hid_after_the_1_3_ones() {
        let ps = profiles();
        assert_eq!(ps.len(), 3);
        assert_eq!(ps[2], Profile { firmware: Firmware::V1_2, bus: Bus::Hid, vid: 0x0416, pid: 0x1100 });
        assert!(ps[..2].iter().all(|p| p.firmware == Firmware::V1_3 && p.bus == Bus::Serial));
        assert_eq!(profile_for(0x0416, 0x1100).map(|p| p.bus), Some(Bus::Hid));
        assert_eq!(Bus::Hid.label(), "USB HID");
    }

    #[test]
    fn no_id_overlap_between_firmware_families() {
        for id in crate::v1_2::IDS {
            assert!(!crate::v1_3::IDS.contains(id), "{id:04x?} is in both families");
        }
    }

    // -- selection (`find`) with injected device lists ----------------------

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

    fn hid(path: &str, vid: u16, pid: u16) -> HidDeviceInfo {
        HidDeviceInfo { vendor_id: vid, product_id: pid, path: path.into(), ..Default::default() }
    }

    fn no_hid() -> Result<Vec<HidDeviceInfo>> {
        panic!("HID must not be enumerated when a 1.3 port is present")
    }

    fn hint() -> String {
        String::new()
    }

    #[test]
    fn a_1_3_port_wins_and_hid_is_not_enumerated() {
        let ports = vec![port("COM3", 0x1234, 0x5678), port("COM5", 0x1A86, 0x484A)];
        assert_eq!(find(Ok(ports), no_hid, hint).unwrap(), Found::Serial("COM5".into()));
        // Even with a 1.2 device plugged in too: 1.3 first, deterministically.
        let ports = vec![port("COM4", 0x1F3A, 0x0008)];
        let both = || Ok(vec![hid("h", 0x0416, 0x1100)]);
        assert_eq!(find(Ok(ports), both, hint).unwrap(), Found::Serial("COM4".into()));
    }

    #[test]
    fn serial_only_input_picks_the_same_port_as_before() {
        let fixtures = vec![
            vec![port("/dev/tty.usbserial-1410", 0x1A86, 0x484A), port("/dev/cu.usbserial-1410", 0x1A86, 0x484A)],
            vec![port("COM3", 0x1A86, 0x484A), port("COM4", 0x1F3A, 0x0008)],
            vec![port("/dev/ttyUSB0", 0x1A86, 0x484A)],
            vec![port("/dev/ttyACM0", 0x1F3A, 0x0008), port("/dev/ttyUSB1", 0x0403, 0x6001)],
        ];
        for ports in fixtures {
            let before = crate::v1_3::serial::pick_display_port(&ports).unwrap();
            assert_eq!(find(Ok(ports), no_hid, hint).unwrap(), Found::Serial(before));
        }
    }

    #[test]
    fn the_1_2_display_is_chosen_when_only_hid_is_present() {
        let ports = vec![port("COM3", 0x1234, 0x5678)];
        let list = || {
            Ok(vec![hid("other", 0x0416, 0x5020), hid("kalkan", 0x0416, 0x1100), hid("mouse", 0x046d, 0xc077)])
        };
        assert_eq!(find(Ok(ports), list, hint).unwrap(), Found::Hid(vec![hid("kalkan", 0x0416, 0x1100)]));
        // Also when there are no serial ports at all, or their enumeration failed.
        let one = || Ok(vec![hid("k", 0x0416, 0x1100)]);
        assert!(matches!(find(Ok(vec![]), one, hint).unwrap(), Found::Hid(_)));
        let one = || Ok(vec![hid("k", 0x0416, 0x1100)]);
        assert!(matches!(find(Err(anyhow::anyhow!("no serial")), one, hint).unwrap(), Found::Hid(_)));
    }

    #[test]
    fn nothing_found_lists_both_firmware_families() {
        let other_nuvoton = || Ok(vec![hid("x", 0x0416, 0x5020)]);
        let e = find(Ok(vec![port("COM3", 0x1234, 0x5678)]), other_nuvoton, || " HINT".into()).unwrap_err();
        let nf = e.downcast_ref::<NotFound>().expect("a NotFound error");
        for text in ["firmware 1.3", "1f3a:0008 or 1a86:484a", "firmware 1.2", "0416:1100", "HINT"] {
            assert!(nf.0.contains(text), "{text} missing from {nf}");
        }
        assert!(!nf.0.contains("HID enumeration failed"));
    }

    #[test]
    fn hid_enumeration_errors_are_reported_but_not_fatal() {
        let broken = || Err(anyhow::anyhow!("hidapi init failed"));
        let e = find(Ok(vec![]), broken, hint).unwrap_err();
        assert!(e.downcast_ref::<NotFound>().is_some());
        assert!(e.to_string().contains("HID enumeration failed: hidapi init failed"));
    }

    #[test]
    fn serial_enumeration_errors_still_surface_when_nothing_is_found() {
        let e = find(Err(anyhow::anyhow!("serial broken")), || Ok(vec![]), hint).unwrap_err();
        assert_eq!(e.to_string(), "serial broken");
    }

    #[test]
    fn the_hint_is_only_computed_when_nothing_is_found() {
        let ports = vec![port("COM5", 0x1A86, 0x484A)];
        let r = find(Ok(ports), no_hid, || panic!("hint not needed"));
        assert!(r.is_ok());
        let r = find(Ok(vec![]), || Ok(vec![hid("k", 0x0416, 0x1100)]), || panic!("hint not needed"));
        assert!(r.is_ok());
    }

    #[test]
    fn models_hint_and_profile_lookup() {
        assert_eq!(Firmware::V1_3.models_hint(), "Alpha Rev.2 (from Jan/2026) or Delta V2");
        assert!(Firmware::V1_2.models_hint().contains("experimental"));
        let (v, p) = crate::v1_3::IDS[0];
        assert_eq!(profile_for(v, p).map(|p| p.firmware), Some(Firmware::V1_3));
        assert!(profile_for(0xffff, 0xffff).is_none());
        assert_eq!(Bus::Serial.label(), "USB serial");
    }

    #[test]
    fn firmware_names() {
        assert_eq!(Firmware::V1_2.to_string(), "1.2");
        assert_eq!(Firmware::V1_3.to_string(), "1.3");
    }
}
