//! Firmware-independent view of the cooler display: which firmware/bus a
//! device speaks ([`Profile`]) and the [`Display`] trait the app drives, so the
//! UI does not care how a frame reaches the hardware.

use anyhow::Result;
use std::fmt;

/// Firmware generation of the cooler's display.
#[allow(dead_code)] // `V1_2` has no implementation yet
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
            Firmware::V1_2 => "Alpha (purchased before Jan/2026) \u{2014} experimental",
            Firmware::V1_3 => "Alpha (purchased from Jan/2026) or Delta V2",
        }
    }
}

/// How a firmware reaches the device.
#[allow(dead_code)] // `Hid` goes with firmware 1.2
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

static PROFILES: [Profile; 2] = [
    serial_1_3(crate::v1_3::IDS[0]),
    serial_1_3(crate::v1_3::IDS[1]),
];

/// Every supported display, in the order they are tried.
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

/// Finds and opens the first supported display. Only firmware 1.3 for now.
pub fn open() -> Result<Box<dyn Display>> {
    let transport = crate::v1_3::open_display()?;
    Ok(Box::new(crate::v1_3::V1_3Display::new(transport)))
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
        let ids: Vec<_> = profiles().iter().map(|p| (p.vid, p.pid)).collect();
        assert_eq!(ids, crate::v1_3::IDS);
        assert!(profiles().iter().all(|p| p.firmware == Firmware::V1_3 && p.bus == Bus::Serial));
    }

    #[test]
    fn models_hint_and_profile_lookup() {
        assert_eq!(Firmware::V1_3.models_hint(), "Alpha (purchased from Jan/2026) or Delta V2");
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
