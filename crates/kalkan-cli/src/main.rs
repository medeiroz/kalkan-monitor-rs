//! `kalkan` — utilitário de linha de comando do driver multiplataforma (1.3).
//!
//! Comandos:
//!   kalkan list       Lista portas seriais (marca o cooler por VID/PID)
//!   kalkan sensors    Mostra a leitura de sensores atual
//!   kalkan frame      Monta e embaralha um frame do protocolo e imprime o hex
//!   kalkan send [temp] [fan] [segundos]
//!                     Envia valores fixos ao display (teste de hardware)
//!   kalkan hwinfo     Lista temp/fan do motor HWiNFO do original (admin; Windows)
//!   kalkan hid       Lista dispositivos HID (diagnóstico; a 1.3 não usa HID)
//!
//! Rode `kalkan list` na máquina em que o watercooler está plugado: a porta
//! marcada com ★ é o cooler (VID/PID 1F3A:0008 ou 1A86:484A).

use anyhow::Result;
use std::time::Duration;

fn main() -> Result<()> {
    let cmd = std::env::args().nth(1).unwrap_or_else(|| "help".into());
    match cmd.as_str() {
        "list" | "ports" => list(),
        "sensors" => sensors(),
        "frame" => frame(),
        "send" => send(),
        "hwinfo" => hwinfo(),
        "hid" => hid(),
        _ => {
            eprintln!("uso: kalkan <list|sensors|frame|send|hwinfo|hid>");
            Ok(())
        }
    }
}

fn list() -> Result<()> {
    let mut ports = kalkan_core::available_serial_ports()?;
    // candidatos ao cooler primeiro
    ports.sort_by_key(|p| !p.is_display());
    if ports.is_empty() {
        println!("nenhuma porta serial encontrada.");
        return Ok(());
    }
    println!("{} porta(s) serial:\n", ports.len());
    for p in &ports {
        let star = if p.is_display() { "★ " } else { "  " };
        let ids = match (p.vendor_id, p.product_id) {
            (Some(v), Some(pid)) => format!("{v:04x}:{pid:04x}"),
            _ => "----:----".into(),
        };
        println!(
            "{star}{:<12} {ids}  {} — {}",
            p.port_name,
            p.manufacturer.as_deref().unwrap_or("?"),
            p.product.as_deref().unwrap_or("?"),
        );
    }
    println!("\n★ = display Kalkan 1.3 (VID/PID {}).", ids_help());
    Ok(())
}

fn ids_help() -> String {
    kalkan_core::DISPLAY_IDS
        .iter()
        .map(|(v, p)| format!("{v:04x}:{p:04x}"))
        .collect::<Vec<_>>()
        .join(" ou ")
}

fn sensors() -> Result<()> {
    let mut src = kalkan_core::default_source();
    // sysinfo precisa de duas amostras para o %CPU
    src.read()?;
    std::thread::sleep(Duration::from_millis(300));
    let r = src.read()?;
    println!("CPU load : {:.1}%", r.cpu_load);
    println!(
        "CPU temp : {}",
        r.cpu_temp_c.map(|t| format!("{t:.1} °C")).unwrap_or_else(|| "n/d".into())
    );
    println!(
        "Fan      : {}",
        r.fan_rpm.map(|f| format!("{f} RPM")).unwrap_or_else(|| "n/d".into())
    );
    Ok(())
}

/// Demonstra o protocolo 1.3 ponta a ponta sem hardware: lê os sensores, monta
/// o frame de 5 bytes, aplica o XOR e imprime os dois buffers em hex.
fn frame() -> Result<()> {
    use kalkan_core::{build_frame, scramble};

    let mut src = kalkan_core::default_source();
    src.read()?;
    std::thread::sleep(Duration::from_millis(300));
    let r = src.read()?;

    let temp = r.cpu_temp_c.map(|t| t.round().clamp(0.0, 255.0) as u8).unwrap_or(0);
    let fan = r.fan_rpm.map(|f| f.min(0xFFFF) as u16).unwrap_or(0);

    let raw = build_frame(temp, fan);
    let scr = scramble(&raw);

    println!("leitura  : temp {temp}°C  fan {fan} RPM  (a 1.3 não envia cpu_load)");
    println!("source   : {}", hex(&raw));
    println!("scramb   : {}", hex(&scr));
    Ok(())
}

/// Teste de hardware: abre o display e envia um frame fixo a cada 1 s.
///
/// Valores fixos (padrão 42 °C / 1234 RPM) independem dos sensores, então o que
/// aparecer no display confirma transporte + protocolo isoladamente. Também
/// imprime qualquer byte que o display devolver (a 1.3 não deveria responder).
fn send() -> Result<()> {
    use kalkan_core::{build_frame, scramble, Transport};

    let arg = |i: usize, default: u32| -> Result<u32> {
        match std::env::args().nth(i) {
            Some(s) => s.parse().map_err(|_| anyhow::anyhow!("argumento inválido: {s}")),
            None => Ok(default),
        }
    };
    let temp = arg(2, 42)?.min(255) as u8;
    let fan = arg(3, 1234)?.min(0xFFFF) as u16;
    let secs = arg(4, 15)?;

    let mut t = kalkan_core::open_display()?;
    let pkt = scramble(&build_frame(temp, fan));
    println!("aberto   : {}", t.description());
    println!("enviando : temp {temp}°C  fan {fan} RPM  → {}  ({secs}× a cada 1 s)", hex(&pkt));

    let mut buf = [0u8; 64];
    for i in 1..=secs {
        let n = t.write(&pkt)?;
        let got = t.read(&mut buf, Duration::from_millis(1000))?;
        if got > 0 {
            println!("  [{i:>3}] {n} bytes enviados, resposta: {}", hex(&buf[..got]));
        } else {
            println!("  [{i:>3}] {n} bytes enviados");
        }
    }
    Ok(())
}

/// Diagnóstico do motor HWiNFO embutido (`config.dat`): lista toda leitura de
/// temperatura/fan e mostra qual o original escolheria. Precisa de admin.
#[cfg(windows)]
fn hwinfo() -> Result<()> {
    use kalkan_core::sensors::hwinfo_dll::Hwinfo;

    let mut hw = Hwinfo::load()?;
    println!("DLL      : {}", hw.path.display());
    let readings = hw.readings();
    println!("{} leitura(s) de temp/fan:\n", readings.len());
    for r in &readings {
        let kind = if r.kind == 1 { "temp" } else { "fan " };
        println!(
            "  [{:>2}:{:>3}] {kind} {:>9.1}  {} — {}",
            r.sensor, r.index, r.value, r.sensor_name, r.label
        );
    }
    let s = hw.snapshot();
    println!("\nescolha  : temp {:?} °C  fan {:?} RPM", s.temp_c, s.fan_rpm);
    Ok(())
}

#[cfg(not(windows))]
fn hwinfo() -> Result<()> {
    anyhow::bail!("o motor HWiNFO só existe no Windows")
}

fn hid() -> Result<()> {
    let devices = kalkan_core::enumerate()?;
    if devices.is_empty() {
        println!("nenhum dispositivo HID encontrado.");
        return Ok(());
    }
    println!("{} dispositivo(s) HID (diagnóstico — a 1.3 usa serial):\n", devices.len());
    for d in &devices {
        println!(
            "  {:04x}:{:04x}  {} — {}",
            d.vendor_id,
            d.product_id,
            d.manufacturer.as_deref().unwrap_or("?"),
            d.product.as_deref().unwrap_or("?"),
        );
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join("")
}
