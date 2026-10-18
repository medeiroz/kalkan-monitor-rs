//! Protocolo de aplicação do display — formato e embaralhamento dos pacotes.
//!
//! **Versão 1.3.** Totalmente reconstruído por engenharia reversa estática
//! (Ghidra via REA) da classe `Sysctl` do binário
//! `Kalkan PC SmartMonitor DZ V1.3.exe`. Ver `docs/protocol.md` para os
//! endereços e o pseudocódigo de origem.
//!
//! O protocolo da 1.3 é **muito mais simples** que o da 1.2 e **incompatível**
//! com ele:
//!   * **Sem handshake / sem seed** — o host apenas abre a serial e envia frames.
//!   * **Frame de 5 bytes** `[0x74, 0x00, temp, fanHi, fanLo]` (sem `cpu_load` e
//!     sem índice de frame, que existiam na 1.2).
//!   * **Embaralhamento trivial**: a partir do byte 2, cada byte é XORado com
//!     `0x02`; os dois primeiros bytes (cabeçalho `0x74 0x00`) vão literais.
//!
//! Origem (`Sysctl::processTrans`, `FUN_1400062b0`):
//! ```text
//! buf.clear();
//! buf.append('t'); buf.append('\0');      // 0x74 0x00
//! buf.append(temp);                        // 1 byte
//! buf.append(fan >> 8); buf.append(fan);   // big-endian
//! for (i = 2; i < buf.size(); i++) buf[i] ^= 0x02;
//! emitSendData(buf);                       // -> Transport escreve na serial
//! ```

use crate::sensors::Reading;

/// Tamanho do frame enviado ao display — 5 bytes.
pub const FRAME_LEN: usize = 5;

/// Cabeçalho literal do frame (`'t'`, `0x00`), não embaralhado.
pub const HEADER: [u8; 2] = [0x74, 0x00];

/// Byte de XOR aplicado do índice 2 em diante (o "scrambler" da 1.3).
pub const SCRAMBLE_XOR: u8 = 0x02;

/// Monta o frame de dados cru (antes do XOR): `Sysctl::processTrans`.
///
/// Layout (5 bytes):
/// | offset | conteúdo |
/// |--------|----------|
/// | 0 | `0x74` (`'t'`) |
/// | 1 | `0x00` |
/// | 2 | `temp` (°C, 1 byte) |
/// | 3 | `fan` byte alto (big-endian) |
/// | 4 | `fan` byte baixo |
pub fn build_frame(temp: u8, fan: u16) -> [u8; FRAME_LEN] {
    [HEADER[0], HEADER[1], temp, (fan >> 8) as u8, fan as u8]
}

/// Embaralha o frame: XOR de `0x02` do byte 2 ao fim; cabeçalho literal.
///
/// Porte fiel do laço observado em `FUN_1400062b0` (o original XORa a partir de
/// `data + 2`). Os dois primeiros bytes (`0x74 0x00`) são preservados.
pub fn scramble(frame: &[u8]) -> Vec<u8> {
    let mut out = frame.to_vec();
    for b in out.iter_mut().skip(HEADER.len()) {
        *b ^= SCRAMBLE_XOR;
    }
    out
}

/// Converte um valor de temperatura em °C para o byte do frame (satura 0..=255).
fn temp_byte(reading: &Reading) -> u8 {
    reading
        .cpu_temp_c
        .map(|t| t.round().clamp(0.0, 255.0) as u8)
        .unwrap_or(0)
}

/// Converte RPM para o campo de 2 bytes do frame (satura em 0xFFFF).
fn fan_word(reading: &Reading) -> u16 {
    reading.fan_rpm.map(|f| f.min(0xFFFF) as u16).unwrap_or(0)
}

/// Monta os pacotes a enviar ao display a partir de uma leitura de sensores.
pub trait PacketBuilder: Send {
    /// Produz zero ou mais pacotes prontos para `Transport::write`.
    fn build(&mut self, reading: &Reading) -> Vec<Vec<u8>>;

    /// Tamanho do frame esperado pelo dispositivo.
    fn frame_len(&self) -> usize;
}

/// Construtor de pacotes do protocolo real do Kalkan SmartMonitor **1.3**.
///
/// Sem estado: a 1.3 não tem seed nem contador de frames. Para cada leitura,
/// monta o frame de 5 bytes e aplica o XOR.
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
        assert_eq!(&s[..2], &[0x74, 0x00]); // cabeçalho literal
        assert_eq!(s.len(), FRAME_LEN);
    }

    #[test]
    fn scramble_regression_vector() {
        // Vetor de regressão da 1.3, conferido contra FUN_1400062b0:
        // temp=40 (0x28), fan=1200 (0x04B0).
        // cru       : 74 00 28 04 B0
        // scrambled : 74 00 2A 06 B2   (bytes 2.. XORados com 0x02)
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
