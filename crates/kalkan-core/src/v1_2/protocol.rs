//! Firmware **1.2** application protocol: handshake packet, data frame and
//! scrambler. Pure functions, no I/O (the transport and the handshake + send
//! sequence live in the sibling `hid` and `display` modules).
//!
//! Reconstructed by static reverse engineering of
//! `Kalkan PC SmartMonitor DZ V1.2.exe` (class `HidDeviceController`); see
//! `docs/v1.2/protocol.md` for the addresses and pseudocode. Ported from the
//! earlier 1.2 Rust repository, whose scrambler was cross-checked against the
//! x86-64 assembly of `FUN_140002a20`. **Untested on real hardware.**
//!
//! Overview:
//!   1. **Handshake**: the host writes the 64-byte seed request
//!      `01 81 E3 00…` ([`seed_request`]) and reads the reply; the byte right
//!      after the magic `0xE3` (at index 0 or 1) is the scrambler seed
//!      ([`parse_seed`]).
//!   2. **Data frame** ([`build_frame`]): 64 bytes
//!      `01 86 E0 idx temp fanHi fanLo cpuLoad 00…`; no checksum. `idx` is a
//!      `u8` incremented per frame sent (wraps at 256).
//!   3. **Scramble** ([`scramble`]): bytes 0–2 go out literal; from byte 3 on
//!      each byte is scrambled bit by bit. The seed is taken **by value**:
//!      every frame restarts from the handshake seed, and it only evolves
//!      across byte positions inside one frame.

/// Size of the HID output report / frame: 64 bytes (byte 0 is `0x01`, which
/// hidapi sends as the report ID).
pub const REPORT_LEN: usize = 64;

/// Size of the vendor's reads (`hid_read_timeout(dev, buf, 65, …)`).
pub const READ_LEN: usize = 65;

/// Magic byte that marks the position of the seed in the handshake reply.
pub const SEED_MARKER: u8 = 0xE3;

/// Literal frame header (`0x01 0x86 0xE0`, "show info"), not scrambled.
pub const FRAME_HEADER: [u8; 3] = [0x01, 0x86, 0xE0];

/// The handshake's seed request: `GenerateScramblerSeed` (`FUN_140002740`).
///
/// 64 zero bytes with `[0]=0x01`, `[1]=0x81`, `[2]=0xE3`.
pub fn seed_request() -> [u8; REPORT_LEN] {
    let mut buf = [0u8; REPORT_LEN];
    buf[0] = 0x01;
    buf[1] = 0x81;
    buf[2] = SEED_MARKER;
    buf
}

/// The seed in a handshake reply: logic of `open` (`FUN_140002cf0`).
///
/// `0xE3` at index 0 → seed at index 1; otherwise `0xE3` at index 1 → seed at
/// index 2. `None` when the marker is absent (or the seed byte is missing).
/// The vendor app then logs `[open] Can't found 0xE3` and silently keeps going
/// with seed 0; this app treats it as a failed handshake instead.
pub fn parse_seed(resp: &[u8]) -> Option<u8> {
    if resp.first() == Some(&SEED_MARKER) {
        return resp.get(1).copied();
    }
    if resp.get(1) == Some(&SEED_MARKER) {
        return resp.get(2).copied();
    }
    None
}

/// The raw data frame (before scrambling): `GenerateShowDevInfo`
/// (`FUN_140002800`).
///
/// | offset | content |
/// |--------|---------|
/// | 0 | `0x01` |
/// | 1 | `0x86` |
/// | 2 | `0xE0` ("show info" command) |
/// | 3 | `frame_index` |
/// | 4 | `temp` (°C) |
/// | 5 | `fan` high byte (big-endian) |
/// | 6 | `fan` low byte |
/// | 7 | `cpu_load` (%) |
/// | 8..63 | `0x00` |
pub fn build_frame(frame_index: u8, temp: u8, fan: u16, cpu_load: u8) -> [u8; REPORT_LEN] {
    let mut buf = [0u8; REPORT_LEN];
    buf[..3].copy_from_slice(&FRAME_HEADER);
    buf[3] = frame_index;
    buf[4] = temp;
    buf[5] = (fan >> 8) as u8;
    buf[6] = fan as u8;
    buf[7] = cpu_load;
    buf
}

/// Scrambles a frame with `seed`: `Scrambler` (`FUN_140002a20`).
///
/// Faithful port (8-bit arithmetic with wraparound): the first 3 bytes are
/// copied unchanged; for each position `pos >= 3`, 8 bit contributions are
/// accumulated and the seed evolves on every bit (`seed = seed*2 + (v>>6)`),
/// carrying over to the next position. `seed` is a local copy: the caller's
/// seed never changes, so every frame starts from the handshake seed.
pub fn scramble(seed: u8, src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    out.extend(src.iter().take(3));
    let mut seed = seed;
    for &s in src.iter().skip(3) {
        let mut acc: u8 = 0;
        for bit in 0..8u32 {
            // v = ((s << bit) & 0x80) + ((seed & 0xf0) << 3) + (seed & 0x80), as u8.
            let v = (s.wrapping_shl(bit) & 0x80)
                .wrapping_add((seed & 0xf0).wrapping_shl(3))
                .wrapping_add(seed & 0x80);
            acc = acc.wrapping_add(v >> bit);
            seed = seed.wrapping_mul(2).wrapping_add(v >> 6);
        }
        out.push(acc);
    }
    out
}

/// Frame builder for one 1.2 session: the handshake seed plus the frame index.
///
/// Decision: the index starts at 0 for every new builder, i.e. on every new
/// handshake. The vendor app keeps the counter in the controller object and
/// does not reset it on reopen; the device is not known to care (the initial
/// value is unconfirmed, most likely 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Builder {
    seed: u8,
    frame_index: u8,
}

impl Builder {
    pub fn new(seed: u8) -> Self {
        Self { seed, frame_index: 0 }
    }

    /// The scrambler seed from the handshake.
    pub fn seed(&self) -> u8 {
        self.seed
    }

    /// Index the next frame will carry.
    pub fn next_index(&self) -> u8 {
        self.frame_index
    }

    /// Builds the next frame: returns `(index, raw, scrambled)` and advances
    /// the index (wrapping at 256).
    pub fn next(&mut self, temp: u8, fan: u16, cpu_load: u8) -> (u8, [u8; REPORT_LEN], Vec<u8>) {
        let idx = self.frame_index;
        let raw = build_frame(idx, temp, fan, cpu_load);
        let wire = scramble(self.seed, &raw);
        self.frame_index = idx.wrapping_add(1);
        (idx, raw, wire)
    }
}

/// Lowercase hex, no separators (also used by tests and the CLI).
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Golden vectors from `docs/v1.2/protocol.md`: temp=45, fan=1500,
    /// cpu_load=20, seed restarted for each frame. (seed, idx, full frame).
    pub(crate) const GOLDEN: [(u8, u8, &str); 6] = [
        (0x00, 0, "0186e0002fa0947b42afa51b8fe1de592044c5d6c19a9cf6855f4a371fc3bcb240898bad833539ed0abe946e3f8779648113175b066a73da157d28dc7f0ef2c9"),
        (0x00, 1, "0186e0013cb7cf7d28dc7f0ef2c902262eb60cd4e7b42afa51b8fe1de592044c5d6c19a9cf6855f4a371fc3bcb240898bad833539ed0abe946e3f87796481131"),
        (0xFF, 0, "0186e00756c4156855f4a371fc3bcb240898bad833539ed0abe946e3f8779648113175b066a73da157d28dc7f0ef2c902262eb60cd4e7b42afa51b8fe1de5920"),
        (0xFF, 1, "0186e00645d34e6e3f8779648113175b066a73da157d28dc7f0ef2c902262eb60cd4e7b42afa51b8fe1de592044c5d6c19a9cf6855f4a371fc3bcb240898bad8"),
        (0x2A, 0, "0186e07d077ceb75b066a73da157d28dc7f0ef2c902262eb60cd4e7b42afa51b8fe1de592044c5d6c19a9cf6855f4a371fc3bcb240898bad833539ed0abe946e"),
        (0x2A, 1, "0186e07c146bb073da157d28dc7f0ef2c902262eb60cd4e7b42afa51b8fe1de592044c5d6c19a9cf6855f4a371fc3bcb240898bad833539ed0abe946e3f87796"),
    ];

    #[test]
    fn golden_vectors_are_full_frames() {
        for (_, _, h) in GOLDEN {
            assert_eq!(h.len(), REPORT_LEN * 2, "{h}");
        }
    }

    #[test]
    fn scramble_matches_the_golden_vectors() {
        for (seed, idx, expected) in GOLDEN {
            let out = scramble(seed, &build_frame(idx, 45, 1500, 20));
            assert_eq!(out.len(), REPORT_LEN);
            assert_eq!(hex(&out), expected, "seed {seed:02x} idx {idx}");
        }
    }

    #[test]
    fn builder_restarts_from_the_seed_every_frame() {
        for seed in [0x00, 0xFF, 0x2A] {
            let mut b = Builder::new(seed);
            for (s, idx, expected) in GOLDEN.iter().filter(|g| g.0 == seed) {
                let (i, raw, wire) = b.next(45, 1500, 20);
                assert_eq!(i, *idx);
                assert_eq!(raw, build_frame(*idx, 45, 1500, 20));
                assert_eq!(hex(&wire), *expected, "seed {s:02x} idx {idx}");
            }
            assert_eq!(b.seed(), seed);
        }
    }

    #[test]
    fn builder_index_wraps_at_256() {
        let mut b = Builder::new(0x2A);
        for i in 0..=255u8 {
            assert_eq!(b.next(0, 0, 0).0, i);
        }
        assert_eq!(b.next_index(), 0);
        assert_eq!(b.next(0, 0, 0).0, 0);
    }

    #[test]
    fn scramble_regression_vector() {
        // From the 1.2 repository: seed 0x2A, idx 1, temp 40, fan 1200, load 55.
        let out = scramble(0x2A, &build_frame(1, 40, 1200, 55));
        let expected: [u8; 16] = [
            0x01, 0x86, 0xe0, 0x7c, 0x11, 0x35, 0x83, 0x01, 0x13, 0x17, 0x5b, 0x06, 0x6a, 0x73,
            0xda, 0x15,
        ];
        assert_eq!(&out[..16], &expected);
    }

    #[test]
    fn scramble_keeps_header_and_length() {
        let frame = build_frame(1, 40, 1200, 55);
        let out = scramble(0x2A, &frame);
        assert_eq!(out.len(), frame.len());
        assert_eq!(&out[..3], &FRAME_HEADER);
        assert!(scramble(0x2A, &[]).is_empty());
        assert_eq!(scramble(0x2A, &[1, 2]), vec![1, 2]);
    }

    #[test]
    fn seed_request_layout() {
        let r = seed_request();
        assert_eq!(r.len(), 64);
        assert_eq!(&r[..3], &[0x01, 0x81, 0xE3]);
        assert!(r[3..].iter().all(|&b| b == 0));
    }

    #[test]
    fn frame_layout() {
        let f = build_frame(0x05, 42, 0x0ABC, 73);
        assert_eq!(&f[..8], &[0x01, 0x86, 0xE0, 0x05, 42, 0x0A, 0xBC, 73]);
        assert!(f[8..].iter().all(|&b| b == 0));
    }

    #[test]
    fn parse_seed_at_index_0_or_1() {
        assert_eq!(parse_seed(&[0xE3, 0x2A, 0x00]), Some(0x2A));
        assert_eq!(parse_seed(&[0x00, 0xE3, 0x2A]), Some(0x2A));
        // Index 0 wins when both positions hold the marker.
        assert_eq!(parse_seed(&[0xE3, 0xE3, 0x11]), Some(0xE3));
        // Report-ID style reply (65 bytes, byte 0 = 0x01).
        let mut r = [0u8; READ_LEN];
        r[0] = 0x01;
        r[1] = 0xE3;
        r[2] = 0xFF;
        assert_eq!(parse_seed(&r), Some(0xFF));
    }

    #[test]
    fn parse_seed_without_marker_or_seed_byte() {
        assert_eq!(parse_seed(&[0x00, 0x00, 0xE3]), None);
        assert_eq!(parse_seed(&[]), None);
        assert_eq!(parse_seed(&[0xE3]), None);
        assert_eq!(parse_seed(&[0x00, 0xE3]), None);
        assert_eq!(parse_seed(&[0u8; READ_LEN]), None);
    }
}
