//! Firmware 1.2 [`Display`]: seed handshake on open, then one scrambled
//! 64-byte frame per [`Display::send`], optionally followed by a read-back.
//!
//! Implementation decisions (the vendor app's behavior is in
//! `docs/v1.2/protocol.md`; none of this is tested on real hardware):
//!
//! * **Missing `0xE3` is a failure.** The vendor app logs
//!   `[open] Can't found 0xE3`, closes the handle and still reports "open"
//!   with seed 0. Here a reply without the marker (or no reply within
//!   [`HANDSHAKE_TIMEOUT`]) is a [`HandshakeError`]: no frame is streamed to a
//!   device that did not complete the handshake (`0416` is a generic Nuvoton
//!   VID, so the device might not even be a cooler).
//! * **The frame index restarts at 0 on every new handshake** (the vendor
//!   keeps counting across reopens). It is a `u8` that wraps at 256.
//! * **Read-back after each write** mirrors the vendor (which reads 65 bytes,
//!   blocking up to 2000 ms, and only logs the reply). Whether the device needs
//!   it is unknown, so it is configurable ([`READ_BACK`]) and uses a short
//!   timeout ([`READ_BACK_TIMEOUT`]) so the device thread never stalls for
//!   long; an empty read, a timeout or a read error are ignored.
//! * **A failed write drops the session**: the next `send` re-handshakes (fresh
//!   seed, index back to 0). The UI worker also drops and reopens the device
//!   after a write error, which re-handshakes on a new handle.
//! * **Unplug detection** relies on the write failing; the vendor instead
//!   re-enumerates (`isOpen`, `hid_enumerate`) before every frame.

use super::protocol::{parse_seed, seed_request, Builder, READ_LEN, REPORT_LEN};
use crate::firmware::{Display, Fields, Firmware, Sent};
use crate::transport::Transport;
use anyhow::Result;
use std::fmt;
use std::time::Duration;

/// How long to wait for the handshake reply (the vendor's
/// `hid_read_timeout(…, 2000)`).
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(2000);

/// Whether to read back after each frame, like the vendor app. The reply is
/// ignored; set to `false` (or use [`V1_2Display::set_read_back`]) if real
/// hardware turns out not to need it.
pub const READ_BACK: bool = true;

/// Read-back timeout. The vendor waits up to 2000 ms; a short one keeps the
/// 1 s send cadence even when the device stays silent.
pub const READ_BACK_TIMEOUT: Duration = Duration::from_millis(150);

/// The device did not complete the seed handshake (no reply, no `0xE3`
/// marker, or an I/O error during it). The UI backs off on this error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeError(pub String);

impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "1.2 handshake failed: {}", self.0)
    }
}

impl std::error::Error for HandshakeError {}

/// Raw result of one handshake exchange, for diagnostics (`kalkan handshake`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
    /// What was written (the 64-byte seed request).
    pub request: [u8; REPORT_LEN],
    /// What came back (empty on timeout).
    pub reply: Vec<u8>,
    /// The parsed seed, if the reply had the `0xE3` marker.
    pub seed: Option<u8>,
}

/// Writes the seed request and reads one reply (up to 65 bytes). I/O errors
/// are returned; a missing marker is not (see [`handshake`]).
pub fn exchange(t: &mut dyn Transport, timeout: Duration) -> Result<Exchange> {
    let request = seed_request();
    t.write(&request)?;
    let mut buf = [0u8; READ_LEN];
    let n = t.read(&mut buf, timeout)?;
    let reply = buf[..n].to_vec();
    let seed = parse_seed(&reply);
    Ok(Exchange { request, reply, seed })
}

/// Performs the handshake and returns the scrambler seed. Every failure is a
/// [`HandshakeError`].
pub fn handshake(t: &mut dyn Transport, timeout: Duration) -> Result<u8> {
    let ex = exchange(t, timeout).map_err(|e| HandshakeError(format!("{e}")))?;
    match ex.seed {
        Some(seed) => Ok(seed),
        None if ex.reply.is_empty() => Err(HandshakeError(format!(
            "no reply within {} ms (is it a Kalkan 1.2 display? is the official software running?)",
            timeout.as_millis()
        ))
        .into()),
        None => Err(HandshakeError(format!(
            "magic byte 0xE3 not found in the {}-byte reply ({})",
            ex.reply.len(),
            super::protocol::hex(&ex.reply[..ex.reply.len().min(8)]),
        ))
        .into()),
    }
}

/// A 1.2 display over any [`Transport`] (USB HID in production).
pub struct V1_2Display<T: Transport> {
    transport: T,
    /// `None` until a handshake succeeds (and again after a write error).
    builder: Option<Builder>,
    read_back: bool,
}

impl<T: Transport> V1_2Display<T> {
    /// Takes an open transport and performs the handshake.
    pub fn open(transport: T) -> Result<Self> {
        let mut d = Self { transport, builder: None, read_back: READ_BACK };
        d.handshake()?;
        Ok(d)
    }

    /// (Re)does the handshake: new seed, frame index back to 0.
    pub fn handshake(&mut self) -> Result<u8> {
        self.builder = None;
        let seed = handshake(&mut self.transport, HANDSHAKE_TIMEOUT)?;
        self.builder = Some(Builder::new(seed));
        Ok(seed)
    }

    /// The current session's seed, if a handshake succeeded.
    pub fn seed(&self) -> Option<u8> {
        self.builder.map(|b| b.seed())
    }

    /// Turns the read-back after each frame on or off (default [`READ_BACK`]).
    pub fn set_read_back(&mut self, on: bool) {
        self.read_back = on;
    }
}

impl<T: Transport> Display for V1_2Display<T> {
    fn firmware(&self) -> Firmware {
        Firmware::V1_2
    }

    fn description(&self) -> String {
        self.transport.description()
    }

    fn send(&mut self, f: &Fields) -> Result<Sent> {
        if self.builder.is_none() {
            self.handshake()?;
        }
        let builder = self.builder.as_mut().expect("handshake sets the builder");
        let (index, raw, wire) = builder.next(f.temp, f.fan, f.load);
        if let Err(e) = self.transport.write(&wire) {
            self.builder = None; // the next send re-handshakes
            return Err(e);
        }
        if self.read_back {
            // Only logged by the vendor; anything here (data, nothing, an
            // error) is ignored.
            let mut buf = [0u8; READ_LEN];
            let _ = self.transport.read(&mut buf, READ_BACK_TIMEOUT);
        }
        Ok(Sent { raw: raw.to_vec(), wire, index: Some(index) })
    }
}

#[cfg(test)]
mod tests {
    use super::super::protocol::{hex, tests::GOLDEN};
    use super::*;
    use crate::transport::MockTransport;

    const F: Fields = Fields { temp: 45, fan: 1500, load: 20 };

    fn reply(seed_at: usize, seed: u8) -> Vec<u8> {
        let mut r = vec![0u8; READ_LEN];
        r[seed_at - 1] = 0xE3;
        r[seed_at] = seed;
        r
    }

    fn golden(seed: u8, idx: u8) -> &'static str {
        GOLDEN.iter().find(|g| g.0 == seed && g.1 == idx).unwrap().2
    }

    #[test]
    fn handshake_with_seed_at_index_1() {
        let mock = MockTransport::default();
        mock.push_read(&reply(1, 0x2A));
        let d = V1_2Display::open(mock.clone()).unwrap();
        assert_eq!(d.seed(), Some(0x2A));
        assert_eq!(mock.written(), vec![seed_request().to_vec()]);
        assert_eq!(d.firmware(), Firmware::V1_2);
        assert_eq!(d.description(), "mock");
    }

    #[test]
    fn handshake_with_seed_at_index_2() {
        let mock = MockTransport::default();
        mock.push_read(&reply(2, 0xFF)); // 0x01 report ID style: E3 at index 1
        assert_eq!(V1_2Display::open(mock).unwrap().seed(), Some(0xFF));
    }

    #[test]
    fn missing_marker_is_a_handshake_error() {
        let mock = MockTransport::default();
        mock.push_read(&[0x01, 0x81, 0x00, 0x2A]);
        let e = V1_2Display::open(mock.clone()).err().unwrap();
        assert!(e.downcast_ref::<HandshakeError>().is_some(), "{e}");
        assert!(e.to_string().contains("0xE3"));
        // Nothing but the seed request was written.
        assert_eq!(mock.written().len(), 1);
    }

    #[test]
    fn handshake_timeout_and_io_errors_are_handshake_errors() {
        let mock = MockTransport::default(); // nothing scripted: read times out
        let e = V1_2Display::open(mock).err().unwrap();
        assert!(e.downcast_ref::<HandshakeError>().is_some());
        assert!(e.to_string().contains("no reply"));

        let mock = MockTransport::default();
        mock.push_read_error("device gone");
        let e = V1_2Display::open(mock).err().unwrap();
        assert!(e.downcast_ref::<HandshakeError>().is_some());
        assert!(e.to_string().contains("device gone"));

        let mock = MockTransport::default();
        mock.fail_next_write("write refused");
        let e = V1_2Display::open(mock).err().unwrap();
        assert!(e.downcast_ref::<HandshakeError>().is_some());
    }

    #[test]
    fn sends_the_golden_frames_for_each_seed() {
        for seed in [0x00u8, 0xFF, 0x2A] {
            let mock = MockTransport::default();
            mock.push_read(&reply(1, seed));
            let mut d = V1_2Display::open(mock.clone()).unwrap();
            for idx in 0..2u8 {
                let sent = d.send(&F).unwrap();
                assert_eq!(sent.index, Some(idx));
                assert_eq!(hex(&sent.wire), golden(seed, idx), "seed {seed:02x} idx {idx}");
                assert_eq!(sent.raw, super::super::protocol::build_frame(idx, 45, 1500, 20).to_vec());
            }
            let w = mock.written();
            assert_eq!(w.len(), 3); // seed request + 2 frames
            assert_eq!(hex(&w[1]), golden(seed, 0));
            assert_eq!(hex(&w[2]), golden(seed, 1));
        }
    }

    #[test]
    fn index_increments_and_wraps_at_256() {
        let mock = MockTransport::default();
        mock.push_read(&reply(1, 0x2A));
        let mut d = V1_2Display::open(mock).unwrap();
        for i in 0..=255u8 {
            assert_eq!(d.send(&F).unwrap().index, Some(i));
        }
        let s = d.send(&F).unwrap();
        assert_eq!(s.index, Some(0));
        assert_eq!(hex(&s.wire), golden(0x2A, 0));
    }

    #[test]
    fn write_error_rehandshakes_with_a_fresh_seed_and_index_0() {
        let mock = MockTransport::default();
        mock.push_read(&reply(1, 0x00));
        let mut d = V1_2Display::open(mock.clone()).unwrap();
        d.send(&F).unwrap();
        d.send(&F).unwrap();
        mock.fail_next_write("unplugged");
        assert!(d.send(&F).unwrap_err().to_string().contains("unplugged"));
        assert_eq!(d.seed(), None);
        // Next send: handshake again (new seed), then frame 0 with that seed.
        mock.push_read(&reply(1, 0xFF));
        let s = d.send(&F).unwrap();
        assert_eq!(d.seed(), Some(0xFF));
        assert_eq!(s.index, Some(0));
        assert_eq!(hex(&s.wire), golden(0xFF, 0));
        let w = mock.written();
        assert_eq!(w.len(), 5); // req, f0, f1, req, f0
        assert_eq!(w[3], seed_request().to_vec());
    }

    #[test]
    fn failed_rehandshake_keeps_reporting_errors() {
        let mock = MockTransport::default();
        mock.push_read(&reply(1, 0x00));
        let mut d = V1_2Display::open(mock.clone()).unwrap();
        mock.fail_next_write("unplugged");
        assert!(d.send(&F).is_err());
        let e = d.send(&F).unwrap_err(); // no reply scripted
        assert!(e.downcast_ref::<HandshakeError>().is_some());
    }

    #[test]
    fn read_back_is_non_fatal_and_consumes_one_reply() {
        let mock = MockTransport::default();
        mock.push_read(&reply(1, 0x2A));
        let mut d = V1_2Display::open(mock.clone()).unwrap();
        mock.push_read(&[0xAA; 65]); // a reply: read and ignored
        mock.push_read_error("read failed"); // an error: ignored
        assert_eq!(d.send(&F).unwrap().index, Some(0));
        assert_eq!(d.send(&F).unwrap().index, Some(1));
        assert_eq!(d.send(&F).unwrap().index, Some(2)); // nothing scripted: timeout
        assert_eq!(mock.written().len(), 4);
    }

    #[test]
    fn read_back_can_be_disabled() {
        let mock = MockTransport::default();
        mock.push_read(&reply(1, 0x2A));
        let mut d = V1_2Display::open(mock.clone()).unwrap();
        d.set_read_back(false);
        mock.push_read(&reply(1, 0x00)); // stays queued: no read-back
        d.send(&F).unwrap();
        mock.fail_next_write("x");
        assert!(d.send(&F).is_err());
        // The re-handshake gets the queued reply.
        let s = d.send(&F).unwrap();
        assert_eq!((d.seed(), s.index), (Some(0x00), Some(0)));
    }

    #[test]
    fn exchange_reports_raw_bytes() {
        let mock = MockTransport::default();
        mock.push_read(&[0xE3, 0x42, 0x00]);
        let ex = exchange(&mut mock.clone(), HANDSHAKE_TIMEOUT).unwrap();
        assert_eq!(ex.request, seed_request());
        assert_eq!(ex.reply, vec![0xE3, 0x42, 0x00]);
        assert_eq!(ex.seed, Some(0x42));
    }
}
