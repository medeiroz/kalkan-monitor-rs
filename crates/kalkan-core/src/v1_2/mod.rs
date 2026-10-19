//! Firmware 1.2 (Alpha Rev.1, sold before Jan/2026): USB HID `0416:1100`,
//! seed handshake, 64-byte scrambled frame. **Experimental, untested on
//! hardware**: everything comes from static analysis of the vendor app (see
//! `docs/v1.2/protocol.md`).

pub mod protocol;
