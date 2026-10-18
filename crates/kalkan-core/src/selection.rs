//! Choice of which sensor feeds each of the display's two fields.
//!
//! The frame carries exactly two values (temperature byte + fan RPM `u16`).
//! This module holds the **pure** selection logic: given a [`Reading`] and the
//! user's choice, it returns the value to send. A choice that cannot be
//! satisfied (the sensor vanished) falls back to `Auto` for that field instead
//! of producing a bogus 0.

use serde::{Deserialize, Serialize};

use crate::naming::{classify, cpu_rank, friendly_name, is_reading, short_hardware, Category};
use crate::sensors::{Reading, SensorKind, SensorValue};

/// Source of the temperature field.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TempSource {
    /// The automatic CPU temperature (`Reading::cpu_temp_c`).
    #[default]
    Auto,
    /// The highest of the automatic CPU temperature and the hottest GPU
    /// core/hot-spot temperature.
    MaxCpuGpu,
    /// A specific temperature sensor, by id.
    Sensor(String),
}

/// Source of the RPM field.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FanSource {
    /// The automatic CPU fan/pump (`Reading::fan_rpm`).
    #[default]
    Auto,
    /// A specific fan/pump sensor, by id.
    Sensor(String),
}

fn find<'a>(r: &'a Reading, id: &str, kind: SensorKind) -> Option<&'a SensorValue> {
    r.sensors.iter().find(|s| s.kind == kind && s.id == id)
}

/// Whether a sensor is a GPU core / hot-spot temperature (the ones that matter
/// for cooling; memory-junction and the like are left out).
fn is_gpu_core_temp(s: &SensorValue) -> bool {
    if s.kind != SensorKind::Temp || !s.hardware_type.to_lowercase().starts_with("gpu") {
        return false;
    }
    let n = s.name.to_lowercase();
    n.contains("core") || n.contains("hot spot") || n.contains("hotspot")
}

/// Hottest GPU core/hot-spot sensor, if any GPU sensor is present.
fn hottest_gpu(r: &Reading) -> Option<&SensorValue> {
    r.sensors
        .iter()
        .filter(|s| is_gpu_core_temp(s) && is_reading(s))
        .max_by(|a, b| a.value.total_cmp(&b.value))
}

/// How much hotter (°C) the other side of Max CPU/GPU must be before the
/// choice switches to it.
const SWITCH_MARGIN_C: f32 = 1.0;

/// Which side currently wins Max CPU/GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Cpu,
    Gpu,
}

/// What Max CPU/GPU resolves to for one reading.
struct MaxPick<'a> {
    value: f32,
    side: Side,
    /// The winning sensor; `None` when the CPU wins and its sensor is unknown.
    sensor: Option<&'a SensorValue>,
    /// The GPU sensor in use, kept even while the CPU wins.
    gpu: Option<&'a SensorValue>,
}

/// Memory of Max CPU/GPU between readings, so it does not flip back and forth
/// when the CPU and the GPU run at about the same temperature.
///
/// The winning side only changes when the other side is hotter by at least
/// [`SWITCH_MARGIN_C`], and the same goes for the GPU sensor in use when
/// several qualify. Call [`MaxTracker::update`] once per new reading; the
/// selection functions only read it. A default tracker (no history) decides
/// from the reading alone.
#[derive(Debug, Clone, Default)]
pub struct MaxTracker {
    side: Option<Side>,
    gpu_id: Option<String>,
}

impl MaxTracker {
    /// Remembers what Max CPU/GPU resolves to for `r`.
    pub fn update(&mut self, r: &Reading) {
        let pick = self.pick(r);
        self.side = pick.as_ref().map(|p| p.side);
        self.gpu_id = pick.and_then(|p| p.gpu).map(|g| g.id.clone());
    }

    fn pick<'a>(&self, r: &'a Reading) -> Option<MaxPick<'a>> {
        // The GPU sensor by the usual rule (the hottest), unless the one in use
        // is within the margin of it.
        let best = hottest_gpu(r);
        let kept = self.gpu_id.as_deref().and_then(|id| {
            r.sensors.iter().find(|s| s.id == id && is_gpu_core_temp(s) && is_reading(s))
        });
        let gpu = match (best, kept) {
            (Some(b), Some(k)) if b.value - k.value < SWITCH_MARGIN_C => Some(k),
            (b, _) => b,
        };
        let cpu = r.cpu_temp_c;
        let side = match (cpu, gpu.map(|g| g.value)) {
            (Some(c), Some(g)) => match self.side {
                Some(Side::Gpu) if c - g < SWITCH_MARGIN_C => Side::Gpu,
                Some(Side::Gpu) => Side::Cpu,
                Some(Side::Cpu) if g - c < SWITCH_MARGIN_C => Side::Cpu,
                Some(Side::Cpu) => Side::Gpu,
                None if g > c => Side::Gpu,
                None => Side::Cpu,
            },
            (Some(_), None) => Side::Cpu,
            (None, Some(_)) => Side::Gpu,
            (None, None) => return None,
        };
        Some(match side {
            Side::Cpu => MaxPick { value: cpu?, side, sensor: auto_temp_sensor(r), gpu },
            Side::Gpu => {
                let g = gpu?;
                MaxPick { value: g.value, side, sensor: Some(g), gpu }
            }
        })
    }
}

/// Temperature (°C) to send for the given choice.
pub fn select_temp(src: &TempSource, r: &Reading, max: &MaxTracker) -> Option<f32> {
    match src {
        TempSource::Auto => r.cpu_temp_c,
        TempSource::MaxCpuGpu => max.pick(r).map(|p| p.value),
        TempSource::Sensor(id) => match find(r, id, SensorKind::Temp) {
            Some(s) => Some(s.value),
            None => r.cpu_temp_c,
        },
    }
}

/// Fan speed (RPM) to send for the given choice.
pub fn select_fan(src: &FanSource, r: &Reading) -> Option<u32> {
    match src {
        FanSource::Auto => r.fan_rpm,
        FanSource::Sensor(id) => match find(r, id, SensorKind::Fan) {
            Some(s) => Some(s.value.round().max(0.0) as u32),
            None => r.fan_rpm,
        },
    }
}

/// One entry of a source selector.
///
/// `primary`/`secondary` are the two display lines, `group` is the category
/// header the entry belongs to (empty for the entries at the top). The live
/// value is kept apart so the UI can refresh it in place; see [`same_layout`].
#[derive(Debug, Clone, PartialEq)]
pub struct SourceOption<S> {
    pub source: S,
    pub primary: String,
    pub secondary: String,
    pub group: String,
    pub value: Option<f32>,
    /// `value` formatted with its unit (empty when unknown).
    pub value_text: String,
}

/// Whether two option lists look the same apart from the live values, i.e. a
/// UI model can be updated row by row instead of being rebuilt.
pub fn same_layout<S: PartialEq>(a: &[SourceOption<S>], b: &[SourceOption<S>]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.source == y.source
                && x.primary == y.primary
                && x.secondary == y.secondary
                && x.group == y.group
        })
}

fn temp_text(v: Option<f32>) -> String {
    v.map(|v| format!("{v:.0} °C")).unwrap_or_default()
}

fn fan_text(v: Option<f32>) -> String {
    v.map(|v| format!("{} RPM", v.round().max(0.0) as u32)).unwrap_or_default()
}

fn top_option<S>(
    source: S,
    primary: &str,
    secondary: String,
    value: Option<f32>,
    text: fn(Option<f32>) -> String,
) -> SourceOption<S> {
    SourceOption {
        source,
        primary: primary.into(),
        secondary,
        group: String::new(),
        value,
        value_text: text(value),
    }
}

/// `"<category> · <sensor>"`: what an automatic choice currently resolves to.
fn describe(s: &SensorValue) -> String {
    format!("{} · {}", classify(s).title(), friendly_name(s))
}

/// The sensor behind the automatic CPU temperature: the one the backend named
/// by id. Never inferred from values, which several sensors share.
fn auto_temp_sensor(r: &Reading) -> Option<&SensorValue> {
    find(r, r.cpu_temp_id.as_deref()?, SensorKind::Temp)
}

/// The sensor behind the automatic fan/pump RPM, by the id the backend named.
fn auto_fan_sensor(r: &Reading) -> Option<&SensorValue> {
    find(r, r.fan_id.as_deref()?, SensorKind::Fan)
}

fn auto_temp_option(r: &Reading) -> SourceOption<TempSource> {
    let secondary = auto_temp_sensor(r).map(describe).unwrap_or_else(|| "CPU temperature".into());
    top_option(TempSource::Auto, "Auto", secondary, r.cpu_temp_c, temp_text)
}

fn max_option(r: &Reading, max: &MaxTracker) -> SourceOption<TempSource> {
    let (secondary, value) = match max.pick(r) {
        Some(p) => (p.sensor.map(describe).unwrap_or_else(|| "CPU temperature".into()), Some(p.value)),
        None => ("Hottest of CPU and GPU".into(), None),
    };
    top_option(TempSource::MaxCpuGpu, "Max CPU/GPU", secondary, value, temp_text)
}

fn sensor_options<S>(
    sensors: &[SensorValue],
    kind: SensorKind,
    wrap: impl Fn(String) -> S,
    selected: Option<&str>,
    text: fn(Option<f32>) -> String,
    out: &mut Vec<SourceOption<S>>,
) {
    // A saved choice stays listed even if it would be filtered out now.
    let mut found: Vec<(Category, u8, &SensorValue)> = sensors
        .iter()
        .filter(|s| s.kind == kind && (is_reading(s) || selected == Some(s.id.as_str())))
        .map(|s| {
            let cat = classify(s);
            (cat, if cat == Category::Cpu { cpu_rank(&s.name) } else { 0 }, s)
        })
        .collect();
    found.sort_by_key(|(cat, rank, _)| (*cat, *rank)); // stable: keeps backend order
    let seen_selected = found.iter().any(|(_, _, s)| selected == Some(s.id.as_str()));
    for (cat, _, s) in found {
        out.push(SourceOption {
            source: wrap(s.id.clone()),
            primary: friendly_name(s),
            secondary: format!("{} · {}", cat.title(), short_hardware(s, cat)),
            group: cat.title().into(),
            value: Some(s.value),
            value_text: text(Some(s.value)),
        });
    }
    // A saved choice whose sensor is not (yet) present stays visible, so the
    // selection is neither lost nor silently shown as something else.
    if let Some(id) = selected {
        if !seen_selected {
            out.push(SourceOption {
                source: wrap(id.to_string()),
                primary: id.to_string(),
                secondary: "Unavailable, using Auto".into(),
                group: "Unavailable".into(),
                value: None,
                value_text: String::new(),
            });
        }
    }
}

/// Options of the temperature selector: Auto, Max CPU/GPU, then every
/// temperature reading grouped by category.
pub fn temp_options(
    r: &Reading,
    current: &TempSource,
    max: &MaxTracker,
) -> Vec<SourceOption<TempSource>> {
    let mut out = vec![auto_temp_option(r), max_option(r, max)];
    let sel = match current {
        TempSource::Sensor(id) => Some(id.as_str()),
        _ => None,
    };
    sensor_options(&r.sensors, SensorKind::Temp, TempSource::Sensor, sel, temp_text, &mut out);
    out
}

/// Options of the RPM selector: Auto, then every fan/pump reading grouped by category.
pub fn fan_options(r: &Reading, current: &FanSource) -> Vec<SourceOption<FanSource>> {
    let secondary =
        auto_fan_sensor(r).map(describe).unwrap_or_else(|| "CPU fan".into());
    let mut out = vec![top_option(
        FanSource::Auto,
        "Auto",
        secondary,
        r.fan_rpm.map(|v| v as f32),
        fan_text,
    )];
    let sel = match current {
        FanSource::Sensor(id) => Some(id.as_str()),
        FanSource::Auto => None,
    };
    sensor_options(&r.sensors, SensorKind::Fan, FanSource::Sensor, sel, fan_text, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sv(id: &str, hw: &str, hw_type: &str, name: &str, kind: SensorKind, value: f32) -> SensorValue {
        SensorValue {
            id: id.into(),
            hardware: hw.into(),
            hardware_type: hw_type.into(),
            name: name.into(),
            kind,
            value,
        }
    }

    fn reading() -> Reading {
        Reading {
            cpu_temp_c: Some(55.0),
            fan_rpm: Some(1200),
            cpu_temp_id: Some("/cpu/0/temp/0".into()),
            fan_id: Some("/lpc/0/fan/1".into()),
            sensors: vec![
                sv("/cpu/0/temp/0", "Ryzen", "Cpu", "Core (Tctl/Tdie)", SensorKind::Temp, 55.0),
                sv("/gpu/0/temp/0", "RTX", "GpuNvidia", "GPU Core", SensorKind::Temp, 70.0),
                sv("/gpu/0/temp/1", "RTX", "GpuNvidia", "GPU Hot Spot", SensorKind::Temp, 81.0),
                sv("/gpu/0/temp/2", "RTX", "GpuNvidia", "GPU Memory Junction", SensorKind::Temp, 95.0),
                sv("/lpc/0/fan/1", "NCT", "SuperIO", "Fan #2", SensorKind::Fan, 1200.4),
                sv("/lpc/0/fan/2", "NCT", "SuperIO", "Pump", SensorKind::Fan, 2600.0),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn auto_uses_legacy_values() {
        let r = reading();
        assert_eq!(select_temp(&TempSource::Auto, &r, &MaxTracker::default()), Some(55.0));
        assert_eq!(select_fan(&FanSource::Auto, &r), Some(1200));
    }

    #[test]
    fn specific_sensor_is_used() {
        let r = reading();
        assert_eq!(select_temp(&TempSource::Sensor("/gpu/0/temp/0".into()), &r, &MaxTracker::default()), Some(70.0));
        assert_eq!(select_fan(&FanSource::Sensor("/lpc/0/fan/2".into()), &r), Some(2600));
        assert_eq!(select_fan(&FanSource::Sensor("/lpc/0/fan/1".into()), &r), Some(1200));
    }

    #[test]
    fn missing_sensor_falls_back_to_auto() {
        let r = reading();
        assert_eq!(select_temp(&TempSource::Sensor("gone".into()), &r, &MaxTracker::default()), Some(55.0));
        assert_eq!(select_fan(&FanSource::Sensor("gone".into()), &r), Some(1200));
        // Wrong kind for the id is also "missing".
        assert_eq!(select_fan(&FanSource::Sensor("/gpu/0/temp/0".into()), &r), Some(1200));
        let empty = Reading { cpu_temp_c: None, fan_rpm: None, ..Default::default() };
        assert_eq!(select_temp(&TempSource::Sensor("gone".into()), &empty, &MaxTracker::default()), None);
    }

    #[test]
    fn max_cpu_gpu_picks_hottest_core_or_hotspot() {
        let mut r = reading();
        // Hot spot (81) beats CPU (55); memory junction (95) is ignored.
        assert_eq!(select_temp(&TempSource::MaxCpuGpu, &r, &MaxTracker::default()), Some(81.0));
        r.cpu_temp_c = Some(90.0);
        assert_eq!(select_temp(&TempSource::MaxCpuGpu, &r, &MaxTracker::default()), Some(90.0));
    }

    #[test]
    fn max_cpu_gpu_degrades_gracefully() {
        let mut r = reading();
        r.sensors.retain(|s| !s.hardware_type.starts_with("Gpu"));
        assert_eq!(select_temp(&TempSource::MaxCpuGpu, &r, &MaxTracker::default()), Some(55.0)); // no GPU
        let mut r = reading();
        r.cpu_temp_c = None;
        assert_eq!(select_temp(&TempSource::MaxCpuGpu, &r, &MaxTracker::default()), Some(81.0)); // no CPU
        r.sensors.clear();
        assert_eq!(select_temp(&TempSource::MaxCpuGpu, &r, &MaxTracker::default()), None);
    }

    #[test]
    fn options_lists() {
        let r = reading();
        let t = temp_options(&r, &TempSource::Auto, &MaxTracker::default());
        assert_eq!(t[0].source, TempSource::Auto);
        assert_eq!(t[1].source, TempSource::MaxCpuGpu);
        assert_eq!(t.len(), 2 + 4);
        // CPU first, then GPU.
        assert_eq!(t[2].primary, "Core (Tctl/Tdie)");
        assert_eq!(t[2].secondary, "CPU · Ryzen");
        assert_eq!(t[2].group, "CPU");
        assert_eq!(t[3].group, "GPU");
        assert_eq!(t[2].value_text, "55 °C");
        let f = fan_options(&r, &FanSource::Auto);
        assert_eq!(f.len(), 1 + 2);
        assert_eq!(f[0].source, FanSource::Auto);
        // The pump is classified as Liquid / Pump and listed before the board fan.
        assert_eq!(f[1].primary, "Pump");
        assert_eq!(f[1].group, "Liquid / Pump");
        assert_eq!(f[1].value_text, "2600 RPM");
    }

    #[test]
    fn auto_and_max_show_what_they_resolve_to() {
        let r = reading();
        let t = temp_options(&r, &TempSource::Auto, &MaxTracker::default());
        assert_eq!(t[0].secondary, "CPU · Core (Tctl/Tdie)");
        assert_eq!(t[0].value_text, "55 °C");
        assert_eq!(t[1].secondary, "GPU · GPU Hot Spot"); // 81 beats the CPU's 55
        assert_eq!(t[1].value_text, "81 °C");
        let f = fan_options(&r, &FanSource::Auto);
        assert_eq!(f[0].secondary, "Motherboard · Fan #2");
        assert_eq!(f[0].value_text, "1200 RPM");
    }

    /// The list of the real machine this was designed for.
    #[test]
    fn real_machine_list() {
        let dimm3 = "Corsair - CMK32GX5M2\u{FFFD}?\u{FFFD}?r \u{FFFD}\u{FFFD}A\u{FFFD}A\u{FFFD}} (#3)";
        let t = |id: &str, hw: &str, ty: &str, n: &str, v: f32| sv(id, hw, ty, n, SensorKind::Temp, v);
        let nct = "Nuvoton NCT6799D";
        let ddr = "Corsair - CMK32GX5M2B6000C38 (#1)";
        let r = Reading {
            cpu_temp_c: Some(47.0),
            cpu_temp_id: Some("/amdcpu/0/t/1".into()),
            sensors: vec![
                t("/lpc/0/t/0", nct, "SuperIO", "CPU Core", 40.0),
                t("/lpc/0/t/1", nct, "SuperIO", "Temperature #1", 35.0),
                t("/amdcpu/0/t/0", "AMD Ryzen 7 9800X3D", "Cpu", "CCD1 (Tdie)", 44.0),
                t("/amdcpu/0/t/1", "AMD Ryzen 7 9800X3D", "Cpu", "Core (Tctl/Tdie)", 47.0),
                t("/ram/0/t/0", ddr, "Memory", "DIMM #1", 33.0),
                t("/ram/0/t/1", ddr, "Memory", "Thermal Sensor High Limit", 55.0),
                t("/ram/0/t/2", ddr, "Memory", "Thermal Sensor Critical Limit", 85.0),
                t("/ram/2/t/0", dimm3, "Memory", "DIMM #3", 34.0),
                t("/gpu/0/t/0", "AMD Radeon(TM) Graphics", "GpuAmd", "GPU VR SoC", 38.0),
                t("/nvme/0/t/0", "X16 SSD 2TB", "Storage", "Composite Temperature", 41.0),
                t("/nvme/0/t/1", "X16 SSD 2TB", "Storage", "Warning Temperature", 83.0),
                t("/nvme/0/t/2", "X16 SSD 2TB", "Storage", "Critical Temperature", 87.0),
            ],
            ..Default::default()
        };
        let o = temp_options(&r, &TempSource::Auto, &MaxTracker::default());
        let rows: Vec<String> = o
            .iter()
            .map(|x| format!("{} | {} | {} | {}", x.group, x.primary, x.secondary, x.value_text))
            .collect();
        assert_eq!(
            rows,
            [
                " | Auto | CPU · Core (Tctl/Tdie) | 47 °C",
                " | Max CPU/GPU | CPU · Core (Tctl/Tdie) | 47 °C",
                "CPU | Core (Tctl/Tdie) | CPU · AMD Ryzen 7 9800X3D | 47 °C",
                "CPU | CCD1 (Tdie) | CPU · AMD Ryzen 7 9800X3D | 44 °C",
                "GPU | GPU VR SoC | GPU · AMD Radeon Graphics | 38 °C",
                "Motherboard | CPU Core | Motherboard · Nuvoton NCT6799D | 40 °C",
                "Motherboard | Temperature #1 | Motherboard · Nuvoton NCT6799D | 35 °C",
                "Memory | DIMM #1 | Memory · DIMM slot 1 | 33 °C",
                "Memory | DIMM #3 | Memory · DIMM slot 3 | 34 °C",
                "Storage | Composite Temperature | Storage · X16 SSD 2TB | 41 °C",
            ]
        );
    }

    #[test]
    fn saved_filtered_sensor_stays_listed_by_name() {
        let mut r = reading();
        r.sensors.push(sv("/nvme/0/t/1", "SSD", "Storage", "Warning Temperature", SensorKind::Temp, 83.0));
        let sel = TempSource::Sensor("/nvme/0/t/1".into());
        assert_eq!(temp_options(&r, &TempSource::Auto, &MaxTracker::default()).len(), 2 + 4);
        let o = temp_options(&r, &sel, &MaxTracker::default());
        assert_eq!(o.len(), 2 + 5);
        assert_eq!(o.last().unwrap().primary, "Warning Temperature");
    }

    #[test]
    fn same_layout_ignores_values_only() {
        let mut r = reading();
        let a = temp_options(&r, &TempSource::Auto, &MaxTracker::default());
        r.sensors[0].value += 1.0;
        r.cpu_temp_c = Some(56.0);
        let b = temp_options(&r, &TempSource::Auto, &MaxTracker::default());
        assert!(same_layout(&a, &b));
        assert_ne!(a, b);
        r.sensors.remove(1);
        assert!(!same_layout(&a, &temp_options(&r, &TempSource::Auto, &MaxTracker::default())));
    }

    #[test]
    fn options_keep_unavailable_selection() {
        let none = Reading::default();
        let f = fan_options(&none, &FanSource::Sensor("/lpc/0/fan/9".into()));
        assert_eq!(f.len(), 2);
        assert_eq!(f[1].source, FanSource::Sensor("/lpc/0/fan/9".into()));
        // Present selection is not duplicated.
        let r = reading();
        let f = fan_options(&r, &FanSource::Sensor("/lpc/0/fan/2".into()));
        assert_eq!(f.len(), 3);
    }

    /// Two sensors with the very same value: Auto stays on the one the id names.
    #[test]
    fn auto_label_follows_the_id_not_the_value() {
        let mk = |board_first: bool, jitter: f32| {
            let mut sensors = vec![
                sv("/lpc/0/t/0", "Nuvoton NCT6799D", "SuperIO", "CPU Core", SensorKind::Temp, 47.0 + jitter),
                sv("/amdcpu/0/t/1", "AMD Ryzen 7 9800X3D", "Cpu", "Core (Tctl/Tdie)", SensorKind::Temp, 47.0),
                sv("/gpu/0/t/0", "AMD Radeon(TM) Graphics", "GpuAmd", "GPU Core", SensorKind::Temp, 47.0),
                sv("/lpc/0/fan/1", "Nuvoton NCT6799D", "SuperIO", "Fan #2", SensorKind::Fan, 1200.0),
                sv("/lpc/0/fan/2", "Nuvoton NCT6799D", "SuperIO", "Fan #3", SensorKind::Fan, 1200.0),
            ];
            if board_first {
                sensors.reverse();
            }
            Reading {
                cpu_temp_c: Some(47.0),
                fan_rpm: Some(1200),
                cpu_temp_id: Some("/amdcpu/0/t/1".into()),
                fan_id: Some("/lpc/0/fan/2".into()),
                sensors,
                ..Default::default()
            }
        };
        let max = MaxTracker::default();
        for i in 0..20 {
            let r = mk(i % 2 == 0, if i % 3 == 0 { 0.0 } else { 0.02 });
            let t = temp_options(&r, &TempSource::Auto, &max);
            assert_eq!(t[0].secondary, "CPU · Core (Tctl/Tdie)");
            let f = fan_options(&r, &FanSource::Auto);
            assert_eq!(f[0].secondary, "Motherboard · Fan #3");
        }
    }

    #[test]
    fn auto_without_an_id_is_generic() {
        let mut r = reading();
        r.cpu_temp_id = None;
        r.fan_id = None;
        let t = temp_options(&r, &TempSource::Auto, &MaxTracker::default());
        assert_eq!(t[0].secondary, "CPU temperature");
        assert_eq!(t[0].value_text, "55 °C");
        assert_eq!(fan_options(&r, &FanSource::Auto)[0].secondary, "CPU fan");
        // An id missing from the list (or of the wrong kind) is generic too.
        r.cpu_temp_id = Some("gone".into());
        r.fan_id = Some("/gpu/0/temp/0".into());
        assert_eq!(temp_options(&r, &TempSource::Auto, &MaxTracker::default())[0].secondary, "CPU temperature");
        assert_eq!(fan_options(&r, &FanSource::Auto)[0].secondary, "CPU fan");
    }

    fn max_label(r: &Reading, m: &MaxTracker) -> (String, Option<f32>) {
        let o = &temp_options(r, &TempSource::Auto, m)[1];
        (o.secondary.clone(), o.value)
    }

    /// CPU "Core (Tctl/Tdie)" against the GPU hot spot, at the given temperatures.
    fn cpu_gpu(cpu: f32, gpu: f32) -> Reading {
        let mut r = reading();
        r.sensors.retain(|s| s.id != "/gpu/0/temp/0" && s.id != "/gpu/0/temp/2");
        r.sensors[0].value = cpu;
        r.sensors[1].value = gpu;
        r.cpu_temp_c = Some(cpu);
        r
    }

    #[test]
    fn max_cpu_gpu_has_hysteresis() {
        const CPU: &str = "CPU · Core (Tctl/Tdie)";
        const GPU: &str = "GPU · GPU Hot Spot";
        let mut m = MaxTracker::default();
        // First decision: the plain maximum.
        let r = cpu_gpu(60.0, 70.0);
        m.update(&r);
        assert_eq!(max_label(&r, &m), (GPU.into(), Some(70.0)));
        // The CPU is hotter by less than 1 °C: stays on the GPU, however often.
        for cpu in [70.4, 70.9, 70.2, 70.9] {
            let r = cpu_gpu(cpu, 70.0);
            m.update(&r);
            assert_eq!(max_label(&r, &m).0, GPU);
            assert_eq!(select_temp(&TempSource::MaxCpuGpu, &r, &m), Some(70.0));
        }
        // Hotter by 1 °C or more: switches to the CPU.
        let r = cpu_gpu(71.0, 70.0);
        m.update(&r);
        assert_eq!(max_label(&r, &m), (CPU.into(), Some(71.0)));
        // And back the same way.
        for gpu in [71.5, 71.9, 71.0] {
            let r = cpu_gpu(71.0, gpu);
            m.update(&r);
            assert_eq!(max_label(&r, &m).0, CPU);
        }
        let r = cpu_gpu(71.0, 72.0);
        m.update(&r);
        assert_eq!(max_label(&r, &m), (GPU.into(), Some(72.0)));
        // Reading again without an update does not change the answer.
        assert_eq!(max_label(&r, &m), (GPU.into(), Some(72.0)));
    }

    #[test]
    fn max_cpu_gpu_label_follows_the_cpu_id() {
        let mut r = cpu_gpu(60.0, 50.0);
        let mut m = MaxTracker::default();
        m.update(&r);
        assert_eq!(max_label(&r, &m).0, "CPU · Core (Tctl/Tdie)");
        r.cpu_temp_id = None;
        assert_eq!(max_label(&r, &m).0, "CPU temperature");
        // No readings at all.
        let none = Reading::default();
        m.update(&none);
        assert_eq!(max_label(&none, &m), ("Hottest of CPU and GPU".into(), None));
    }

    #[test]
    fn max_cpu_gpu_keeps_its_gpu_sensor_within_the_margin() {
        let mk = |core: f32, spot: f32| {
            let mut r = cpu_gpu(40.0, spot);
            r.sensors.push(sv("/gpu/0/temp/0", "RTX", "GpuNvidia", "GPU Core", SensorKind::Temp, core));
            r
        };
        let mut m = MaxTracker::default();
        let r = mk(60.0, 61.0);
        m.update(&r);
        assert_eq!(max_label(&r, &m).0, "GPU · GPU Hot Spot");
        // The core gets 0.5 °C ahead: no switch. 1 °C ahead: switch.
        let r = mk(61.5, 61.0);
        m.update(&r);
        assert_eq!(max_label(&r, &m).0, "GPU · GPU Hot Spot");
        let r = mk(62.0, 61.0);
        m.update(&r);
        assert_eq!(max_label(&r, &m).0, "GPU · GPU Core");
    }
}
