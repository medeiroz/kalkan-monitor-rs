//! `kalkan` — command-line utility for the cross-platform driver of the Kalkan
//! Alpha 240 / 360 / 360-W and Delta V2 240 / 360 watercoolers (SmartMonitor
//! 1.3, serial) and, experimentally, the older Alpha Rev.1 units (SmartMonitor
//! 1.2, USB HID).
//!
//! Commands:
//!   kalkan list       Lists serial ports and 1.2 HID candidates (marks the cooler by VID/PID)
//!   kalkan sensors    Shows the current sensor reading
//!   kalkan frame      Builds and scrambles a protocol frame and prints the hex
//!   kalkan send [temp] [fan] [seconds] [usage]
//!                     Sends fixed values to the display (hardware test; 1.3 or 1.2)
//!   kalkan handshake  Runs the 1.2 seed handshake and prints the bytes (hardware report)
//!   kalkan hwinfo     Lists temp/fan from the original's HWiNFO engine (admin; Windows)
//!   kalkan hid        Lists all HID devices (diagnostics)
//!
//! Run `kalkan list` on the machine the watercooler is plugged into: the port
//! marked with ★ is the cooler (firmware 1.3: VID/PID 1F3A:0008 or 1A86:484A;
//! firmware 1.2: HID 0416:1100).

use anyhow::Result;
use std::time::Duration;

fn main() -> Result<()> {
    let cmd = std::env::args().nth(1).unwrap_or_else(|| "help".into());
    match cmd.as_str() {
        "list" | "ports" => list(),
        "sensors" => sensors(),
        "frame" => frame(),
        "send" => send(),
        "handshake" => handshake(),
        "hwinfo" => hwinfo(),
        "hid" => hid(),
        _ => {
            eprintln!("usage: kalkan <list|sensors|frame|send|handshake|hwinfo|hid>");
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
    } else {
        list_serial(&ports);
    }
    list_hid();
    Ok(())
}

fn list_serial(ports: &[kalkan_core::SerialDeviceInfo]) {
    println!("{} serial port(s):\n", ports.len());
    for p in ports {
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
}

/// The firmware 1.2 HID candidates (`0416:1100`). Not every device with these
/// IDs is a cooler (0416 is a generic Nuvoton VID): `kalkan handshake` tells.
fn list_hid() {
    use kalkan_core::v1_2::{VID, PID};
    println!();
    let candidates = match kalkan_core::v1_2::find_candidates() {
        Ok(c) => c,
        Err(e) => {
            println!("HID unavailable: {e}");
            return;
        }
    };
    if candidates.is_empty() {
        println!("no firmware 1.2 HID device ({VID:04x}:{PID:04x}) found.");
        return;
    }
    println!("{} HID candidate(s) for firmware 1.2 (experimental):\n", candidates.len());
    for d in &candidates {
        println!(
            "★ {:04x}:{:04x}  {} — {}  (usage page {:#06x}, interface {})  ★ firmware 1.2",
            d.vendor_id,
            d.product_id,
            d.manufacturer.as_deref().unwrap_or("?"),
            d.product.as_deref().unwrap_or("?"),
            d.usage_page,
            d.interface_number,
        );
        println!("    {}", d.path);
    }
    println!(
        "\n★ = possible Kalkan 1.2 display (HID {VID:04x}:{PID:04x}). Close the official \
         software, then run `kalkan handshake` to confirm."
    );
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


/// Hardware test: opens the display and sends a fixed frame every 1 s.
///
/// Fixed values (default 42 °C / 1234 RPM; usage 0 %, only sent by firmware
/// 1.2) are independent of the sensors, so whatever shows up on the display
/// confirms transport + protocol in isolation. The display is picked like the
/// app does (`kalkan_core::find`: firmware 1.3 serial first, then 1.2 HID). It
/// also prints any byte the display sends back (the 1.3 should not reply).
fn send() -> Result<()> {
    let arg = |i: usize, default: u32| -> Result<u32> {
        match std::env::args().nth(i) {
            Some(s) => s.parse().map_err(|_| anyhow::anyhow!("invalid argument: {s}")),
            None => Ok(default),
        }
    };
    let temp = arg(2, 42)?.min(255) as u8;
    let fan = arg(3, 1234)?.min(0xFFFF) as u16;
    let secs = arg(4, 15)?;
    let load = arg(5, 0)?.min(100) as u8;

    let found = kalkan_core::find(
        kalkan_core::available_serial_ports(),
        kalkan_core::v1_2::find_candidates,
        kalkan_core::v1_3::serial::not_found_hint,
    )?;
    match found {
        kalkan_core::Found::Serial(port) => send_1_3(&port, temp, fan, secs),
        kalkan_core::Found::Hid(candidates) => send_1_2(&candidates, temp, fan, load, secs),
    }
}

fn send_1_3(port: &str, temp: u8, fan: u16, secs: u32) -> Result<()> {
    use kalkan_core::{build_frame, scramble, SerialTransport, Transport};

    let mut t = SerialTransport::open_display(port)?;
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

fn send_1_2(
    candidates: &[kalkan_core::v1_2::HidDeviceInfo],
    temp: u8,
    fan: u16,
    load: u8,
    secs: u32,
) -> Result<()> {
    use kalkan_core::{Display, Fields};

    let mut d = kalkan_core::v1_2::open_display(candidates)?;
    println!("opened   : {} (firmware 1.2, experimental)", d.description());
    println!("seed     : {:#04x}", d.seed().unwrap_or(0));
    println!("sending  : temp {temp}°C  fan {fan} RPM  usage {load}%  ({secs}× every 1 s)");
    let f = Fields { temp, fan, load };
    for i in 1..=secs {
        let sent = d.send(&f)?;
        let idx = sent.index.unwrap_or(0);
        println!("  [{i:>3}] frame #{idx:<3} {}", hex(&sent.wire));
        if !d.last_reply().is_empty() {
            println!("        reply: {}", hex(d.last_reply()));
        }
        std::thread::sleep(Duration::from_millis(1000));
    }
    Ok(())
}

/// Firmware 1.2 hardware report: opens each `0416:1100` HID candidate, sends
/// the seed request and prints the request, the raw reply and the parsed seed.
/// Nothing else is sent (no data frames).
fn handshake() -> Result<()> {
    use kalkan_core::v1_2::display::{exchange, HANDSHAKE_TIMEOUT};
    use kalkan_core::v1_2::{open_candidate, PID, VID};

    let candidates = kalkan_core::v1_2::find_candidates()?;
    if candidates.is_empty() {
        anyhow::bail!(
            "no firmware 1.2 HID device ({VID:04x}:{PID:04x}) found. `kalkan list` shows the \
             serial ports (firmware 1.3) and `kalkan hid` every HID device."
        );
    }
    println!("Firmware 1.2 handshake (experimental). Close the official Kalkan software first.");
    for (i, d) in candidates.iter().enumerate() {
        println!(
            "\n[{}] {:04x}:{:04x}  {} — {}  (usage page {:#06x}, interface {})",
            i + 1,
            d.vendor_id,
            d.product_id,
            d.manufacturer.as_deref().unwrap_or("?"),
            d.product.as_deref().unwrap_or("?"),
            d.usage_page,
            d.interface_number,
        );
        println!("    path   : {}", d.path);
        let mut t = match open_candidate(d) {
            Ok(t) => t,
            Err(e) => {
                println!("    open   : FAILED: {e}");
                continue;
            }
        };
        let ex = match exchange(&mut t, HANDSHAKE_TIMEOUT) {
            Ok(ex) => ex,
            Err(e) => {
                println!("    request: {}", hex(&kalkan_core::v1_2::protocol::seed_request()));
                println!("    I/O    : FAILED: {e}");
                continue;
            }
        };
        println!("    request: {}", hex(&ex.request));
        if ex.reply.is_empty() {
            println!("    reply  : (nothing within {} ms)", HANDSHAKE_TIMEOUT.as_millis());
        } else {
            println!("    reply  : {} ({} bytes)", hex(&ex.reply), ex.reply.len());
        }
        match ex.seed {
            Some(seed) => {
                let at = if ex.reply.first() == Some(&0xE3) { 0 } else { 1 };
                println!("    seed   : {seed:#04x} (0xE3 at index {at}) — handshake OK");
            }
            None => println!("    seed   : none (0xE3 not at index 0 or 1) — handshake FAILED"),
        }
    }
    println!(
        "\nPlease paste this output (and `kalkan list`) into an issue, saying what the cooler \
         display showed."
    );
    Ok(())
}

fn hid() -> Result<()> {
    let devices = kalkan_core::enumerate()?;
    if devices.is_empty() {
        println!("no HID devices found.");
        return Ok(());
    }
    println!("{} HID device(s) (diagnostics):\n", devices.len());
    for d in &devices {
        let fw = d.profile().map(|pr| format!("  ★ firmware {}", pr.firmware)).unwrap_or_default();
        println!(
            "{}{:04x}:{:04x}  {} — {}{fw}",
            if d.is_display() { "★ " } else { "  " },
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
