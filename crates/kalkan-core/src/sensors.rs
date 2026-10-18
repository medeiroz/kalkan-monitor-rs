//! Fontes de sensores.
//!
//! O original lê CPU/temperatura/fan do **HWiNFO** (via `HWiNFO64.dll`), que só
//! existe no Windows. Para ser multiplataforma trocamos essa fonte por `sysinfo`
//! (CPU load e, onde disponível, temperaturas/fans) e deixamos o trait aberto
//! para fontes específicas por SO (lm-sensors no Linux, IOKit/SMC no macOS,
//! LibreHardwareMonitor no Windows) adicionadas depois.

use anyhow::Result;

/// Uma leitura instantânea do que o display costuma mostrar.
#[derive(Debug, Clone, Default)]
pub struct Reading {
    /// Uso de CPU em porcentagem (0..=100).
    pub cpu_load: f32,
    /// Uso de memória RAM em porcentagem (0..=100).
    pub ram_load: f32,
    /// Temperatura da CPU em °C, quando a plataforma expõe.
    pub cpu_temp_c: Option<f32>,
    /// Rotação de fan/bomba em RPM, quando disponível.
    pub fan_rpm: Option<u32>,
}

/// Qualquer coisa capaz de produzir uma `Reading`.
pub trait SensorSource: Send {
    fn read(&mut self) -> Result<Reading>;
}

/// Fonte multiplataforma baseada em `sysinfo`.
///
/// Entrega **uso de CPU** em qualquer SO e **temperatura de CPU** onde o
/// `sysinfo` expõe componentes térmicos (no macOS Apple Silicon são os
/// sensores `PMU tdie*`; no Linux, `coretemp`/`k10temp`; etc.). A rotação de
/// fan **não** vem do `sysinfo` — precisa de backend específico (SMC no macOS,
/// lm-sensors no Linux), então fica `None` aqui.
pub struct SysinfoSource {
    sys: sysinfo::System,
    components: sysinfo::Components,
}

impl SysinfoSource {
    pub fn new() -> Self {
        Self {
            sys: sysinfo::System::new(),
            components: sysinfo::Components::new_with_refreshed_list(),
        }
    }

    /// Estima a temperatura de CPU a partir dos componentes térmicos.
    ///
    /// Seleciona sensores cujo rótulo sugere die/CPU e cuja leitura é
    /// plausível (0..150 °C — descarta os sensores desconectados que o macOS
    /// reporta como ~-9199 °C) e tira a média.
    fn cpu_temperature(&self) -> Option<f32> {
        let is_cpu_label = |label: &str| {
            let l = label.to_lowercase();
            ["tdie", "cpu", "core", "package", "tctl", "tccd", "soc"]
                .iter()
                .any(|k| l.contains(k))
        };

        let mut sum = 0.0f32;
        let mut n = 0u32;
        for c in self.components.iter() {
            let Some(t) = c.temperature() else { continue };
            if !t.is_finite() || !(0.0..=150.0).contains(&t) {
                continue;
            }
            if is_cpu_label(c.label()) {
                sum += t;
                n += 1;
            }
        }
        if n > 0 {
            return Some(sum / n as f32);
        }

        // Nenhum rótulo bateu: cai para a média de qualquer sensor plausível.
        let (mut sum, mut n) = (0.0f32, 0u32);
        for c in self.components.iter() {
            if let Some(t) = c.temperature() {
                if t.is_finite() && (0.0..=150.0).contains(&t) {
                    sum += t;
                    n += 1;
                }
            }
        }
        (n > 0).then(|| sum / n as f32)
    }
}

impl Default for SysinfoSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorSource for SysinfoSource {
    fn read(&mut self) -> Result<Reading> {
        self.sys.refresh_cpu_usage();
        let cpu_load = self.sys.global_cpu_usage();

        self.sys.refresh_memory();
        let total = self.sys.total_memory();
        let ram_load = if total > 0 {
            self.sys.used_memory() as f32 / total as f32 * 100.0
        } else {
            0.0
        };

        self.components.refresh(false);
        let cpu_temp_c = self.cpu_temperature();

        Ok(Reading {
            cpu_load,
            ram_load,
            cpu_temp_c,
            // Fan: sem backend de SMC/lm-sensors ainda; ver docs/protocol.md.
            fan_rpm: None,
        })
    }
}
