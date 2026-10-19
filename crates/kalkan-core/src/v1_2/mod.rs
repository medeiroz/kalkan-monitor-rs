//! Firmware 1.2 (Alpha Rev.1, sold before Jan/2026): USB HID `0416:1100`,
//! seed handshake, 64-byte scrambled frame. **Experimental, untested on
//! hardware**: everything comes from static analysis of the vendor app (see
//! `docs/v1.2/protocol.md`).

pub mod display;
pub mod hid;
pub mod protocol;

pub use display::{HandshakeError, V1_2Display};
pub use hid::{find_candidates, HidDeviceInfo, HidTransport, IDS, PID, VID};

use anyhow::{anyhow, Result};

/// Label of an open 1.2 display (the HID path is long and platform-specific).
pub fn label() -> String {
    format!("USB HID {VID:04x}:{PID:04x} (Kalkan 1.2)")
}

/// Opens one candidate by path, without any handshake (diagnostics:
/// `kalkan handshake`).
pub fn open_candidate(candidate: &HidDeviceInfo) -> Result<HidTransport> {
    let api = hidapi::HidApi::new()?;
    HidTransport::open_path(&api, &candidate.path, label())
}

/// Opens the first of `candidates` (see [`hid::candidates`]) that opens by
/// path **and** completes the handshake. Returns the first candidate's error
/// when none does (the first one is the most likely to be the display).
///
/// Blocking (up to [`display::HANDSHAKE_TIMEOUT`] per candidate): call it off
/// the UI thread.
pub fn open_display(candidates: &[HidDeviceInfo]) -> Result<V1_2Display<HidTransport>> {
    let api = hidapi::HidApi::new()?;
    let mut first_err = None;
    for c in candidates {
        match HidTransport::open_path(&api, &c.path, label()).and_then(V1_2Display::open) {
            Ok(d) => return Ok(d),
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    Err(first_err.unwrap_or_else(|| anyhow!("no 1.2 display (HID {VID:04x}:{PID:04x}) found")))
}
