//! Choice of which sensor feeds each of the display's two fields.
//!
//! The frame carries exactly two values (temperature byte + fan RPM `u16`).
//! This module holds the **pure** selection logic: given a [`Reading`] and the
//! user's choice, it returns the value to send. A choice that cannot be
//! satisfied (the sensor vanished) falls back to `Auto` for that field instead
//! of producing a bogus 0.

use serde::{Deserialize, Serialize};

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

/// Hottest GPU core/hot-spot temperature, if any GPU sensor is present.
fn hottest_gpu(r: &Reading) -> Option<f32> {
    r.sensors
        .iter()
        .filter(|s| is_gpu_core_temp(s))
        .map(|s| s.value)
        .fold(None, |acc: Option<f32>, v| Some(acc.map_or(v, |a| a.max(v))))
}

/// Temperature (°C) to send for the given choice.
pub fn select_temp(src: &TempSource, r: &Reading) -> Option<f32> {
    match src {
        TempSource::Auto => r.cpu_temp_c,
        TempSource::MaxCpuGpu => match (r.cpu_temp_c, hottest_gpu(r)) {
            (Some(c), Some(g)) => Some(c.max(g)),
            (c, g) => c.or(g),
        },
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

/// One entry of a source selector: the choice and its label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceOption<S> {
    pub source: S,
    pub label: String,
}

fn sensor_options<S>(
    sensors: &[SensorValue],
    kind: SensorKind,
    wrap: impl Fn(String) -> S,
    selected: Option<&str>,
    out: &mut Vec<SourceOption<S>>,
) {
    let mut seen_selected = false;
    for s in sensors.iter().filter(|s| s.kind == kind) {
        seen_selected |= selected == Some(s.id.as_str());
        out.push(SourceOption { source: wrap(s.id.clone()), label: s.label() });
    }
    // A saved choice whose sensor is not (yet) present stays visible, so the
    // selection is neither lost nor silently shown as something else.
    if let Some(id) = selected {
        if !seen_selected {
            out.push(SourceOption {
                source: wrap(id.to_string()),
                label: format!("{id} (unavailable, using Auto)"),
            });
        }
    }
}

/// Options of the temperature selector: Auto, Max CPU/GPU, then every temperature sensor.
pub fn temp_options(sensors: &[SensorValue], current: &TempSource) -> Vec<SourceOption<TempSource>> {
    let mut out = vec![
        SourceOption { source: TempSource::Auto, label: "Auto".into() },
        SourceOption { source: TempSource::MaxCpuGpu, label: "Max CPU/GPU".into() },
    ];
    let sel = match current {
        TempSource::Sensor(id) => Some(id.as_str()),
        _ => None,
    };
    sensor_options(sensors, SensorKind::Temp, TempSource::Sensor, sel, &mut out);
    out
}

/// Options of the RPM selector: Auto, then every fan/pump sensor.
pub fn fan_options(sensors: &[SensorValue], current: &FanSource) -> Vec<SourceOption<FanSource>> {
    let mut out = vec![SourceOption { source: FanSource::Auto, label: "Auto".into() }];
    let sel = match current {
        FanSource::Sensor(id) => Some(id.as_str()),
        FanSource::Auto => None,
    };
    sensor_options(sensors, SensorKind::Fan, FanSource::Sensor, sel, &mut out);
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
        assert_eq!(select_temp(&TempSource::Auto, &r), Some(55.0));
        assert_eq!(select_fan(&FanSource::Auto, &r), Some(1200));
    }

    #[test]
    fn specific_sensor_is_used() {
        let r = reading();
        assert_eq!(select_temp(&TempSource::Sensor("/gpu/0/temp/0".into()), &r), Some(70.0));
        assert_eq!(select_fan(&FanSource::Sensor("/lpc/0/fan/2".into()), &r), Some(2600));
        assert_eq!(select_fan(&FanSource::Sensor("/lpc/0/fan/1".into()), &r), Some(1200));
    }

    #[test]
    fn missing_sensor_falls_back_to_auto() {
        let r = reading();
        assert_eq!(select_temp(&TempSource::Sensor("gone".into()), &r), Some(55.0));
        assert_eq!(select_fan(&FanSource::Sensor("gone".into()), &r), Some(1200));
        // Wrong kind for the id is also "missing".
        assert_eq!(select_fan(&FanSource::Sensor("/gpu/0/temp/0".into()), &r), Some(1200));
        let empty = Reading { cpu_temp_c: None, fan_rpm: None, ..Default::default() };
        assert_eq!(select_temp(&TempSource::Sensor("gone".into()), &empty), None);
    }

    #[test]
    fn max_cpu_gpu_picks_hottest_core_or_hotspot() {
        let mut r = reading();
        // Hot spot (81) beats CPU (55); memory junction (95) is ignored.
        assert_eq!(select_temp(&TempSource::MaxCpuGpu, &r), Some(81.0));
        r.cpu_temp_c = Some(90.0);
        assert_eq!(select_temp(&TempSource::MaxCpuGpu, &r), Some(90.0));
    }

    #[test]
    fn max_cpu_gpu_degrades_gracefully() {
        let mut r = reading();
        r.sensors.retain(|s| !s.hardware_type.starts_with("Gpu"));
        assert_eq!(select_temp(&TempSource::MaxCpuGpu, &r), Some(55.0)); // no GPU
        let mut r = reading();
        r.cpu_temp_c = None;
        assert_eq!(select_temp(&TempSource::MaxCpuGpu, &r), Some(81.0)); // no CPU
        r.sensors.clear();
        assert_eq!(select_temp(&TempSource::MaxCpuGpu, &r), None);
    }

    #[test]
    fn options_lists() {
        let r = reading();
        let t = temp_options(&r.sensors, &TempSource::Auto);
        assert_eq!(t[0].source, TempSource::Auto);
        assert_eq!(t[1].source, TempSource::MaxCpuGpu);
        assert_eq!(t.len(), 2 + 4);
        assert_eq!(t[2].label, "Ryzen — Core (Tctl/Tdie)");
        let f = fan_options(&r.sensors, &FanSource::Auto);
        assert_eq!(f.len(), 1 + 2);
        assert_eq!(f[0].source, FanSource::Auto);
    }

    #[test]
    fn options_keep_unavailable_selection() {
        let f = fan_options(&[], &FanSource::Sensor("/lpc/0/fan/9".into()));
        assert_eq!(f.len(), 2);
        assert_eq!(f[1].source, FanSource::Sensor("/lpc/0/fan/9".into()));
        // Present selection is not duplicated.
        let r = reading();
        let f = fan_options(&r.sensors, &FanSource::Sensor("/lpc/0/fan/2".into()));
        assert_eq!(f.len(), 3);
    }
}
