//! Transport layers for the watercooler display.
//!
//! The **1.3** talks to the device over a **serial port only** (`QSerialPort`,
//! class `Transport` in the binary; see `docs/protocol.md`). Use
//! [`SerialTransport::open_display`] for the cooler's exact parameters.
//!
//! [`HidTransport`] is kept as a 1.2 legacy / diagnostics path; the 1.3 does
//! not use it. Both sit behind a single `Transport` trait.

use anyhow::{anyhow, Result};
use std::time::Duration;

/// An already-open channel able to send packets to the display and read replies.
pub trait Transport: Send {
    /// Sends a packet (an HID "report" or a serial frame) to the device.
    fn write(&mut self, data: &[u8]) -> Result<usize>;

    /// Reads up to `buf.len()` bytes, honoring `timeout`. Returns how many were read.
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize>;

    /// Human-readable channel name, for logs/UI.
    fn description(&self) -> String;
}

/// USB HID transport.
///
/// Reverse-engineering note: the original calls `hid_set_nonblocking(dev, 0)`
/// right before each `hid_write`, i.e. a **blocking** write. We replicate that
/// behavior to match the protocol timing.
pub struct HidTransport {
    device: hidapi::HidDevice,
    label: String,
}

impl HidTransport {
    /// Opens the first device whose (vid, pid) matches the request —
    /// same strategy as `HidDeviceController::isOpen` (enumerates and compares
    /// vid at `dev_info+8` and pid at `dev_info+10`).
    pub fn open(api: &hidapi::HidApi, vid: u16, pid: u16) -> Result<Self> {
        let device = api
            .open(vid, pid)
            .map_err(|e| anyhow!("failed to open HID {vid:04x}:{pid:04x}: {e}"))?;
        device.set_blocking_mode(true)?; // equivale a hid_set_nonblocking(dev, 0)
        Ok(Self {
            device,
            label: format!("HID {vid:04x}:{pid:04x}"),
        })
    }
}

impl Transport for HidTransport {
    fn write(&mut self, data: &[u8]) -> Result<usize> {
        // hidapi requires byte 0 to be the Report ID. The caller must build the
        // buffer with the report id already in front (see `protocol`).
        Ok(self.device.write(data)?)
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize> {
        Ok(self
            .device
            .read_timeout(buf, timeout.as_millis() as i32)?)
    }

    fn description(&self) -> String {
        self.label.clone()
    }
}

/// Serial / COM transport, for the models that use `QSerialPort`.
pub struct SerialTransport {
    pub(crate) port: Box<dyn serialport::SerialPort>,
    pub(crate) label: String,
}

impl SerialTransport {
    pub fn open(path: &str, baud: u32) -> Result<Self> {
        let port = serialport::new(path, baud)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|e| anyhow!("failed to open serial {path} @ {baud}: {e}"))?;
        Ok(Self {
            port,
            label: format!("serial {path} @ {baud}"),
        })
    }
}

impl Transport for SerialTransport {
    fn write(&mut self, data: &[u8]) -> Result<usize> {
        use std::io::Write;
        Ok(self.port.write(data)?)
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize> {
        use std::io::Read;
        self.port.set_timeout(timeout)?;
        match self.port.read(buf) {
            Ok(n) => Ok(n),
            Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(0),
            Err(e) => Err(e.into()),
        }
    }

    fn description(&self) -> String {
        self.label.clone()
    }
}
