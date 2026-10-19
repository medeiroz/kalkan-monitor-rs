//! Firmware 1.3 [`Display`]: builds the 5-byte frame and writes it to the
//! transport, exactly as the UI did on every tick before the abstraction.

use super::protocol::{build_frame, scramble};
use crate::firmware::{Display, Fields, Firmware, Sent};
use crate::transport::Transport;
use anyhow::Result;

/// A 1.3 display over any [`Transport`] (the serial port in production).
pub struct V1_3Display<T: Transport> {
    transport: T,
}

impl<T: Transport> V1_3Display<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }
}

impl<T: Transport> Display for V1_3Display<T> {
    fn firmware(&self) -> Firmware {
        Firmware::V1_3
    }

    fn description(&self) -> String {
        self.transport.description()
    }

    /// The 1.3 frame has no usage field nor counter: `load` is ignored.
    fn send(&mut self, f: &Fields) -> Result<Sent> {
        let raw = build_frame(f.temp, f.fan);
        let wire = scramble(&raw);
        self.transport.write(&wire)?;
        Ok(Sent { raw: raw.to_vec(), wire, index: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MockTransport;

    #[test]
    fn send_matches_the_build_frame_scramble_path() {
        let cases = [
            (0u8, 0u16),
            (40, 1200),
            (255, 0xFFFF),
            (1, 0x0100),
            (0x7F, 0x00FF),
            (254, 65534),
        ];
        for (temp, fan) in cases {
            let mock = MockTransport::default();
            let mut d = V1_3Display::new(mock.clone());
            let sent = d.send(&Fields { temp, fan, load: 77 }).unwrap();
            let expected = scramble(&build_frame(temp, fan));
            assert_eq!(sent.wire, expected);
            assert_eq!(sent.raw, build_frame(temp, fan).to_vec());
            assert_eq!(sent.index, None);
            assert_eq!(mock.written(), vec![expected]);
        }
    }

    #[test]
    fn saturated_readings_match_the_builder() {
        use super::super::protocol::{KalkanBuilder, PacketBuilder};
        use crate::sensors::Reading;
        // What the UI does to a reading: round + clamp the temperature, cap the fan.
        let r = Reading { cpu_temp_c: Some(300.4), fan_rpm: Some(70_000), ..Default::default() };
        let pkt = KalkanBuilder::new().build(&r).remove(0);
        let mock = MockTransport::default();
        let mut d = V1_3Display::new(mock.clone());
        let f = Fields {
            temp: 300.4f32.round().clamp(0.0, 255.0) as u8,
            fan: 70_000u32.min(0xFFFF) as u16,
            load: 0,
        };
        d.send(&f).unwrap();
        assert_eq!(mock.written(), vec![pkt]);
    }

    #[test]
    fn write_errors_propagate() {
        let mock = MockTransport::default();
        mock.fail_next_write("unplugged");
        let mut d = V1_3Display::new(mock.clone());
        let e = d.send(&Fields::default()).unwrap_err();
        assert!(e.to_string().contains("unplugged"));
        assert!(mock.written().is_empty());
        // The next write goes through.
        assert!(d.send(&Fields::default()).is_ok());
        assert_eq!(d.firmware(), Firmware::V1_3);
        assert_eq!(d.description(), "mock");
    }
}
