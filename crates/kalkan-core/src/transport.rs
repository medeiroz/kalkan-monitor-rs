//! Camadas de transporte para o display do watercooler.
//!
//! A **1.3** fala com o dispositivo só por **porta serial** (`QSerialPort`,
//! classe `Transport` no binário; ver `docs/protocol.md`). Use
//! [`SerialTransport::open_display`] para os parâmetros exatos do cooler.
//!
//! [`HidTransport`] é mantido como legado da 1.2 / diagnóstico; a 1.3 não o
//! usa. As duas ficam atrás de um único trait `Transport`.

use anyhow::{anyhow, Result};
use std::time::Duration;

/// Um canal já aberto capaz de enviar pacotes ao display e ler respostas.
pub trait Transport: Send {
    /// Envia um pacote (um "report" HID ou um frame serial) ao dispositivo.
    fn write(&mut self, data: &[u8]) -> Result<usize>;

    /// Lê até `buf.len()` bytes, respeitando `timeout`. Retorna quantos leu.
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize>;

    /// Nome legível do canal, para logs/UI.
    fn description(&self) -> String;
}

/// Transporte USB HID.
///
/// Observação da reversa: o original chama `hid_set_nonblocking(dev, 0)`
/// imediatamente antes de cada `hid_write`, ou seja, escrita **bloqueante**.
/// Replicamos esse comportamento para igualar a temporização do protocolo.
pub struct HidTransport {
    device: hidapi::HidDevice,
    label: String,
}

impl HidTransport {
    /// Abre o primeiro dispositivo cujo (vid, pid) casa com o pedido —
    /// mesma estratégia de `HidDeviceController::isOpen` (enumera e compara
    /// vid em `dev_info+8` e pid em `dev_info+10`).
    pub fn open(api: &hidapi::HidApi, vid: u16, pid: u16) -> Result<Self> {
        let device = api
            .open(vid, pid)
            .map_err(|e| anyhow!("falha ao abrir HID {vid:04x}:{pid:04x}: {e}"))?;
        device.set_blocking_mode(true)?; // equivale a hid_set_nonblocking(dev, 0)
        Ok(Self {
            device,
            label: format!("HID {vid:04x}:{pid:04x}"),
        })
    }
}

impl Transport for HidTransport {
    fn write(&mut self, data: &[u8]) -> Result<usize> {
        // hidapi exige que o byte 0 seja o Report ID. O chamador deve montar o
        // buffer já com o report id na frente (ver `protocol`).
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

/// Transporte serial / COM, para os modelos que usam `QSerialPort`.
pub struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
    label: String,
}

impl SerialTransport {
    pub fn open(path: &str, baud: u32) -> Result<Self> {
        let port = serialport::new(path, baud)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|e| anyhow!("falha ao abrir serial {path} @ {baud}: {e}"))?;
        Ok(Self {
            port,
            label: format!("serial {path} @ {baud}"),
        })
    }

    /// Abre a porta com os parâmetros exatos do display 1.3.
    ///
    /// De `FUN_1400083d0`: 1 Mbps, 8 data bits, sem paridade, 1 stop bit, sem
    /// controle de fluxo (os padrões do `serialport` já são 8N1/sem-fluxo, mas
    /// deixamos explícito para casar com o original byte a byte).
    pub fn open_display(path: &str) -> Result<Self> {
        let port = serialport::new(path, crate::BAUD)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|e| anyhow!("falha ao abrir display 1.3 em {path}: {e}"))?;
        Ok(Self {
            port,
            label: format!("serial {path} @ {} (Kalkan 1.3)", crate::BAUD),
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
