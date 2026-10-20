//! Light/full mode of the Windows sensor sidecar (`kalkan-sensord`).
//!
//! While the window is hidden nobody looks at the sensor lists, so the sidecar
//! only has to poll the hardware groups the display's two fields come from
//! ("light"); disks (SMART), the GPU and the DIMMs are then left alone. With
//! the window visible it polls everything ("full") so the selectors are
//! complete. This module holds the pure part: which groups a selection needs,
//! the stdin commands, and how a light sensor list is merged with the last
//! full one. Platform-independent so it is tested everywhere.

use std::collections::{BTreeSet, HashMap};

use crate::selection::{FanSource, TempSource};
use crate::sensors::SensorValue;

/// Hardware groups of the sidecar (LibreHardwareMonitor `Is…Enabled` flags).
pub(crate) type Groups = BTreeSet<&'static str>;

/// Groups light mode always polls: the automatic CPU temperature and fan
/// picks come from them.
pub(crate) const BASE_GROUPS: [&str; 3] = ["board", "controller", "cpu"];

/// The sidecar group a LibreHardwareMonitor `HardwareType` belongs to.
pub(crate) fn hardware_group(hardware_type: &str) -> Option<&'static str> {
    match hardware_type.to_ascii_lowercase().as_str() {
        "cpu" => Some("cpu"),
        "motherboard" | "superio" | "embeddedcontroller" => Some("board"),
        "cooler" => Some("controller"),
        "memory" => Some("memory"),
        "storage" => Some("storage"),
        t if t.starts_with("gpu") => Some("gpu"),
        _ => None,
    }
}

/// The stdin commands for a mode: `None` is full, `Some(groups)` is light.
pub(crate) fn command(light: Option<&Groups>) -> String {
    match light {
        None => "mode full\n".into(),
        Some(g) => {
            let list: Vec<&str> = g.iter().copied().collect();
            format!("need {}\nmode light\n", list.join(","))
        }
    }
}

/// Whether a sidecar line was produced in light mode.
pub(crate) fn is_light_line(obj: &serde_json::Map<String, serde_json::Value>) -> bool {
    obj.get("mode").and_then(|m| m.as_str()) == Some("light")
}

/// What the Rust side knows about the sidecar's sensors and mode.
#[derive(Debug, Default)]
pub(crate) struct Planner {
    /// The last full sensor list.
    full: Vec<SensorValue>,
    /// Group of every sensor id ever seen, so a selected id keeps its group
    /// while it is absent from light lines.
    groups: HashMap<String, &'static str>,
    /// A full line has been seen (before that, light mode is not entered:
    /// the group of a selected sensor would be unknown).
    seen_full: bool,
    /// Ids the automatic picks resolved to in the last full line.
    auto_temp: Option<String>,
    auto_fan: Option<String>,
    /// Mode last requested: `None` = full (also the sidecar's default).
    sent: Option<Groups>,
}

impl Planner {
    /// Records a full line.
    pub fn learn_full(&mut self, sensors: &[SensorValue], temp_id: Option<&str>, fan_id: Option<&str>) {
        for s in sensors {
            if let Some(g) = hardware_group(&s.hardware_type) {
                self.groups.insert(s.id.clone(), g);
            }
        }
        self.full = sensors.to_vec();
        self.auto_temp = temp_id.map(str::to_string);
        self.auto_fan = fan_id.map(str::to_string);
        self.seen_full = true;
    }

    /// Groups light mode must poll for this selection; `None` means stay in
    /// full mode (no full list seen yet).
    pub fn needed(&self, temp: &TempSource, fan: &FanSource) -> Option<Groups> {
        if !self.seen_full {
            return None;
        }
        let mut g: Groups = BASE_GROUPS.into_iter().collect();
        if *temp == TempSource::MaxCpuGpu {
            g.insert("gpu");
        }
        // Auto (and the fallback of a missing sensor) uses the automatic
        // picks, which may live outside the base groups (e.g. a GPU
        // temperature when the CPU reports none).
        let mut ids = vec![self.auto_temp.as_deref(), self.auto_fan.as_deref()];
        if let TempSource::Sensor(id) = temp {
            ids.push(Some(id));
        }
        if let FanSource::Sensor(id) = fan {
            ids.push(Some(id));
        }
        // An id never seen in a full list is a sensor that is gone: its field
        // falls back to Auto, which the automatic picks above cover.
        g.extend(ids.into_iter().flatten().filter_map(|id| self.groups.get(id).copied()));
        Some(g)
    }

    /// Commands to send so the sidecar is in the wanted mode, or `None` when
    /// it already is (as far as requested).
    pub fn update(&mut self, light: bool, temp: &TempSource, fan: &FanSource) -> Option<String> {
        let want = if light { self.needed(temp, fan) } else { None };
        if want == self.sent {
            return None;
        }
        let cmd = command(want.as_ref());
        self.sent = want;
        Some(cmd)
    }

    /// The sensor list to show for a light line: the last full list, with the
    /// polled groups' entries taken from the light line (dropped when absent
    /// there) and the others kept at their last value. So the selectors keep
    /// their layout across a hide/show, and nothing displayed comes from a
    /// group that is not being polled (the selection's groups always are).
    pub fn merge(&self, light: &[SensorValue]) -> Vec<SensorValue> {
        let mut polled: Groups = self.sent.clone().unwrap_or_default();
        polled.extend(light.iter().filter_map(|s| hardware_group(&s.hardware_type)));
        let mut out: Vec<SensorValue> = Vec::with_capacity(self.full.len().max(light.len()));
        for s in &self.full {
            match hardware_group(&s.hardware_type) {
                Some(g) if polled.contains(g) => {
                    if let Some(fresh) = light.iter().find(|l| l.id == s.id && l.kind == s.kind) {
                        out.push(fresh.clone());
                    }
                }
                _ => out.push(s.clone()),
            }
        }
        for l in light {
            if !self.full.iter().any(|s| s.id == l.id && s.kind == l.kind) {
                out.push(l.clone());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::{parse_sensor_line, SensorKind};

    fn sv(id: &str, ty: &str, kind: SensorKind, value: f32) -> SensorValue {
        SensorValue {
            id: id.into(),
            hardware: "H".into(),
            hardware_type: ty.into(),
            name: id.into(),
            kind,
            value,
        }
    }

    fn full_list() -> Vec<SensorValue> {
        vec![
            sv("/lpc/0/fan/1", "SuperIO", SensorKind::Fan, 1200.0),
            sv("/amdcpu/0/t/1", "Cpu", SensorKind::Temp, 47.0),
            sv("/ram/0/t/0", "Memory", SensorKind::Temp, 33.0),
            sv("/gpu-nvidia/0/t/0", "GpuNvidia", SensorKind::Temp, 60.0),
            sv("/nvme/0/t/0", "Storage", SensorKind::Temp, 41.0),
            sv("/aqua/0/fan/0", "Cooler", SensorKind::Fan, 2000.0),
        ]
    }

    fn planner() -> Planner {
        let mut p = Planner::default();
        p.learn_full(&full_list(), Some("/amdcpu/0/t/1"), Some("/lpc/0/fan/1"));
        p
    }

    fn set(g: &[&'static str]) -> Groups {
        g.iter().copied().collect()
    }

    #[test]
    fn groups_of_hardware_types() {
        assert_eq!(hardware_group("Cpu"), Some("cpu"));
        assert_eq!(hardware_group("SuperIO"), Some("board"));
        assert_eq!(hardware_group("Motherboard"), Some("board"));
        assert_eq!(hardware_group("EmbeddedController"), Some("board"));
        assert_eq!(hardware_group("Cooler"), Some("controller"));
        assert_eq!(hardware_group("GpuAmd"), Some("gpu"));
        assert_eq!(hardware_group("GpuIntel"), Some("gpu"));
        assert_eq!(hardware_group("Storage"), Some("storage"));
        assert_eq!(hardware_group("Memory"), Some("memory"));
        assert_eq!(hardware_group("Network"), None);
        assert_eq!(hardware_group(""), None);
    }

    #[test]
    fn command_format() {
        assert_eq!(command(None), "mode full\n");
        assert_eq!(command(Some(&set(&["cpu", "board", "controller"]))), "need board,controller,cpu\nmode light\n");
        assert_eq!(
            command(Some(&set(&["storage", "cpu", "board", "gpu", "controller"]))),
            "need board,controller,cpu,gpu,storage\nmode light\n"
        );
    }

    #[test]
    fn needed_groups_follow_the_selection() {
        let p = planner();
        let base = set(&BASE_GROUPS);
        assert_eq!(p.needed(&TempSource::Auto, &FanSource::Auto), Some(base.clone()));
        assert_eq!(
            p.needed(&TempSource::MaxCpuGpu, &FanSource::Auto),
            Some(set(&["board", "controller", "cpu", "gpu"]))
        );
        assert_eq!(
            p.needed(&TempSource::Sensor("/nvme/0/t/0".into()), &FanSource::Auto),
            Some(set(&["board", "controller", "cpu", "storage"]))
        );
        assert_eq!(
            p.needed(&TempSource::Sensor("/ram/0/t/0".into()), &FanSource::Sensor("/aqua/0/fan/0".into())),
            Some(set(&["board", "controller", "cpu", "memory"]))
        );
        // A sensor that is gone falls back to Auto: nothing extra.
        assert_eq!(p.needed(&TempSource::Sensor("/gone".into()), &FanSource::Auto), Some(base));
    }

    #[test]
    fn auto_pick_outside_the_base_groups_is_polled() {
        // Unelevated: the only temperature (and so Auto) is a GPU one.
        let mut p = Planner::default();
        p.learn_full(&full_list(), Some("/gpu-nvidia/0/t/0"), None);
        assert_eq!(
            p.needed(&TempSource::Auto, &FanSource::Auto),
            Some(set(&["board", "controller", "cpu", "gpu"]))
        );
    }

    #[test]
    fn stays_full_until_a_full_list_was_seen() {
        let mut p = Planner::default();
        assert_eq!(p.needed(&TempSource::Auto, &FanSource::Auto), None);
        // Hidden at start: nothing to send, the sidecar starts in full mode.
        assert_eq!(p.update(true, &TempSource::Auto, &FanSource::Auto), None);
        p.learn_full(&full_list(), Some("/amdcpu/0/t/1"), None);
        assert_eq!(
            p.update(true, &TempSource::Auto, &FanSource::Auto).as_deref(),
            Some("need board,controller,cpu\nmode light\n")
        );
    }

    #[test]
    fn update_sends_only_changes() {
        let mut p = planner();
        let (a, f) = (TempSource::Auto, FanSource::Auto);
        assert_eq!(p.update(false, &a, &f), None); // already full
        assert!(p.update(true, &a, &f).is_some());
        assert_eq!(p.update(true, &a, &f), None);
        assert_eq!(
            p.update(true, &TempSource::MaxCpuGpu, &f).as_deref(),
            Some("need board,controller,cpu,gpu\nmode light\n")
        );
        assert_eq!(p.update(false, &a, &f).as_deref(), Some("mode full\n"));
        assert_eq!(p.update(false, &TempSource::MaxCpuGpu, &f), None);
    }

    #[test]
    fn merge_keeps_the_layout_and_never_mixes_polled_groups() {
        let mut p = planner();
        p.update(true, &TempSource::Auto, &FanSource::Auto);
        // Light line: cpu/board/controller only; the board fan vanished.
        let light = vec![
            sv("/amdcpu/0/t/1", "Cpu", SensorKind::Temp, 50.0),
            sv("/aqua/0/fan/0", "Cooler", SensorKind::Fan, 2100.0),
            sv("/amdcpu/0/t/9", "Cpu", SensorKind::Temp, 45.0), // new
        ];
        let m = p.merge(&light);
        let ids: Vec<(&str, f32)> = m.iter().map(|s| (s.id.as_str(), s.value)).collect();
        assert_eq!(
            ids,
            [
                ("/amdcpu/0/t/1", 50.0),     // fresh
                ("/ram/0/t/0", 33.0),        // not polled: last value
                ("/gpu-nvidia/0/t/0", 60.0), // not polled: last value
                ("/nvme/0/t/0", 41.0),       // not polled: last value
                ("/aqua/0/fan/0", 2100.0),   // fresh
                ("/amdcpu/0/t/9", 45.0),     // new, appended
            ]
        );
    }

    #[test]
    fn parser_accepts_light_lines() {
        let line = r#"{"cpu_temp_c":47,"cpu_temp_id":"/amdcpu/0/t/1","fan_rpm":null,"fan_id":null,"sensors":[{"id":"/amdcpu/0/t/1","hardware":"Ryzen","hardware_type":"Cpu","name":"Core (Tctl/Tdie)","kind":"temp","value":47}],"mode":"light"}"#;
        let l = parse_sensor_line(line).unwrap();
        assert!(l.light);
        assert_eq!(l.temp_c, Some(47.0));
        assert_eq!(l.sensors.len(), 1);
        let empty = parse_sensor_line(r#"{"cpu_temp_c":null,"cpu_temp_id":null,"fan_rpm":null,"fan_id":null,"sensors":[],"mode":"light"}"#).unwrap();
        assert!(empty.light && empty.sensors.is_empty());
        // Full lines carry no mode key.
        assert!(!parse_sensor_line(r#"{"cpu_temp_c":40,"fan_rpm":null,"sensors":[]}"#).unwrap().light);
    }
}
