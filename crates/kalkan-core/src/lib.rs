//! Núcleo multiplataforma do driver do watercooler Kalkan SmartMonitor **1.3**.
//!
//! Reimplementação em Rust do comportamento observado no app Windows original
//! (ver `docs/protocol.md`). Compila para Windows, macOS e Linux.
//!
//! A 1.3 fala com o cooler **só por porta serial** (USB-serial), diferente da
//! 1.2 que também tinha um caminho HID. O dispositivo é achado por VID/PID e
//! aberto a 1 Mbps, 8N1, sem controle de fluxo. Não há handshake.

pub mod device;
pub mod protocol;
pub mod sensors;
pub mod transport;

pub use device::{
    available_serial_ports, enumerate, find_display_port, is_display_port, looks_like_display,
    HidDeviceInfo, SerialDeviceInfo, DISPLAY_IDS,
};
pub use protocol::{build_frame, scramble, KalkanBuilder, PacketBuilder, FRAME_LEN};
pub use sensors::{Reading, SensorSource, SysinfoSource};
pub use transport::{HidTransport, SerialTransport, Transport};

use anyhow::{anyhow, Result};
use std::time::Duration;

/// Baud rate do display 1.3 (`QSerialPort::setBaudRate(1000000, ...)`).
pub const BAUD: u32 = 1_000_000;

/// Acha e abre o display 1.3 na primeira porta serial cujo VID/PID casa.
///
/// Reproduz `Sysctl::connect`/`Transport::open` (`FUN_140008300` →
/// `FUN_1400083d0`): enumera as portas, casa por (VID, PID) contra
/// [`DISPLAY_IDS`], e abre a 1 Mbps / 8N1 / sem controle de fluxo.
pub fn open_display() -> Result<SerialTransport> {
    let port = find_display_port()?.ok_or_else(|| {
        anyhow!(
            "nenhum display Kalkan 1.3 encontrado (procurando VID:PID {})",
            DISPLAY_IDS
                .iter()
                .map(|(v, p)| format!("{v:04x}:{p:04x}"))
                .collect::<Vec<_>>()
                .join(" ou ")
        )
    })?;
    SerialTransport::open_display(&port)
}

/// Laço do driver: lê sensores, monta pacotes e envia ao display, num intervalo.
///
/// Junta as três peças (`SensorSource` + `PacketBuilder` + `Transport`) sem
/// acoplar a UI. Roda até `should_stop()` retornar true.
pub fn run_loop(
    sensors: &mut dyn SensorSource,
    builder: &mut dyn PacketBuilder,
    transport: &mut dyn Transport,
    interval: Duration,
    mut should_stop: impl FnMut() -> bool,
) -> Result<()> {
    while !should_stop() {
        let reading = sensors.read()?;
        for pkt in builder.build(&reading) {
            transport.write(&pkt)?;
        }
        std::thread::sleep(interval);
    }
    Ok(())
}
