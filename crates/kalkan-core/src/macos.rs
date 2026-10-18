//! macOS sensors: IOKit HID (Apple Silicon) and the SMC (Intel Macs, fans).
//!
//! Neither source needs root.
//!
//! - **Apple Silicon**: the temperature sensors are HID services
//!   (`IOHIDEventSystemClient`, usage page `0xff00`, usage 5) whose `Product`
//!   property names them (`PMU tdie1`, `PMU tdev1`, `NAND CH0 temp`,
//!   `gas gauge battery`, ...). Each one is mapped to a friendly name and a
//!   hardware type that `naming.rs` turns into a category. The Auto CPU
//!   temperature is the average of the CPU die sensors.
//! - **Intel**: temperatures come from the SMC (`AppleSMC` IOKit service) by
//!   four-character key (`TC0P` CPU proximity, `TG0D` GPU diode, ...). The
//!   keys are probed once at start-up; only those that exist are read.
//! - **Fans**: SMC `FNum` (count) and `F<n>Ac` (actual RPM), on Intel and on
//!   Apple Silicon Macs that have fans. A fanless Mac reports no fans.
//!
//! Everything platform independent lives here and is tested on every OS: the
//! SMC wire format and value decoding, the sensor table, the HID name mapping,
//! sensor building and the Auto picks. The IOKit/CoreFoundation FFI is in the
//! `iokit` submodule, compiled on macOS only (see `docs/sensors-macos.md`).

use anyhow::Result;

use crate::sensors::{Reading, SensorKind, SensorSource, SensorValue, SysinfoSource};

#[cfg(target_os = "macos")]
mod iokit;

// -- SMC wire format ------------------------------------------------------------

/// `IOConnectCallStructMethod` selector of the SMC user client
/// (`kSMCHandleYPCEvent`).
pub const SMC_SELECTOR: u32 = 2;
/// `SmcKeyData::data8` command: read the bytes of a key.
pub const SMC_CMD_READ_BYTES: u8 = 5;
/// `SmcKeyData::data8` command: read size and type of a key.
pub const SMC_CMD_READ_KEYINFO: u8 = 9;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SmcVersion {
    pub major: u8,
    pub minor: u8,
    pub build: u8,
    pub reserved: u8,
    pub release: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SmcPLimit {
    pub version: u16,
    pub length: u16,
    pub cpu: u32,
    pub gpu: u32,
    pub mem: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SmcKeyInfo {
    pub data_size: u32,
    /// Four-character type (`sp78`, `flt `, ...) packed big-endian.
    pub data_type: u32,
    pub data_attributes: u8,
}

/// The struct exchanged with the SMC user client (80 bytes, C layout).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SmcKeyData {
    pub key: u32,
    pub vers: SmcVersion,
    pub p_limit: SmcPLimit,
    pub key_info: SmcKeyInfo,
    pub result: u8,
    pub status: u8,
    pub data8: u8,
    pub data32: u32,
    pub bytes: [u8; 32],
}

/// A four-character code (an SMC key or data type) as the big-endian `u32`
/// the SMC uses. Anything but exactly 4 bytes gives 0.
pub fn fourcc(s: &str) -> u32 {
    match <[u8; 4]>::try_from(s.as_bytes()) {
        Ok(b) => u32::from_be_bytes(b),
        Err(_) => 0,
    }
}

/// Inverse of [`fourcc`], for messages.
pub fn fourcc_to_string(v: u32) -> String {
    v.to_be_bytes()
        .iter()
        .map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' })
        .collect()
}

fn hex_digit(b: u8) -> Option<u32> {
    (b as char).to_digit(16)
}

/// Decodes the bytes of an SMC value of the given type.
///
/// Supported: `ui8 `/`ui16`/`ui32`, `si8 `/`si16` (integers, big-endian),
/// `flt ` (little-endian IEEE float) and the fixed-point family `spXY`
/// (signed, X integer bits + Y fraction bits = 15, e.g. `sp78`) and `fpXY`
/// (unsigned, X + Y = 16, e.g. `fpe2`). Anything else, or too few bytes,
/// gives `None`.
pub fn decode_smc(data_type: u32, data: &[u8]) -> Option<f32> {
    let t = data_type.to_be_bytes();
    let be16 = || Some(u16::from_be_bytes(data.get(..2)?.try_into().ok()?));
    let v = match &t {
        b"ui8 " => *data.first()? as f32,
        b"si8 " => *data.first()? as i8 as f32,
        b"ui16" => be16()? as f32,
        b"si16" => be16()? as i16 as f32,
        b"ui32" => u32::from_be_bytes(data.get(..4)?.try_into().ok()?) as f32,
        b"flt " => f32::from_le_bytes(data.get(..4)?.try_into().ok()?),
        [b's' | b'f', b'p', i, f] => {
            let signed = t[0] == b's';
            let (int_bits, frac_bits) = (hex_digit(*i)?, hex_digit(*f)?);
            if int_bits + frac_bits != if signed { 15 } else { 16 } {
                return None;
            }
            let raw = be16()?;
            let raw = if signed { raw as i16 as f32 } else { raw as f32 };
            raw / (1u32 << frac_bits) as f32
        }
        _ => return None,
    };
    v.is_finite().then_some(v)
}

/// Name of a fan from its `F<n>ID` value (type `{fds}`: 4 bytes of type, zone
/// and location, then a 12-byte text such as `Left side`). `None` when the
/// text is empty or not printable.
pub fn parse_fan_name(bytes: &[u8]) -> Option<String> {
    let text = bytes.get(4..)?;
    let text = &text[..text.iter().position(|&b| b == 0).unwrap_or(text.len())];
    if !text.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        return None;
    }
    let s = String::from_utf8_lossy(text).trim().to_string();
    (!s.is_empty()).then_some(s)
}

// -- Intel SMC temperature keys ---------------------------------------------------

/// A known SMC temperature key.
pub struct SmcTemp {
    pub key: &'static str,
    /// Hardware type in the vocabulary of `naming::classify`.
    pub hardware_type: &'static str,
    pub name: &'static str,
}

const fn t(key: &'static str, hardware_type: &'static str, name: &'static str) -> SmcTemp {
    SmcTemp { key, hardware_type, name }
}

/// SMC temperature keys worth probing. Keys that a Mac does not have are
/// absent from its SMC and are skipped when probing.
pub const SMC_TEMPS: &[SmcTemp] = &[
    t("TC0P", "Cpu", "CPU Proximity"),
    t("TC0D", "Cpu", "CPU Die"),
    t("TC0E", "Cpu", "CPU Die (virtual)"),
    t("TC0F", "Cpu", "CPU Die (filtered)"),
    t("TC0H", "Cpu", "CPU Heatsink"),
    t("TCXC", "Cpu", "CPU PECI"),
    t("TC1C", "Cpu", "CPU Core 1"),
    t("TC2C", "Cpu", "CPU Core 2"),
    t("TC3C", "Cpu", "CPU Core 3"),
    t("TC4C", "Cpu", "CPU Core 4"),
    t("TC5C", "Cpu", "CPU Core 5"),
    t("TC6C", "Cpu", "CPU Core 6"),
    t("TC7C", "Cpu", "CPU Core 7"),
    t("TC8C", "Cpu", "CPU Core 8"),
    t("TG0P", "Gpu", "GPU Core (proximity)"),
    t("TG0D", "Gpu", "GPU Core (diode)"),
    t("TG0H", "Gpu", "GPU Heatsink"),
    t("Tm0P", "Memory", "Memory Proximity"),
    t("TM0P", "Memory", "Memory Slot Proximity"),
    t("TH0P", "Storage", "Drive Proximity"),
    t("TPCD", "Motherboard", "Platform Controller Hub"),
    t("TN0P", "Motherboard", "Northbridge Proximity"),
    t("TN0D", "Motherboard", "Northbridge Die"),
    t("TB0T", "Battery", "Battery 1"),
    t("TB1T", "Battery", "Battery 2"),
    t("TA0P", "Other", "Ambient"),
    t("Ts0P", "Other", "Palm Rest"),
    t("TW0P", "Other", "Wi-Fi Proximity"),
    t("TL0P", "Other", "Display Proximity"),
    t("Th0H", "Other", "Heatpipe 1"),
    t("Th1H", "Other", "Heatpipe 2"),
    t("Th2H", "Other", "Heatpipe 3"),
];

/// SMC keys tried in order for the Auto CPU temperature on Intel.
const AUTO_SMC_KEYS: [&str; 4] = ["TC0P", "TC0D", "TC0E", "TC0F"];

/// Id of the synthetic average of the CPU die sensors (Apple Silicon).
pub const CPU_DIE_AVERAGE_ID: &str = "iohid/cpu-die-average";

// -- Apple Silicon HID names --------------------------------------------------------

/// How a HID sensor takes part in the CPU die average.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidGroup {
    /// `PMU tdie*`: the CPU die sensors, preferred for the average.
    CpuDie,
    /// `pACC`/`eACC MTR Temp Sensor*`: per-core sensors of newer chips, used
    /// for the average only when there are no die sensors.
    CpuCore,
    Other,
}

/// Friendly identity of a HID temperature sensor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidInfo {
    pub hardware_type: &'static str,
    pub name: String,
    pub group: HidGroup,
}

/// Splits a trailing number off a name (`PMU tdie12` -> `PMU tdie`, 12).
fn split_number(s: &str) -> (&str, Option<u32>) {
    let stem = s.trim_end_matches(|c: char| c.is_ascii_digit());
    (stem, s[stem.len()..].parse().ok())
}

/// Maps the `Product` property of a HID temperature service to a friendly
/// name and a hardware type. `None` for services that are not readings
/// (`PMU tcal` is a calibration value).
pub fn classify_hid(product: &str) -> Option<HidInfo> {
    let p = product.trim();
    let l = p.to_ascii_lowercase();
    let n = split_number(p).1;
    let numbered = |base: &str| match n {
        Some(n) => format!("{base} {n}"),
        None => base.to_string(),
    };
    let info = |hardware_type, name: String, group| Some(HidInfo { hardware_type, name, group });
    if l.is_empty() || l.contains("tcal") {
        None
    } else if l.contains("tdie") {
        info("Cpu", numbered("CPU Die"), HidGroup::CpuDie)
    } else if l.contains("tdev") {
        info("Cpu", numbered("SoC Device"), HidGroup::Other)
    } else if l.starts_with("pacc") {
        info("Cpu", numbered("CPU Performance"), HidGroup::CpuCore)
    } else if l.starts_with("eacc") {
        info("Cpu", numbered("CPU Efficiency"), HidGroup::CpuCore)
    } else if l.starts_with("soc") {
        info("Cpu", numbered("SoC"), HidGroup::Other)
    } else if l.starts_with("gpu") {
        info("GpuApple", numbered("GPU Core"), HidGroup::Other)
    } else if l.starts_with("ane") {
        info("Other", numbered("Neural Engine"), HidGroup::Other)
    } else if l.contains("nand") {
        let channel = p.split_whitespace().find(|w| w.to_ascii_lowercase().starts_with("ch"));
        let name = channel.map_or("NAND".to_string(), |c| format!("NAND {}", c.to_ascii_uppercase()));
        info("Storage", name, HidGroup::Other)
    } else if l.contains("battery") || l.contains("gas gauge") {
        info("Battery", "Battery".into(), HidGroup::Other)
    } else {
        info("Other", p.to_string(), HidGroup::Other)
    }
}

/// Position of a HID sensor in the list: CPU first (die, cores, the rest),
/// then GPU, storage, battery and anything else; natural order inside.
fn hid_rank(info: &HidInfo) -> u8 {
    match (info.hardware_type, info.group) {
        ("Cpu", HidGroup::CpuDie) => 0,
        ("Cpu", HidGroup::CpuCore) => 1,
        ("Cpu", _) => 2,
        ("GpuApple", _) => 3,
        ("Storage", _) => 4,
        ("Battery", _) => 5,
        _ => 6,
    }
}

// -- raw samples and sensor building --------------------------------------------------

/// A HID temperature service as read: its `Product` and the value in °C.
#[derive(Debug, Clone, PartialEq)]
pub struct HidTemp {
    pub product: String,
    pub value: f32,
}

/// An SMC temperature key as read.
#[derive(Debug, Clone, PartialEq)]
pub struct SmcTempReading {
    pub key: String,
    pub value: f32,
}

/// An SMC fan as read.
#[derive(Debug, Clone, PartialEq)]
pub struct FanReading {
    pub index: u32,
    /// From `F<n>ID`, when the SMC has one.
    pub name: Option<String>,
    pub rpm: f32,
}

/// Everything one pass over IOKit returns.
#[derive(Debug, Clone, Default)]
pub struct MacSample {
    pub hid: Vec<HidTemp>,
    pub smc_temps: Vec<SmcTempReading>,
    pub fans: Vec<FanReading>,
}

/// Where a [`MacSource`] gets its raw samples. The real one is the IOKit
/// backend; tests plug in a fake.
pub trait MacBackend: Send {
    fn sample(&mut self) -> MacSample;
}

/// A plausible temperature: sensors that are not connected read 0 or
/// absurdly low values.
fn valid_temp(v: f32) -> bool {
    v.is_finite() && v > 0.0 && v <= 150.0
}

fn hardware_for(hardware_type: &str, chip: &str) -> String {
    let chip = if chip.trim().is_empty() { "Mac" } else { chip.trim() };
    match hardware_type {
        "Cpu" | "GpuApple" => chip.to_string(),
        "Gpu" => "GPU".into(),
        "Memory" => "Memory".into(),
        "Storage" => "Storage".into(),
        "Motherboard" => "Logic board".into(),
        "Battery" => "Battery".into(),
        _ => "Mac".into(),
    }
}

/// Turns raw samples into the sensor list: friendly names, hardware types,
/// stable ids (`iohid/<Product>`, `smc/<KEY>`). `chip` is the CPU brand string
/// (`Apple M1 Pro`, `Intel(R) Core(TM) i7-...`).
pub fn build_sensors(
    chip: &str,
    hid: &[HidTemp],
    smc_temps: &[SmcTempReading],
    fans: &[FanReading],
) -> Vec<SensorValue> {
    let mut out: Vec<SensorValue> = Vec::new();
    let push = |out: &mut Vec<SensorValue>, mut id: String, ty: &str, mut name: String, kind: SensorKind, value: f32| {
        // Two services with the same Product, or with the same friendly name.
        if out.iter().any(|s| s.id == id) {
            let dup = out.iter().filter(|s| s.id.starts_with(&id)).count() + 1;
            id = format!("{id}#{dup}");
        }
        if out.iter().any(|s| s.name == name && s.hardware_type == ty && s.kind == kind) {
            let dup = out.iter().filter(|s| s.name.starts_with(&name) && s.kind == kind).count() + 1;
            name = format!("{name} ({dup})");
        }
        out.push(SensorValue {
            id,
            hardware: hardware_for(ty, chip),
            hardware_type: ty.into(),
            name,
            kind,
            value,
        });
    };

    // Apple Silicon: HID sensors, led by the CPU die average.
    let mut classified: Vec<(&HidTemp, HidInfo)> = hid
        .iter()
        .filter(|h| valid_temp(h.value))
        .filter_map(|h| classify_hid(&h.product).map(|i| (h, i)))
        .collect();
    classified.sort_by_key(|(h, i)| {
        let (stem, n) = split_number(h.product.trim());
        (hid_rank(i), stem.to_ascii_lowercase(), n)
    });
    let average_of = |group| {
        let v: Vec<f32> = classified.iter().filter(|(_, i)| i.group == group).map(|(h, _)| h.value).collect();
        (!v.is_empty()).then(|| v.iter().sum::<f32>() / v.len() as f32)
    };
    if let Some(avg) = average_of(HidGroup::CpuDie).or_else(|| average_of(HidGroup::CpuCore)) {
        push(&mut out, CPU_DIE_AVERAGE_ID.into(), "Cpu", "CPU Die (average)".into(), SensorKind::Temp, avg);
    }
    for (h, info) in classified {
        push(&mut out, format!("iohid/{}", h.product.trim()), info.hardware_type, info.name, SensorKind::Temp, h.value);
    }

    // Intel: SMC keys, in table order.
    for def in SMC_TEMPS {
        if let Some(r) = smc_temps.iter().find(|r| r.key == def.key && valid_temp(r.value)) {
            push(&mut out, format!("smc/{}", def.key), def.hardware_type, def.name.into(), SensorKind::Temp, r.value);
        }
    }

    // Fans, by index.
    let mut fans: Vec<&FanReading> =
        fans.iter().filter(|f| f.rpm.is_finite() && (0.0..=100_000.0).contains(&f.rpm)).collect();
    fans.sort_by_key(|f| f.index);
    for f in fans {
        let name = match &f.name {
            Some(n) => format!("Fan {} ({n})", f.index + 1),
            None => format!("Fan {}", f.index + 1),
        };
        push(&mut out, format!("smc/F{}Ac", f.index), "Motherboard", name, SensorKind::Fan, f.rpm);
    }
    out
}

/// The automatic CPU temperature: the CPU die average (Apple Silicon), else
/// the Intel SMC `TC0P`, `TC0D`, `TC0E` or `TC0F`. `None` leaves the `sysinfo`
/// estimate in place.
pub fn pick_cpu_temp(sensors: &[SensorValue]) -> Option<f32> {
    let temp = |id: &str| sensors.iter().find(|s| s.kind == SensorKind::Temp && s.id == id).map(|s| s.value);
    temp(CPU_DIE_AVERAGE_ID).or_else(|| AUTO_SMC_KEYS.iter().find_map(|k| temp(&format!("smc/{k}"))))
}

/// The automatic fan: `F0`, else the first fan. A fan reading 0 RPM is
/// reported as such: Macs stop their fans when cool.
pub fn pick_fan(sensors: &[SensorValue]) -> Option<u32> {
    let fans = || sensors.iter().filter(|s| s.kind == SensorKind::Fan);
    fans()
        .find(|s| s.id == "smc/F0Ac")
        .or_else(|| fans().next())
        .map(|s| s.value.round().max(0.0) as u32)
}

// -- the macOS source -------------------------------------------------------------------

/// Default source on macOS: CPU/RAM usage from `sysinfo`, temperatures and
/// fans from IOKit (HID sensors and the SMC).
pub struct MacSource {
    sys: SysinfoSource,
    chip: String,
    backend: Option<Box<dyn MacBackend>>,
}

impl MacSource {
    /// The real source. When IOKit cannot be reached the readings are empty
    /// and `sysinfo`'s estimate is all there is.
    #[cfg(target_os = "macos")]
    pub fn new() -> Self {
        let mut s = sysinfo::System::new();
        s.refresh_cpu_all();
        let chip = s.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default();
        Self::with_backend(chip, Box::new(iokit::IoKitBackend::open()))
    }

    /// A source over a custom backend, for tests.
    pub fn with_backend(chip: String, backend: Box<dyn MacBackend>) -> Self {
        Self { sys: SysinfoSource::new(), chip, backend: Some(backend) }
    }
}

#[cfg(target_os = "macos")]
impl Default for MacSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorSource for MacSource {
    fn read(&mut self) -> Result<Reading> {
        let mut r = self.sys.read()?;
        let Some(backend) = self.backend.as_mut() else { return Ok(r) };
        let s = backend.sample();
        let sensors = build_sensors(&self.chip, &s.hid, &s.smc_temps, &s.fans);
        // sysinfo's guess stays as the fallback when IOKit has no CPU sensor.
        r.cpu_temp_c = pick_cpu_temp(&sensors).or(r.cpu_temp_c);
        r.fan_rpm = pick_fan(&sensors);
        r.sensors = sensors;
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::{classify, Category};
    use crate::selection::{fan_options, select_fan, select_temp, temp_options, FanSource, TempSource};

    fn k(s: &str) -> u32 {
        fourcc(s)
    }

    fn hid(p: &str, v: f32) -> HidTemp {
        HidTemp { product: p.into(), value: v }
    }

    fn smc(key: &str, v: f32) -> SmcTempReading {
        SmcTempReading { key: key.into(), value: v }
    }

    fn names(s: &[SensorValue]) -> Vec<&str> {
        s.iter().map(|s| s.name.as_str()).collect()
    }

    fn by_id<'a>(s: &'a [SensorValue], id: &str) -> &'a SensorValue {
        s.iter().find(|s| s.id == id).unwrap_or_else(|| panic!("no sensor {id}"))
    }

    #[test]
    fn smc_key_data_matches_the_c_layout() {
        use std::mem::{offset_of, size_of};
        assert_eq!(size_of::<SmcKeyData>(), 80);
        assert_eq!(offset_of!(SmcKeyData, vers), 4);
        assert_eq!(offset_of!(SmcKeyData, p_limit), 12);
        assert_eq!(offset_of!(SmcKeyData, key_info), 28);
        assert_eq!(offset_of!(SmcKeyData, result), 40);
        assert_eq!(offset_of!(SmcKeyData, data8), 42);
        assert_eq!(offset_of!(SmcKeyData, data32), 44);
        assert_eq!(offset_of!(SmcKeyData, bytes), 48);
    }

    #[test]
    fn fourcc_round_trips() {
        assert_eq!(k("TC0P"), 0x5443_3050);
        assert_eq!(fourcc_to_string(k("flt ")), "flt ");
        assert_eq!(k("abc"), 0);
        assert_eq!(k("abcde"), 0);
    }

    #[test]
    fn decodes_the_smc_formats() {
        // sp78: signed 7.8 fixed point, big-endian. 0x2D80 = 45.5 degrees.
        assert_eq!(decode_smc(k("sp78"), &[0x2D, 0x80]), Some(45.5));
        assert_eq!(decode_smc(k("sp78"), &[0xFF, 0x00]), Some(-1.0));
        // fpe2: unsigned 14.2. 0x1130 = 4400 / 4 = 1100 RPM.
        assert_eq!(decode_smc(k("fpe2"), &[0x11, 0x30]), Some(1100.0));
        // flt: little-endian float.
        assert_eq!(decode_smc(k("flt "), &1234.5f32.to_le_bytes()), Some(1234.5));
        assert_eq!(decode_smc(k("flt "), &f32::NAN.to_le_bytes()), None);
        // Integers are big-endian.
        assert_eq!(decode_smc(k("ui8 "), &[2]), Some(2.0));
        assert_eq!(decode_smc(k("ui16"), &[0x01, 0x02]), Some(258.0));
        assert_eq!(decode_smc(k("ui32"), &[0, 1, 0, 0]), Some(65536.0));
        assert_eq!(decode_smc(k("si8 "), &[0xFF]), Some(-1.0));
        assert_eq!(decode_smc(k("si16"), &[0xFF, 0xFE]), Some(-2.0));
        // Other fixed-point widths, and the bit-count check.
        assert_eq!(decode_smc(k("sp87"), &[0x01, 0x80]), Some(3.0));
        assert_eq!(decode_smc(k("fp88"), &[0x01, 0x80]), Some(1.5));
        assert_eq!(decode_smc(k("sp99"), &[0x01, 0x80]), None);
        // Unknown type, or not enough bytes.
        assert_eq!(decode_smc(k("{fds"), &[0; 16]), None);
        assert_eq!(decode_smc(k("sp78"), &[0x2D]), None);
        assert_eq!(decode_smc(k("flt "), &[0, 0]), None);
    }

    #[test]
    fn fan_names_come_from_the_fds_struct() {
        let mut b = vec![1, 0, 0, 0];
        b.extend_from_slice(b"Left side\0\0\0");
        assert_eq!(parse_fan_name(&b).as_deref(), Some("Left side"));
        assert_eq!(parse_fan_name(&[1, 0, 0, 0, 0, 0, 0, 0]), None);
        assert_eq!(parse_fan_name(&[1, 0, 0, 0, 0x80, 0x81]), None);
        assert_eq!(parse_fan_name(&[1, 2]), None);
    }

    #[test]
    fn smc_table_has_unique_keys() {
        for (i, a) in SMC_TEMPS.iter().enumerate() {
            assert_eq!(a.key.len(), 4, "{}", a.key);
            assert!(SMC_TEMPS[i + 1..].iter().all(|b| b.key != a.key), "{}", a.key);
        }
        for key in AUTO_SMC_KEYS {
            assert!(SMC_TEMPS.iter().any(|d| d.key == key));
        }
    }

    #[test]
    fn hid_names_map_to_friendly_names_and_types() {
        let c = |p| classify_hid(p).unwrap();
        assert_eq!(c("PMU tdie3").name, "CPU Die 3");
        assert_eq!(c("PMU tdie3").hardware_type, "Cpu");
        assert_eq!(c("PMU2 tdie1").name, "CPU Die 1");
        assert_eq!(c("PMU tdev2").name, "SoC Device 2");
        assert_eq!(c("pACC MTR Temp Sensor0").name, "CPU Performance 0");
        assert_eq!(c("eACC MTR Temp Sensor1").name, "CPU Efficiency 1");
        assert_eq!(c("GPU MTR Temp Sensor2").hardware_type, "GpuApple");
        assert_eq!(c("GPU MTR Temp Sensor2").name, "GPU Core 2");
        assert_eq!(c("NAND CH0 temp").name, "NAND CH0");
        assert_eq!(c("NAND CH0 temp").hardware_type, "Storage");
        assert_eq!(c("gas gauge battery").hardware_type, "Battery");
        assert_eq!(c("ISP MTR Temp Sensor5").name, "ISP MTR Temp Sensor5");
        assert_eq!(c("PMU tdie1").group, HidGroup::CpuDie);
        assert_eq!(c("PMU tdev1").group, HidGroup::Other);
        assert!(classify_hid("PMU tcal").is_none());
        assert!(classify_hid("  ").is_none());
    }

    fn apple_silicon() -> Vec<SensorValue> {
        let hid = [
            hid("PMU tdie10", 60.0),
            hid("PMU tdie2", 50.0),
            hid("PMU tdie1", 40.0),
            hid("PMU tdev1", 35.0),
            hid("PMU tcal", 30.0),
            hid("NAND CH0 temp", 33.0),
            hid("gas gauge battery", 31.0),
            hid("GPU MTR Temp Sensor1", 44.0),
            hid("SOC MTR Temp Sensor0", -9199.0), // disconnected
        ];
        let fans = [FanReading { index: 0, name: None, rpm: 0.0 }];
        build_sensors("Apple M1 Pro", &hid, &[], &fans)
    }

    #[test]
    fn apple_silicon_sensors_are_named_and_ordered() {
        let s = apple_silicon();
        assert_eq!(
            names(&s),
            [
                "CPU Die (average)", "CPU Die 1", "CPU Die 2", "CPU Die 10", "SoC Device 1",
                "GPU Core 1", "NAND CH0", "Battery", "Fan 1",
            ]
        );
        assert_eq!(by_id(&s, "iohid/PMU tdie2").hardware, "Apple M1 Pro");
        assert_eq!(by_id(&s, "iohid/NAND CH0 temp").hardware, "Storage");
    }

    #[test]
    fn apple_silicon_categories() {
        let s = apple_silicon();
        let cat = |id| classify(by_id(&s, id));
        assert_eq!(cat(CPU_DIE_AVERAGE_ID), Category::Cpu);
        assert_eq!(cat("iohid/PMU tdev1"), Category::Cpu);
        assert_eq!(cat("iohid/GPU MTR Temp Sensor1"), Category::Gpu);
        assert_eq!(cat("iohid/NAND CH0 temp"), Category::Storage);
        assert_eq!(cat("iohid/gas gauge battery"), Category::Other);
        assert_eq!(cat("smc/F0Ac"), Category::Motherboard);
    }

    #[test]
    fn apple_silicon_auto_is_the_die_average() {
        let s = apple_silicon();
        assert_eq!(by_id(&s, CPU_DIE_AVERAGE_ID).value, 50.0); // (40 + 50 + 60) / 3
        assert_eq!(pick_cpu_temp(&s), Some(50.0));
        // A fan at 0 RPM is a real reading on a Mac.
        assert_eq!(pick_fan(&s), Some(0));
    }

    #[test]
    fn newer_chips_average_the_core_sensors_without_die_sensors() {
        let hid = [hid("pACC MTR Temp Sensor0", 50.0), hid("eACC MTR Temp Sensor0", 40.0), hid("PMU tdev1", 90.0)];
        let s = build_sensors("Apple M3", &hid, &[], &[]);
        assert_eq!(pick_cpu_temp(&s), Some(45.0));
    }

    fn intel() -> Vec<SensorValue> {
        let temps = [
            smc("TG0D", 60.0),
            smc("TC0D", 66.0),
            smc("TC0P", 55.0),
            smc("Ts0P", 0.0), // absent sensors read as 0
            smc("TB0T", 30.0),
            smc("XXXX", 99.0), // not in the table
        ];
        let fans = [
            FanReading { index: 1, name: Some("Right side".into()), rpm: 2100.0 },
            FanReading { index: 0, name: Some("Left side".into()), rpm: 1800.0 },
        ];
        build_sensors("Intel(R) Core(TM) i9-9980HK", &[], &temps, &fans)
    }

    #[test]
    fn intel_sensors_and_ids() {
        let s = intel();
        assert_eq!(
            names(&s),
            [
                "CPU Proximity", "CPU Die", "GPU Core (diode)", "Battery 1",
                "Fan 1 (Left side)", "Fan 2 (Right side)",
            ]
        );
        assert_eq!(by_id(&s, "smc/TC0P").hardware, "Intel(R) Core(TM) i9-9980HK");
        assert_eq!(classify(by_id(&s, "smc/TG0D")), Category::Gpu);
        assert_eq!(classify(by_id(&s, "smc/TB0T")), Category::Other);
    }

    #[test]
    fn intel_auto_picks() {
        let s = intel();
        assert_eq!(pick_cpu_temp(&s), Some(55.0)); // TC0P before TC0D
        assert_eq!(pick_fan(&s), Some(1800)); // F0
        // Without TC0P the next key is used.
        let s = build_sensors("Intel", &[], &[smc("TC0D", 66.0), smc("TC0F", 70.0)], &[]);
        assert_eq!(pick_cpu_temp(&s), Some(66.0));
        // Without F0 the first fan is.
        let fans = [FanReading { index: 1, name: None, rpm: 900.0 }];
        assert_eq!(pick_fan(&build_sensors("Intel", &[], &[], &fans)), Some(900));
    }

    #[test]
    fn fanless_or_sensorless_macs_pick_nothing() {
        let s = build_sensors("Apple M2", &[], &[], &[]);
        assert!(s.is_empty());
        assert_eq!(pick_cpu_temp(&s), None);
        assert_eq!(pick_fan(&s), None);
    }

    #[test]
    fn duplicate_products_get_distinct_ids_and_names() {
        let hid = [hid("PMU tdie1", 40.0), hid("PMU tdie1", 42.0)];
        let s = build_sensors("Apple M1", &hid, &[], &[]);
        let ids: Vec<&str> = s.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, [CPU_DIE_AVERAGE_ID, "iohid/PMU tdie1", "iohid/PMU tdie1#2"]);
        assert_eq!(names(&s), ["CPU Die (average)", "CPU Die 1", "CPU Die 1 (2)"]);
    }

    #[test]
    fn selectors_work_with_the_macos_sensors() {
        let s = apple_silicon();
        let r = Reading {
            cpu_temp_c: pick_cpu_temp(&s),
            fan_rpm: pick_fan(&s),
            sensors: s,
            ..Default::default()
        };
        // Max CPU/GPU compares the CPU average (50) with the GPU core (44).
        assert_eq!(select_temp(&TempSource::MaxCpuGpu, &r), Some(50.0));
        assert_eq!(select_temp(&TempSource::Sensor("iohid/PMU tdie10".into()), &r), Some(60.0));
        assert_eq!(select_fan(&FanSource::Sensor("smc/F0Ac".into()), &r), Some(0));
        let t = temp_options(&r, &TempSource::Auto);
        assert_eq!(t[0].secondary, "CPU · CPU Die (average)");
        assert!(t.iter().any(|o| o.group == "Storage" && o.primary == "NAND CH0"));
        assert_eq!(fan_options(&r, &FanSource::Auto)[1].primary, "Fan 1");
    }

    struct Fake(MacSample);
    impl MacBackend for Fake {
        fn sample(&mut self) -> MacSample {
            self.0.clone()
        }
    }

    #[test]
    fn mac_source_fills_the_reading() {
        let sample = MacSample {
            hid: vec![hid("PMU tdie1", 40.0), hid("PMU tdie2", 60.0)],
            smc_temps: vec![],
            fans: vec![FanReading { index: 0, name: None, rpm: 1500.4 }],
        };
        let mut src = MacSource::with_backend("Apple M1".into(), Box::new(Fake(sample)));
        let r = src.read().unwrap();
        assert_eq!(r.cpu_temp_c, Some(50.0));
        assert_eq!(r.fan_rpm, Some(1500));
        assert_eq!(r.sensors.len(), 4);
    }
}
