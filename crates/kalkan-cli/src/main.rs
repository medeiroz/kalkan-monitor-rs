//! `kalkan` — command-line utility for the cross-platform driver (SmartMonitor
//! 1.3) of the Kalkan Alpha 240 / 360 / 360-W and Delta V2 240 / 360 watercoolers.
//!
//! Commands:
//!   kalkan list       Lists serial ports (marks the cooler by VID/PID)
//!   kalkan sensors    Shows the current sensor reading
//!   kalkan frame      Builds and scrambles a protocol frame and prints the hex
//!   kalkan send [temp] [fan] [seconds]
//!                     Sends fixed values to the display (hardware test)
//!   kalkan hwinfo     Lists temp/fan from the original's HWiNFO engine (admin; Windows)
//!   kalkan hid        Lists HID devices (diagnostics; the 1.3 does not use HID)
//!
//! Run `kalkan list` on the machine the watercooler is plugged into: the port
//! marked with ★ is the cooler (VID/PID 1F3A:0008 or 1A86:484A).

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
            eprintln!("usage: kalkan <list|sensors|frame|send|hwinfo|hid>");
            Ok(())
        }
    }
}

fn list() -> Result<()> {
    let mut ports = kalkan_core::available_serial_ports()?;
    // cooler candidates first
    ports.sort_by_key(|p| !p.is_display());
    if ports.is_empty() {
        println!("no serial ports found.");
        return Ok(());
    }
    println!("{} serial port(s):\n", ports.len());
    for p in &ports {
        let star = if p.is_display() { "★ " } else { "  " };
        let fw = p.profile().map(|pr| format!("  ★ firmware {}", pr.firmware)).unwrap_or_default();
        let ids = match (p.vendor_id, p.product_id) {
            (Some(v), Some(pid)) => format!("{v:04x}:{pid:04x}"),
            _ => "----:----".into(),
        };
        println!(
            "{star}{:<12} {ids}  {} — {}{fw}",
            p.port_name,
            p.manufacturer.as_deref().unwrap_or("?"),
            p.product.as_deref().unwrap_or("?"),
        );
    }
    println!("\n★ = Kalkan 1.3 display (VID/PID {}).", ids_help());
    Ok(())
}

fn ids_help() -> String {
    kalkan_core::DISPLAY_IDS
        .iter()
        .map(|(v, p)| format!("{v:04x}:{p:04x}"))
        .collect::<Vec<_>>()
        .join(" or ")
}

fn sensors() -> Result<()> {
    let mut src = kalkan_core::default_source();
    // sysinfo needs two samples for the CPU %
    src.read()?;
    std::thread::sleep(Duration::from_millis(300));
    let r = src.read()?;
    println!("CPU load : {:.1}%", r.cpu_load);
    println!(
        "CPU temp : {}",
        r.cpu_temp_c.map(|t| format!("{t:.1} °C")).unwrap_or_else(|| "n/a".into())
    );
    println!(
        "Fan      : {}",
        r.fan_rpm.map(|f| format!("{f} RPM")).unwrap_or_else(|| "n/a".into())
    );
    // Every sensor the backend enumerates (ids are what settings.json stores).
    if !r.sensors.is_empty() {
        println!("\n{} sensor(s):", r.sensors.len());
        for s in &r.sensors {
            let (value, unit) = match s.kind {
                kalkan_core::SensorKind::Temp => (format!("{:.1}", s.value), "°C"),
                kalkan_core::SensorKind::Fan => (format!("{:.0}", s.value), "RPM"),
            };
            println!("  {:<42} {:>8} {unit:<3}  {}", s.id, value, s.label());
        }
    }
    Ok(())
}

/// Demonstrates the 1.3 protocol end to end without hardware: reads the sensors,
/// builds the 5-byte frame, applies the XOR and prints both buffers in hex.
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

    println!("reading  : temp {temp}°C  fan {fan} RPM  (the 1.3 does not send cpu_load)");
    println!("raw      : {}", hex(&raw));
    println!("scrambled: {}", hex(&scr));
    Ok(())
}

/// Hardware test: opens the display and sends a fixed frame every 1 s.
///
/// Fixed values (default 42 °C / 1234 RPM) are independent of the sensors, so
/// whatever shows up on the display confirms transport + protocol in isolation.
/// It also prints any byte the display sends back (the 1.3 should not reply).
fn send() -> Result<()> {
    use kalkan_core::{build_frame, scramble, Transport};

    let arg = |i: usize, default: u32| -> Result<u32> {
        match std::env::args().nth(i) {
            Some(s) => s.parse().map_err(|_| anyhow::anyhow!("invalid argument: {s}")),
            None => Ok(default),
        }
    };
    let temp = arg(2, 42)?.min(255) as u8;
    let fan = arg(3, 1234)?.min(0xFFFF) as u16;
    let secs = arg(4, 15)?;

    let mut t = kalkan_core::open_display()?;
    let pkt = scramble(&build_frame(temp, fan));
    println!("opened   : {}", t.description());
    println!("sending  : temp {temp}°C  fan {fan} RPM  → {}  ({secs}× every 1 s)", hex(&pkt));

    let mut buf = [0u8; 64];
    for i in 1..=secs {
        let n = t.write(&pkt)?;
        let got = t.read(&mut buf, Duration::from_millis(1000))?;
        if got > 0 {
            println!("  [{i:>3}] {n} bytes sent, reply: {}", hex(&buf[..got]));
        } else {
            println!("  [{i:>3}] {n} bytes sent");
        }
    }
    Ok(())
}

/// Diagnostics for the embedded HWiNFO engine (`config.dat`): lists every
/// temperature/fan reading and shows which one the original would pick. Needs admin.
#[cfg(windows)]
fn hwinfo() -> Result<()> {
    use kalkan_core::sensors::hwinfo_dll::Hwinfo;

    let mut hw = Hwinfo::load()?;
    println!("DLL      : {}", hw.path.display());
    let readings = hw.readings();
    println!("{} temp/fan reading(s):\n", readings.len());
    for r in &readings {
        let kind = if r.kind == 1 { "temp" } else { "fan " };
        println!(
            "  [{:>2}:{:>3}] {kind} {:>9.1}  {} — {}",
            r.sensor, r.index, r.value, r.sensor_name, r.label
        );
    }
    let s = hw.snapshot();
    println!("\npicked   : temp {:?} °C  fan {:?} RPM", s.temp_c, s.fan_rpm);
    Ok(())
}

#[cfg(not(windows))]
fn hwinfo() -> Result<()> {
    anyhow::bail!("the HWiNFO engine only exists on Windows")
}

fn hid() -> Result<()> {
    let devices = kalkan_core::enumerate()?;
    if devices.is_empty() {
        println!("no HID devices found.");
        return Ok(());
    }
    println!("{} HID device(s) (diagnostics — the 1.3 uses serial):\n", devices.len());
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
