//! Human-friendly names and categories for sensors.
//!
//! Pure helpers behind the source selectors: cleaning up names that come from
//! drivers (control characters, mojibake), deciding which "sensors" are really
//! constant thresholds rather than readings, and grouping sensors into
//! categories a user can recognize (a Super I/O chip is "Motherboard").

use crate::sensors::{SensorKind, SensorValue};

/// Friendly category of a sensor. The declaration order is the display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Cpu,
    Gpu,
    LiquidPump,
    Motherboard,
    Memory,
    Storage,
    Other,
}

impl Category {
    /// Name shown in group headers and secondary lines.
    pub fn title(self) -> &'static str {
        match self {
            Category::Cpu => "CPU",
            Category::Gpu => "GPU",
            Category::LiquidPump => "Liquid / Pump",
            Category::Motherboard => "Motherboard",
            Category::Memory => "Memory",
            Category::Storage => "Storage",
            Category::Other => "Other",
        }
    }
}

/// Whether a character is a sign of a corrupted string: control characters,
/// the replacement character, private-use and non-character code points.
fn is_garbage_char(c: char) -> bool {
    c.is_control()
        || c == '\u{FFFD}'
        || ('\u{E000}'..='\u{F8FF}').contains(&c)
        || ('\u{FDD0}'..='\u{FDEF}').contains(&c)
        || (c as u32) & 0xFFFE == 0xFFFE
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Cleans a driver-provided name: collapses whitespace and, when the text is
/// corrupted (replacement characters, control bytes), keeps only the
/// trustworthy part before the first bad character.
///
/// A trailing instance marker such as `(#3)` is preserved. Returns an empty
/// string when too little readable text is left.
pub fn sanitize_name(raw: &str) -> String {
    if !raw.chars().any(is_garbage_char) {
        return collapse_ws(raw);
    }
    let suffix = instance_suffix(raw).map(|n| format!(" (#{n})")).unwrap_or_default();
    let head: String = raw.chars().take_while(|c| !is_garbage_char(*c)).collect();
    let head = collapse_ws(&head);
    let head = head.trim_end_matches(|c: char| !c.is_alphanumeric() && c != ')' && c != ']');
    if head.chars().filter(|c| c.is_alphanumeric()).count() < 4 {
        return String::new();
    }
    format!("{head}{suffix}")
}

/// Instance number from a trailing `(#N)` marker, e.g. the DIMM slot.
pub fn instance_suffix(s: &str) -> Option<u32> {
    let inner = s.trim_end().strip_suffix(')')?;
    let start = inner.rfind("(#")?;
    inner[start + 2..].parse().ok()
}

/// `s` without a trailing `(#N)` marker.
fn strip_instance(s: &str) -> &str {
    let t = s.trim_end();
    if instance_suffix(t).is_some() {
        if let Some(p) = t.rfind("(#") {
            return t[..p].trim_end();
        }
    }
    t
}

/// Whether a temperature "sensor" is really a constant threshold or a derived
/// value that is not a temperature (warning/critical/limit levels, distance
/// to TjMax, min/max thresholds).
pub fn is_threshold_temp(name: &str) -> bool {
    let n = name.to_lowercase();
    const WORDS: [&str; 17] = [
        "temperature max", "temperature min",
        "warning", "critical", "limit", "alarm", "threshold", "hyst", "tjmax", "tj max",
        "distance to", "maximum", "minimum", "max temp", "min temp", "temp max", "temp min",
    ];
    WORDS.iter().any(|w| n.contains(w))
}

/// Whether a fan "sensor" is a control/percentage value rather than a speed.
pub fn is_non_rpm_fan(name: &str) -> bool {
    let n = name.to_lowercase();
    ["control", "pwm", "duty", "percent", "%", "level", "limit", "threshold"]
        .iter()
        .any(|w| n.contains(w))
}

/// Whether the sensor is a real reading that belongs in its selector.
pub fn is_reading(s: &SensorValue) -> bool {
    match s.kind {
        SensorKind::Temp => !is_threshold_temp(&s.name),
        SensorKind::Fan => !is_non_rpm_fan(&s.name),
    }
}

fn has_any(hay: &str, keys: &[&str]) -> bool {
    keys.iter().any(|k| hay.contains(k))
}

/// Classifies a sensor: hardware type first, name heuristics second.
pub fn classify(s: &SensorValue) -> Category {
    let ht = s.hardware_type.to_lowercase();
    if ht == "cpu" {
        return Category::Cpu;
    }
    if ht.starts_with("gpu") {
        return Category::Gpu;
    }
    if ht == "memory" {
        return Category::Memory;
    }
    if ht == "storage" {
        return Category::Storage;
    }
    let name = s.name.to_lowercase();
    if ht == "cooler" || has_any(&name, &["coolant", "liquid", "water", "pump", "aio"]) {
        return Category::LiquidPump;
    }
    if has_any(&ht, &["superio", "motherboard", "lpc", "chipset", "embedded"]) {
        return Category::Motherboard;
    }
    let hw = s.hardware.to_lowercase();
    if has_any(&name, &["cpu", "tctl", "tdie", "package"])
        || has_any(&hw, &["ryzen", "intel core", "xeon"])
    {
        return Category::Cpu;
    }
    if name.contains("gpu") || has_any(&hw, &["geforce", "radeon", "nvidia"]) {
        return Category::Gpu;
    }
    if has_any(&name, &["dimm", "memory"]) {
        return Category::Memory;
    }
    if has_any(&name, &["ssd", "nvme", "hdd", "drive"]) || has_any(&hw, &["ssd", "nvme", "hdd"]) {
        return Category::Storage;
    }
    if has_any(&name, &["motherboard", "chipset", "vrm", "system"]) {
        return Category::Motherboard;
    }
    Category::Other
}

/// How representative a CPU sensor is as "the" CPU temperature (lower is better).
pub fn cpu_rank(name: &str) -> u8 {
    let n = name.to_lowercase();
    if n.contains("package") {
        0
    } else if n.contains("ccd") {
        3
    } else if n.contains("tctl") || n.contains("tdie") {
        1
    } else if n.contains("core (max") || n.contains("core max") {
        2
    } else if n.contains("core") {
        3
    } else {
        4
    }
}

/// Slot number of a memory module, from the hardware name (`... (#3)`) or a
/// sensor name such as `DIMM #3`.
fn memory_slot(s: &SensorValue) -> Option<u32> {
    instance_suffix(&s.hardware).or_else(|| {
        let n = s.name.to_lowercase();
        let rest = n.strip_prefix("dimm")?.trim_start().trim_start_matches('#');
        rest.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
    })
}

/// Short hardware name for the secondary line.
pub fn short_hardware(s: &SensorValue, cat: Category) -> String {
    let clean = sanitize_name(&s.hardware);
    if cat == Category::Memory {
        return match (memory_slot(s), clean.is_empty()) {
            (Some(n), _) => format!("DIMM slot {n}"),
            (None, false) => strip_instance(&clean).to_string(),
            (None, true) => "Memory module".into(),
        };
    }
    let clean = collapse_ws(
        &clean.replace("(TM)", "").replace("(R)", "").replace("(tm)", "").replace("(r)", ""),
    );
    if clean.is_empty() {
        "Unknown device".into()
    } else {
        clean
    }
}

/// Friendly sensor name (primary text); a generic name when the original is
/// unreadable.
pub fn friendly_name(s: &SensorValue) -> String {
    let n = sanitize_name(&s.name);
    if !n.is_empty() {
        return n;
    }
    match s.kind {
        SensorKind::Temp => "Temperature".into(),
        SensorKind::Fan => "Fan".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sv(hw: &str, ty: &str, name: &str, kind: SensorKind) -> SensorValue {
        SensorValue {
            id: "/x".into(),
            hardware: hw.into(),
            hardware_type: ty.into(),
            name: name.into(),
            kind,
            value: 40.0,
        }
    }

    #[test]
    fn thresholds_from_real_machine_are_filtered() {
        for n in [
            "Warning Temperature",
            "Critical Temperature",
            "Thermal Sensor High Limit",
            "Thermal Sensor Critical Limit",
            "Core Distance to TjMax",
            "Distance to TjMax",
            "Temperature Max",
        ] {
            assert!(is_threshold_temp(n), "{n}");
        }
        for n in [
            "CPU Core",
            "Temperature #1",
            "Core (Tctl/Tdie)",
            "CCD1 (Tdie)",
            "DIMM #1",
            "Temperature Sensor",
            "GPU VR SoC",
            "Composite Temperature",
            "Core Max",
            "GPU Hot Spot",
        ] {
            assert!(!is_threshold_temp(n), "{n}");
        }
    }

    #[test]
    fn non_rpm_fans_are_filtered() {
        for n in ["Fan Control #1", "Pump PWM", "Fan Duty", "Fan Speed %", "Fan Level"] {
            assert!(is_non_rpm_fan(n), "{n}");
        }
        for n in ["Fan #2", "Pump", "CPU Fan", "Pump Speed"] {
            assert!(!is_non_rpm_fan(n), "{n}");
        }
    }

    #[test]
    fn sanitize_keeps_clean_names() {
        assert_eq!(sanitize_name("  Nuvoton   NCT6799D "), "Nuvoton NCT6799D");
        assert_eq!(sanitize_name("Core (Tctl/Tdie)"), "Core (Tctl/Tdie)");
        assert_eq!(sanitize_name("N\u{fa}cleo \u{b0}C"), "N\u{fa}cleo \u{b0}C");
    }

    #[test]
    fn sanitize_cuts_garbage_and_keeps_instance() {
        let raw = "Corsair - CMK32GX5M2\u{FFFD}?\u{FFFD}?r \u{FFFD}\u{FFFD}\u{FFFD}A\u{FFFD}A\u{FFFD}A\u{FFFD}} (#3)";
        assert_eq!(sanitize_name(raw), "Corsair - CMK32GX5M2 (#3)");
        assert_eq!(sanitize_name("ab\u{1}\u{2}\u{FFFD}"), "");
        assert_eq!(sanitize_name("\u{FFFD}\u{FFFD}\u{FFFD}"), "");
    }

    #[test]
    fn garbage_memory_name_falls_back_to_slot() {
        let raw = "Corsair - CMK32GX5M2\u{FFFD}?\u{FFFD}?r \u{FFFD}\u{FFFD}A\u{FFFD}} (#3)";
        let s = sv(raw, "Memory", "DIMM #3", SensorKind::Temp);
        assert_eq!(short_hardware(&s, Category::Memory), "DIMM slot 3");
        let s = sv("\u{FFFD}\u{FFFD}\u{FFFD}", "Memory", "Temperature Sensor", SensorKind::Temp);
        assert_eq!(short_hardware(&s, Category::Memory), "Memory module");
        let s = sv("Corsair - CMK32GX5M2B6000C38 (#1)", "Memory", "Temperature Sensor", SensorKind::Temp);
        assert_eq!(short_hardware(&s, Category::Memory), "DIMM slot 1");
        let s = sv("Corsair - CMK32GX5M2B6000C38", "Memory", "Temperature Sensor", SensorKind::Temp);
        assert_eq!(short_hardware(&s, Category::Memory), "Corsair - CMK32GX5M2B6000C38");
    }

    #[test]
    fn classification_of_real_machine() {
        let c = |hw, ty, n| classify(&sv(hw, ty, n, SensorKind::Temp));
        assert_eq!(c("Nuvoton NCT6799D", "SuperIO", "CPU Core"), Category::Motherboard);
        assert_eq!(c("Nuvoton NCT6799D", "SuperIO", "Temperature #1"), Category::Motherboard);
        assert_eq!(c("AMD Ryzen 7 9800X3D", "Cpu", "CCD1 (Tdie)"), Category::Cpu);
        assert_eq!(c("Corsair", "Memory", "DIMM #1"), Category::Memory);
        assert_eq!(c("AMD Radeon(TM) Graphics", "GpuAmd", "GPU VR SoC"), Category::Gpu);
        assert_eq!(c("X16 SSD 2TB", "Storage", "Composite Temperature"), Category::Storage);
        assert_eq!(c("Nuvoton NCT6799D", "SuperIO", "Pump"), Category::LiquidPump);
        assert_eq!(c("Kraken", "Cooler", "Coolant Temperature"), Category::LiquidPump);
        assert_eq!(c("HWiNFO", "Other", "Tctl/Tdie"), Category::Cpu);
        assert_eq!(c("Foo", "Network", "Chip"), Category::Other);
    }

    #[test]
    fn cpu_rank_prefers_package_then_tctl() {
        assert!(cpu_rank("CPU Package") < cpu_rank("Core (Tctl/Tdie)"));
        assert!(cpu_rank("Core (Tctl/Tdie)") < cpu_rank("CCD1 (Tdie)"));
        assert!(cpu_rank("CCD1 (Tdie)") < cpu_rank("Something"));
    }
}
