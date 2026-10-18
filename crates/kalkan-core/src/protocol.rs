//! Display application protocol — packet format and scrambling.
//!
//! **Version 1.3.** Fully reconstructed by static reverse engineering
//! (Ghidra via REA) of the `Sysctl` class in the binary
//! `Kalkan PC SmartMonitor DZ V1.3.exe`. See `docs/protocol.md` for the
//! addresses and the source pseudocode.
//!
//! The 1.3 protocol is **much simpler** than the 1.2 one and **incompatible**
//! with it:
//!   * **No handshake / no seed** — the host just opens the serial port and sends frames.
//!   * **5-byte frame** `[0x74, 0x00, temp, fanHi, fanLo]` (no `cpu_load` and
//!     no frame index, which existed in the 1.2).
//!   * **Trivial scrambling**: from byte 2 on, each byte is XORed with
//!     `0x02`; the first two bytes (header `0x74 0x00`) go out literal.
//!
//! Source (`Sysctl::processTrans`, `FUN_1400062b0`):
//! ```text
//! buf.clear();
//! buf.append('t'); buf.append('\0');      // 0x74 0x00
//! buf.append(temp);                        // 1 byte
//! buf.append(fan >> 8); buf.append(fan);   // big-endian
//! for (i = 2; i < buf.size(); i++) buf[i] ^= 0x02;
//! emitSendData(buf);                       // -> Transport escreve na serial
//! ```

use crate::sensors::Reading;

/// Size of the frame sent to the display — 5 bytes.
pub const FRAME_LEN: usize = 5;

/// Literal frame header (`'t'`, `0x00`), not scrambled.
pub const HEADER: [u8; 2] = [0x74, 0x00];

/// XOR byte applied from index 2 onward (the 1.3 "scrambler").
pub const SCRAMBLE_XOR: u8 = 0x02;

/// Builds the raw data frame (before the XOR): `Sysctl::processTrans`.
///
/// Layout (5 bytes):
/// | offset | content |
/// |--------|----------|
/// | 0 | `0x74` (`'t'`) |
/// | 1 | `0x00` |
/// | 2 | `temp` (°C, 1 byte) |
/// | 3 | `fan` high byte (big-endian) |
/// | 4 | `fan` low byte |
pub fn build_frame(temp: u8, fan: u16) -> [u8; FRAME_LEN] {
    [HEADER[0], HEADER[1], temp, (fan >> 8) as u8, fan as u8]
}

/// Scrambles the frame: XOR with `0x02` from byte 2 to the end; header literal.
///
/// Faithful port of the loop seen in `FUN_1400062b0` (the original XORs from
/// `data + 2`). The first two bytes (`0x74 0x00`) are preserved.
pub fn scramble(frame: &[u8]) -> Vec<u8> {
    let mut out = frame.to_vec();
    for b in out.iter_mut().skip(HEADER.len()) {
        *b ^= SCRAMBLE_XOR;
    }
    out
}

/// Converts a temperature in °C to the frame byte (saturates to 0..=255).
fn temp_byte(reading: &Reading) -> u8 {
    reading
        .cpu_temp_c
        .map(|t| t.round().clamp(0.0, 255.0) as u8)
        .unwrap_or(0)
}

/// Converts RPM to the frame's 2-byte field (saturates at 0xFFFF).
fn fan_word(reading: &Reading) -> u16 {
    reading.fan_rpm.map(|f| f.min(0xFFFF) as u16).unwrap_or(0)
}

/// Builds the packets to send to the display from a sensor reading.
pub trait PacketBuilder: Send {
    /// Produces zero or more packets ready for `Transport::write`.
    fn build(&mut self, reading: &Reading) -> Vec<Vec<u8>>;

    /// Frame size expected by the device.
    fn frame_len(&self) -> usize;
}

/// Packet builder for the real Kalkan SmartMonitor **1.3** protocol.
///
/// Stateless: the 1.3 has no seed or frame counter. For each reading, it
/// builds the 5-byte frame and applies the XOR.
#[derive(Debug, Default, Clone, Copy)]
pub struct KalkanBuilder;

impl KalkanBuilder {
    pub fn new() -> Self {
        Self
    }
}

impl PacketBuilder for KalkanBuilder {
    fn build(&mut self, reading: &Reading) -> Vec<Vec<u8>> {
        let frame = build_frame(temp_byte(reading), fan_word(reading));
        vec![scramble(&frame)]
    }

    fn frame_len(&self) -> usize {
        FRAME_LEN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_layout() {
        let f = build_frame(42, 0x0ABC);
        assert_eq!(f, [0x74, 0x00, 42, 0x0A, 0xBC]);
    }

    #[test]
    fn scramble_preserves_header() {
        let f = build_frame(40, 1200);
        let s = scramble(&f);
        assert_eq!(&s[..2], &[0x74, 0x00]); // literal header
        assert_eq!(s.len(), FRAME_LEN);
    }

    #[test]
    fn scramble_regression_vector() {
        // 1.3 regression vector, checked against FUN_1400062b0:
        // temp=40 (0x28), fan=1200 (0x04B0).
        // raw       : 74 00 28 04 B0
        // scrambled : 74 00 2A 06 B2   (bytes 2.. XORed with 0x02)
        let f = build_frame(40, 1200);
        assert_eq!(f, [0x74, 0x00, 0x28, 0x04, 0xB0]);
        assert_eq!(scramble(&f), vec![0x74, 0x00, 0x2A, 0x06, 0xB2]);
    }

    #[test]
    fn builder_uses_sensor_reading() {
        let mut b = KalkanBuilder::new();
        let r = Reading {
            cpu_temp_c: Some(40.0),
            fan_rpm: Some(1200),
            ..Default::default()
        };
        let pkts = b.build(&r);
        assert_eq!(pkts.len(), 1);
        assert_eq!(pkts[0], vec![0x74, 0x00, 0x2A, 0x06, 0xB2]);
    }
}
