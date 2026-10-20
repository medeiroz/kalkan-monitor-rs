//! Sensor sources.
//!
//! The original reads CPU/temperature/fan from **HWiNFO** (via `HWiNFO64.dll`),
//! which only exists on Windows. To be cross-platform we replace that source
//! with `sysinfo` (CPU load and, where available, temperatures/fans) and leave
//! the trait open for OS-specific sources (hwmon on Linux, IOKit/SMC on
//! macOS, LibreHardwareMonitor on Windows) added later.

use anyhow::Result;

use crate::selection::{FanSource, TempSource};

/// An instantaneous reading of what the display usually shows.
#[derive(Debug, Clone, Default)]
pub struct Reading {
    /// CPU usage in percent (0..=100).
    pub cpu_load: f32,
    /// RAM usage in percent (0..=100).
    pub ram_load: f32,
    /// CPU temperature in °C, when the platform exposes it.
    pub cpu_temp_c: Option<f32>,
    /// Fan/pump speed in RPM, when available.
    pub fan_rpm: Option<u32>,
    /// Id (`SensorValue::id`) of the sensor `cpu_temp_c` comes from, when the
    /// backend knows it. What "Auto" resolves to is this id, never a value match.
    pub cpu_temp_id: Option<String>,
    /// Id of the sensor `fan_rpm` comes from, when the backend knows it.
    pub fan_id: Option<String>,
    /// Every temperature and fan sensor the backend sees (for the source
    /// selection). Empty where the backend cannot enumerate them.
    pub sensors: Vec<SensorValue>,
}

/// Kind of an enumerated sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorKind {
    /// Temperature in °C.
    Temp,
    /// Fan/pump speed in RPM.
    Fan,
}

/// One temperature or fan sensor with its current value.
#[derive(Debug, Clone, PartialEq)]
pub struct SensorValue {
    /// Stable id (LibreHardwareMonitor `Identifier`, e.g. `/lpc/nct6799d/0/fan/1`).
    pub id: String,
    /// Name of the hardware that owns the sensor.
    pub hardware: String,
    /// Hardware type (LibreHardwareMonitor `HardwareType`, e.g. `Cpu`, `GpuNvidia`).
    pub hardware_type: String,
    /// Sensor name.
    pub name: String,
    pub kind: SensorKind,
    /// °C for [`SensorKind::Temp`], RPM for [`SensorKind::Fan`].
    pub value: f32,
}

impl SensorValue {
    /// Label shown in the selectors: `"<hardware> — <sensor>"`.
    pub fn label(&self) -> String {
        format!("{} — {}", self.hardware, self.name)
    }
}

/// What a reading has to contain when nobody is looking at the sensor list:
/// just what feeds the display. See [`SensorSource::set_wanted`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wanted {
    /// Ids of the temperature sensors picked by hand.
    pub ids: Vec<String>,
    /// Whether the GPU temperatures are needed too (Max CPU/GPU).
    pub gpu: bool,
}

/// Anything able to produce a `Reading`.
pub trait SensorSource: Send {
    fn read(&mut self) -> Result<Reading>;

    /// Narrows later readings to what feeds the display (`Some`), or goes back
    /// to everything (`None`). A source may ignore this and always read all;
    /// the automatic CPU temperature and the fans are always included.
    fn set_wanted(&mut self, _wanted: Option<Wanted>) {}
    /// Light mode (`light` = true, e.g. while the window is hidden): the
    /// backend may stop polling hardware that the chosen `temp`/`fan`
    /// sources do not need. With `light` = false it polls everything again,
    /// so the sensor lists are complete. Cheap to call every tick; a no-op
    /// where the backend has nothing to save (everything but the Windows
    /// sidecar).
    fn set_light(&mut self, _light: bool, _temp: &TempSource, _fan: &FanSource) {}
}

/// Cross-platform source based on `sysinfo`.
///
/// Delivers **CPU usage** on any OS and **CPU temperature** where `sysinfo`
/// exposes thermal components (on Apple Silicon macOS these are the
/// `PMU tdie*` sensors; on Linux, `coretemp`/`k10temp`; etc.). Fan speed does
/// **not** come from `sysinfo` — it needs a specific backend (SMC on macOS,
/// hwmon on Linux), so it is `None` here.
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

    /// Estimates the CPU temperature from the thermal components.
    ///
    /// Selects sensors whose label suggests die/CPU and whose reading is
    /// plausible (0..150 °C — discards the disconnected sensors macOS reports
    /// as ~-9199 °C) and averages them.
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

        // No label matched: fall back to the average of any plausible sensor.
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

impl SysinfoSource {
    /// CPU and RAM usage only: the cheap part of a reading. The thermal
    /// components are left out (`cpu_temp_c` is `None`); see [`Self::fill_cpu_temp`].
    pub fn read_load(&mut self) -> Reading {
        self.sys.refresh_cpu_usage();
        let cpu_load = self.sys.global_cpu_usage();

        self.sys.refresh_memory();
        let total = self.sys.total_memory();
        let ram_load = if total > 0 {
            self.sys.used_memory() as f32 / total as f32 * 100.0
        } else {
            0.0
        };

        Reading {
            cpu_load,
            ram_load,
            cpu_temp_c: None,
            // Fan: sysinfo does not expose RPM; on Windows it comes from HWiNFO (see below).
            fan_rpm: None,
            cpu_temp_id: None,
            fan_id: None,
            sensors: Vec::new(),
        }
    }

    /// Adds `sysinfo`'s CPU temperature estimate to `r`. Refreshing the thermal
    /// components is the costly part of a read, so sources with a better
    /// backend call this only when that backend has no CPU sensor.
    pub fn fill_cpu_temp(&mut self, r: &mut Reading) {
        self.components.refresh(false);
        r.cpu_temp_c = self.cpu_temperature();
    }
}

impl SensorSource for SysinfoSource {
    fn read(&mut self) -> Result<Reading> {
        let mut r = self.read_load();
        self.fill_cpu_temp(&mut r);
        Ok(r)
    }
}

/// Default source for the platform.
///
/// On Windows it uses [`WindowsSource`] (CPU/RAM from `sysinfo` + temperature and
/// fan from HWiNFO); on Linux, `LinuxSource` (CPU/RAM from `sysinfo` + hwmon);
/// on macOS, `MacSource` (CPU/RAM from `sysinfo` + IOKit HID sensors and the
/// SMC); on other platforms, [`SysinfoSource`].
pub fn default_source() -> Box<dyn SensorSource> {
    #[cfg(windows)]
    {
        Box::new(WindowsSource::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(crate::hwmon::LinuxSource::new())
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(crate::macos::MacSource::new())
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Box::new(SysinfoSource::new())
    }
}

// ============================================================================
// Windows backend — HWiNFO Shared Memory (`Global\HWiNFO_SENS_SM2`)
// ============================================================================
//
// Same source as the original app (HwiHandler → HWiNFO64.dll). With "Shared
// Memory Support" enabled, HWiNFO publishes a shared-memory block with a header
// and N "reading elements". Each element has a type (1=temp, 3=fan), labels and
// a `double` with the value. Layout from the official SDK
// (`HWiNFO_SENSORS_SHARED_MEM2` / `HWiNFO_SENSORS_READING_ELEMENT`).
//
// Reading does not require administrator; it requires HWiNFO running with SHM enabled.

/// Values extracted from an HWiNFO sample.
#[derive(Debug, Default, Clone, Copy)]
pub struct HwinfoSnapshot {
    pub temp_c: Option<f32>,
    pub fan_rpm: Option<u32>,
}

const HWINFO_SIG: &[u8; 4] = b"SiWH";
const READING_TYPE_TEMP: u32 = 1;
const READING_TYPE_FAN: u32 = 3;

// Offsets in the header.
const H_OFF_READING_SECTION: usize = 32;
const H_OFF_READING_SIZE: usize = 36;
const H_OFF_READING_COUNT: usize = 40;
const HEADER_MIN_LEN: usize = 44;

// Offsets inside a reading element.
const R_OFF_TYPE: usize = 0;
const R_OFF_LABEL_USER: usize = 140; // after tReading/index/id (12) + szLabelOrig (128)
const R_OFF_VALUE: usize = 284; // after szLabelUser (128) + szUnit (16)
const R_MIN_LEN: usize = 292;

fn rd_u32(buf: &[u8], off: usize) -> Option<u32> {
    buf.get(off..off + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn rd_f64(buf: &[u8], off: usize) -> Option<f64> {
    buf.get(off..off + 8).map(|b| {
        f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
    })
}

/// Reads a C label (NUL-terminated) as a string (bytes >=0x20, plain latin-1).
fn rd_label(buf: &[u8], off: usize, max: usize) -> String {
    let end = (off + max).min(buf.len());
    let slice = &buf[off.min(buf.len())..end];
    let bytes: Vec<u8> = slice.iter().copied().take_while(|&b| b != 0).collect();
    bytes.iter().map(|&b| b as char).collect()
}

fn label_matches(label: &str, keys: &[&str]) -> bool {
    let l = label.to_lowercase();
    keys.iter().any(|k| l.contains(k))
}

/// **Pure** parser of the HWiNFO SHM block (no OS calls — testable).
///
/// Picks:
///   * **temperature**: label with `cpu/tctl/tdie/core/package` and a plausible
///     value (0..=150 °C); otherwise the first plausible temperature.
///   * **fan**: label with `pump/cpu/aio/water/liquid`; otherwise the first fan > 0.
pub(crate) fn parse_hwinfo(buf: &[u8]) -> HwinfoSnapshot {
    let mut out = HwinfoSnapshot::default();
    if buf.len() < HEADER_MIN_LEN || &buf[0..4] != HWINFO_SIG {
        return out;
    }
    let (Some(sec), Some(size), Some(count)) = (
        rd_u32(buf, H_OFF_READING_SECTION),
        rd_u32(buf, H_OFF_READING_SIZE),
        rd_u32(buf, H_OFF_READING_COUNT),
    ) else {
        return out;
    };
    let (sec, size, count) = (sec as usize, size as usize, count as usize);
    if size < R_MIN_LEN || count == 0 {
        return out;
    }

    let mut temp_cpu: Option<f32> = None;
    let mut temp_any: Option<f32> = None;
    let mut fan_pref: Option<u32> = None;
    let mut fan_any: Option<u32> = None;

    for i in 0..count {
        let base = sec + i * size;
        let Some(el) = buf.get(base..base + size) else { break };
        let Some(kind) = rd_u32(el, R_OFF_TYPE) else { continue };
        let Some(value) = rd_f64(el, R_OFF_VALUE) else { continue };
        if !value.is_finite() {
            continue;
        }
        let label = rd_label(el, R_OFF_LABEL_USER, 128);

        match kind {
            READING_TYPE_TEMP if (0.0..=150.0).contains(&value) => {
                let v = value as f32;
                temp_any.get_or_insert(v);
                if temp_cpu.is_none()
                    && label_matches(&label, &["cpu", "tctl", "tdie", "core", "package"])
                {
                    temp_cpu = Some(v);
                }
            }
            READING_TYPE_FAN if value > 0.0 && value < 100_000.0 => {
                let v = value.round() as u32;
                fan_any.get_or_insert(v);
                if fan_pref.is_none()
                    && label_matches(&label, &["pump", "cpu", "aio", "water", "liquid"])
                {
                    fan_pref = Some(v);
                }
            }
            _ => {}
        }
    }

    out.temp_c = temp_cpu.or(temp_any);
    out.fan_rpm = fan_pref.or(fan_any);
    out
}

#[cfg(windows)]
mod hwinfo_shm {
    use super::{parse_hwinfo, HwinfoSnapshot};
    use core::ffi::c_void;

    const FILE_MAP_READ: u32 = 0x0004;
    const SHM_NAME: &str = "Global\\HWiNFO_SENS_SM2";
    // Read ceiling: the real HWiNFO block is a few MB at most.
    const MAX_MAP_BYTES: usize = 16 * 1024 * 1024;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenFileMappingW(
            dw_desired_access: u32,
            b_inherit_handle: i32,
            lp_name: *const u16,
        ) -> *mut c_void;
        fn MapViewOfFile(
            h_file_mapping_object: *mut c_void,
            dw_desired_access: u32,
            dw_file_offset_high: u32,
            dw_file_offset_low: u32,
            dw_number_of_bytes_to_map: usize,
        ) -> *mut c_void;
        fn UnmapViewOfFile(lp_base_address: *const c_void) -> i32;
        fn CloseHandle(h_object: *mut c_void) -> i32;
    }

    /// Reads an HWiNFO sample, or `None` if the SHM is not available.
    pub fn read() -> Option<HwinfoSnapshot> {
        let mut name: Vec<u16> = SHM_NAME.encode_utf16().collect();
        name.push(0);

        // SAFETY: valid names; null pointers are handled; the view is read-only
        // and unmapped/closed before returning.
        unsafe {
            let mapping = OpenFileMappingW(FILE_MAP_READ, 0, name.as_ptr());
            if mapping.is_null() {
                return None;
            }
            let view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
            if view.is_null() {
                CloseHandle(mapping);
                return None;
            }
            // We map the whole view (0 = the entire mapping). We do not know the
            // exact size here; we read the header and deduce the end of the
            // readings section, capped by MAX_MAP_BYTES.
            let head = std::slice::from_raw_parts(view as *const u8, 44);
            let sec = u32::from_le_bytes([head[32], head[33], head[34], head[35]]) as usize;
            let size = u32::from_le_bytes([head[36], head[37], head[38], head[39]]) as usize;
            let count = u32::from_le_bytes([head[40], head[41], head[42], head[43]]) as usize;
            let needed = sec
                .saturating_add(size.saturating_mul(count))
                .min(MAX_MAP_BYTES);
            let full = std::slice::from_raw_parts(view as *const u8, needed.max(44));
            let snap = parse_hwinfo(full);
            UnmapViewOfFile(view);
            CloseHandle(mapping);
            Some(snap)
        }
    }
}

// ============================================================================
// Windows backend — embedded HWiNFO engine (`config.dat` = `HWiNFO64.dll`)
// ============================================================================
//
// What the original app does (`RunHwinfo`, `FUN_140002d80`): `LoadLibraryA(
// "./config.dat")` — which is the HWiNFO SDK DLL renamed, with its own
// HVCI-compatible driver — and resolves the functions **by ordinal**:
//
//   590 Init(flags=0x40) -> int (0 = ok)      883 NumSensors() -> int
//   726 Update()                               749 SelectSensor(int)
//   106 GetReading(type, sensor, index, *Raw) -> bool
//
// The read loop (`FUN_1400027e0`) walks sensor × index (0..0x200) ×
// type ∈ {1 temp, 3 fan}. Requires administrator (the driver is loaded on the fly).

/// A reading from the HWiNFO engine (label + value), for diagnostics/selection.
#[derive(Debug, Clone)]
pub struct HwinfoReading {
    pub kind: u32,
    pub sensor: i32,
    pub index: i32,
    pub sensor_name: String,
    pub label: String,
    pub value: f64,
}

/// Stable id of an HWiNFO engine reading in the common sensor list.
pub(crate) fn hwinfo_id(r: &HwinfoReading) -> String {
    format!("hwinfo/{}/{}/{}", r.sensor, r.index, r.kind)
}

/// Converts HWiNFO engine readings into the common sensor list.
///
/// Applies the same sanity filters as the other sources. The hardware type is
/// inferred from the sensor group name (HWiNFO has no type enum), which is
/// enough for the GPU/CPU heuristics of the selection.
pub(crate) fn sensors_from_hwinfo(readings: &[HwinfoReading]) -> Vec<SensorValue> {
    readings
        .iter()
        .filter_map(|r| {
            let kind = match r.kind {
                READING_TYPE_TEMP if (0.0..=150.0).contains(&r.value) => SensorKind::Temp,
                READING_TYPE_FAN if r.value > 0.0 && r.value < 100_000.0 => SensorKind::Fan,
                _ => return None,
            };
            let group = r.sensor_name.to_lowercase();
            let hardware_type = if group.contains("gpu") {
                "Gpu"
            } else if group.contains("cpu") {
                "Cpu"
            } else if group.contains("drive") || group.contains("smart") {
                "Storage"
            } else {
                "Other"
            };
            Some(SensorValue {
                id: hwinfo_id(r),
                hardware: r.sensor_name.clone(),
                hardware_type: hardware_type.into(),
                name: r.label.clone(),
                kind,
                value: r.value as f32,
            })
        })
        .collect()
}

#[cfg(windows)]
pub mod hwinfo_dll {
    use super::{
        hwinfo_id, sensors_from_hwinfo, HwinfoReading, HwinfoSnapshot, SensorKind, SensorValue,
        READING_TYPE_FAN, READING_TYPE_TEMP,
    };
    use anyhow::{anyhow, Result};
    use core::ffi::c_void;
    use std::path::PathBuf;

    const ORD_INIT: usize = 590;
    const ORD_NUM_SENSORS: usize = 883;
    const ORD_UPDATE: usize = 726;
    const ORD_SELECT_SENSOR: usize = 749;
    const ORD_GET_READING: usize = 106;

    /// Maximum indices per sensor that the original scans (`0x200`).
    const MAX_INDEX: i32 = 0x200;

    /// Output struct of `GetReading` — 0x1D0 bytes (copied whole in the original).
    const RAW_LEN: usize = 0x1D0;
    const RAW_OFF_VALUE: usize = 0x08; // f64
    const RAW_OFF_SENSOR_NAME: usize = 0x3C; // char[256]
    const RAW_OFF_LABEL: usize = 0x148; // char[128] — where the original looks for "package"/"cpu"
    const RAW_OFF_TYPE: usize = 0x1C8; // u32

    type FnInit = unsafe extern "C" fn(u32) -> i32;
    type FnNumSensors = unsafe extern "C" fn() -> i32;
    type FnUpdate = unsafe extern "C" fn();
    type FnSelectSensor = unsafe extern "C" fn(i32);
    type FnGetReading = unsafe extern "C" fn(u32, i32, i32, *mut u8) -> bool;

    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(lp_lib_file_name: *const u16) -> *mut c_void;
        fn GetProcAddress(h_module: *mut c_void, lp_proc_name: *const u8) -> *mut c_void;
    }

    /// Where to look for the DLL, in order: `KALKAN_HWINFO_DLL`; `config.dat` /
    /// `HWiNFO64.dll` next to the executable; the original app's install.
    fn candidates() -> Vec<PathBuf> {
        let mut v = Vec::new();
        if let Some(p) = std::env::var_os("KALKAN_HWINFO_DLL") {
            v.push(PathBuf::from(p));
        }
        if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
            v.push(dir.join("config.dat"));
            v.push(dir.join("HWiNFO64.dll"));
        }
        v.push(PathBuf::from(
            r"C:\Program Files (x86)\Kalkan PC SmartMonitor DZ V1.3\config.dat",
        ));
        v
    }

    /// Loaded and initialized HWiNFO engine. (The DLL stays loaded until the
    /// process ends, as in the original.)
    pub struct Hwinfo {
        update: FnUpdate,
        num_sensors: FnNumSensors,
        select_sensor: FnSelectSensor,
        get_reading: FnGetReading,
        pub path: PathBuf,
    }

    // SAFETY: function pointers into a DLL that is never unloaded; usage is
    // serialized by the caller's `&mut self`.
    unsafe impl Send for Hwinfo {}

    impl Hwinfo {
        /// Loads the first DLL found and calls `Init(0x40)`.
        pub fn load() -> Result<Self> {
            let path = candidates()
                .into_iter()
                .find(|p| p.is_file())
                .ok_or_else(|| anyhow!("HWiNFO DLL (config.dat) not found"))?;
            let mut wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
            wide.push(0);

            // SAFETY: NUL-terminated path; ordinals checked against the original
            // binary; we check each pointer before the transmute.
            unsafe {
                let module = LoadLibraryW(wide.as_ptr());
                if module.is_null() {
                    return Err(anyhow!("failed to load {}", path.display()));
                }
                let proc = |ord: usize| -> Result<*mut c_void> {
                    let p = GetProcAddress(module, ord as *const u8);
                    if p.is_null() {
                        Err(anyhow!("ordinal {ord} missing in {}", path.display()))
                    } else {
                        Ok(p)
                    }
                };
                let init: FnInit = std::mem::transmute(proc(ORD_INIT)?);
                let hw = Self {
                    update: std::mem::transmute(proc(ORD_UPDATE)?),
                    num_sensors: std::mem::transmute(proc(ORD_NUM_SENSORS)?),
                    select_sensor: std::mem::transmute(proc(ORD_SELECT_SENSOR)?),
                    get_reading: std::mem::transmute(proc(ORD_GET_READING)?),
                    path: path.clone(),
                };
                let rc = init(0x40);
                if rc != 0 {
                    return Err(anyhow!(
                        "HWiNFO Init failed ({rc}) — the app must run as administrator"
                    ));
                }
                (hw.update)();
                Ok(hw)
            }
        }

        /// Updates and lists all temperature and fan readings.
        pub fn readings(&mut self) -> Vec<HwinfoReading> {
            let mut out = Vec::new();
            let mut raw = vec![0u8; RAW_LEN];
            // SAFETY: `raw` has the size the original reserves for the struct.
            unsafe {
                let n = (self.num_sensors)();
                (self.update)();
                for sensor in 0..n {
                    (self.select_sensor)(sensor);
                    for index in 0..MAX_INDEX {
                        for kind in [READING_TYPE_TEMP, READING_TYPE_FAN] {
                            raw.fill(0);
                            if !(self.get_reading)(kind, sensor, index, raw.as_mut_ptr()) || raw[0] == 0 {
                                continue;
                            }
                            out.push(HwinfoReading {
                                kind: u32::from_le_bytes(raw[RAW_OFF_TYPE..RAW_OFF_TYPE + 4].try_into().unwrap()),
                                sensor,
                                index,
                                sensor_name: cstr(&raw[RAW_OFF_SENSOR_NAME..RAW_OFF_SENSOR_NAME + 256]),
                                label: cstr(&raw[RAW_OFF_LABEL..RAW_OFF_LABEL + 128]),
                                value: f64::from_le_bytes(raw[RAW_OFF_VALUE..RAW_OFF_VALUE + 8].try_into().unwrap()),
                            });
                        }
                    }
                }
            }
            out
        }

        /// CPU temperature and fan, with the same choice as the original
        /// (`FUN_1400023b0`): temp with "package"/"average" in the label; fan with
        /// "cpu". With no match, falls back to the first plausible reading.
        pub fn snapshot(&mut self) -> HwinfoSnapshot {
            let all = self.readings();
            let (t, f) = pick(&all);
            HwinfoSnapshot {
                temp_c: t.map(|r| r.value as f32),
                fan_rpm: f.map(|r| r.value.round() as u32),
            }
        }

        /// Re-reads only an already-known reading (after `update`).
        fn value_of(&mut self, key: (u32, i32, i32)) -> Option<f64> {
            let (kind, sensor, index) = key;
            let mut raw = vec![0u8; RAW_LEN];
            // SAFETY: same as `readings`.
            unsafe {
                (self.select_sensor)(sensor);
                if !(self.get_reading)(kind, sensor, index, raw.as_mut_ptr()) || raw[0] == 0 {
                    return None;
                }
            }
            Some(f64::from_le_bytes(raw[RAW_OFF_VALUE..RAW_OFF_VALUE + 8].try_into().unwrap()))
        }
    }

    /// Temp/fan source on top of the HWiNFO engine: scans everything once to pick
    /// the readings (like the original in `RunHwinfo`) and then re-reads only those two.
    pub struct HwinfoSource {
        hw: Hwinfo,
        temp_key: Option<(u32, i32, i32)>,
        fan_key: Option<(u32, i32, i32)>,
        /// Ids (in the `sensors` list) of the picked temperature and fan.
        temp_id: Option<String>,
        fan_id: Option<String>,
        /// Every temp/fan reading found by the initial scan (key + metadata).
        known: Vec<((u32, i32, i32), SensorValue)>,
    }

    impl HwinfoSource {
        pub fn new() -> Result<Self> {
            let mut hw = Hwinfo::load()?;
            let all = hw.readings();
            let (t, f) = pick(&all);
            let key = |r: &HwinfoReading| (r.kind, r.sensor, r.index);
            // `sensors_from_hwinfo` drops readings that fail the sanity filters,
            // so convert one at a time to keep each kept sensor paired with its key.
            let known = all
                .iter()
                .filter_map(|r| {
                    sensors_from_hwinfo(std::slice::from_ref(r)).pop().map(|s| (key(r), s))
                })
                .collect();
            Ok(Self {
                temp_key: t.map(key),
                fan_key: f.map(key),
                temp_id: t.map(hwinfo_id),
                fan_id: f.map(hwinfo_id),
                known,
                hw,
            })
        }

        pub fn read(&mut self) -> HwinfoSnapshot {
            // SAFETY: function pointer valid while the DLL is loaded.
            unsafe { (self.hw.update)() };
            let temp = self.temp_key.and_then(|k| self.hw.value_of(k));
            let fan = self.fan_key.and_then(|k| self.hw.value_of(k));
            HwinfoSnapshot {
                temp_c: temp.filter(|v| (0.0..=150.0).contains(v)).map(|v| v as f32),
                fan_rpm: fan.filter(|v| *v >= 0.0 && *v < 100_000.0).map(|v| v.round() as u32),
            }
        }
    }

    impl HwinfoSource {
        /// Id of the sensor `read` takes the temperature from.
        pub fn temp_id(&self) -> Option<String> {
            self.temp_id.clone()
        }

        /// Id of the sensor `read` takes the fan speed from.
        pub fn fan_id(&self) -> Option<String> {
            self.fan_id.clone()
        }

        /// Current values of every sensor found at startup. Call after `read`
        /// (which refreshes the engine); readings that went invalid are dropped.
        pub fn sensors(&mut self) -> Vec<SensorValue> {
            let mut out = Vec::with_capacity(self.known.len());
            for i in 0..self.known.len() {
                let key = self.known[i].0;
                let Some(v) = self.hw.value_of(key) else { continue };
                let mut s = self.known[i].1.clone();
                let ok = match s.kind {
                    SensorKind::Temp => (0.0..=150.0).contains(&v),
                    SensorKind::Fan => v > 0.0 && v < 100_000.0,
                };
                if ok {
                    s.value = v as f32;
                    out.push(s);
                }
            }
            out
        }
    }

    pub(crate) fn pick(readings: &[HwinfoReading]) -> (Option<&HwinfoReading>, Option<&HwinfoReading>) {
        let has = |r: &HwinfoReading, keys: &[&str]| {
            let l = r.label.to_lowercase();
            keys.iter().any(|k| l.contains(k))
        };
        let temps = || {
            readings
                .iter()
                .filter(|r| r.kind == READING_TYPE_TEMP && (0.0..=150.0).contains(&r.value))
        };
        let fans = || {
            readings
                .iter()
                .filter(|r| r.kind == READING_TYPE_FAN && r.value > 0.0 && r.value < 100_000.0)
        };
        let temp = temps()
            .find(|r| has(r, &["package", "average"]))
            .or_else(|| temps().next());
        let fan = fans().find(|r| has(r, &["cpu"])).or_else(|| fans().next());
        (temp, fan)
    }

    fn cstr(bytes: &[u8]) -> String {
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    }
}

// ============================================================================
// Self-contained backend — LibreHardwareMonitor sidecar (`kalkan-sensord.exe`)
// ============================================================================
//
// Main path (self-contained, like the original): a .NET helper process that
// loads LibreHardwareMonitor (engine + its own kernel driver) and emits
// readings as one JSON line per line on stdout. We read that output. It sits
// next to the executable (or via env KALKAN_SENSORD). When the sidecar is
// absent or delivers no data, we fall back to HWiNFO SHM and finally to `None`.

/// A parsed sidecar line.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SidecarLine {
    pub temp_c: Option<f32>,
    pub fan_rpm: Option<u32>,
    /// Ids of the sensors the two values were picked from; `None` for
    /// old-format lines.
    pub cpu_temp_id: Option<String>,
    pub fan_id: Option<String>,
    /// Empty for old-format lines (no `sensors` key).
    pub sensors: Vec<SensorValue>,
    /// Produced in light mode (`"mode":"light"`): `sensors` only covers the
    /// polled hardware groups.
    pub light: bool,
}

/// Parses a sidecar JSON line:
/// `{"cpu_temp_c":42.0,"cpu_temp_id":"/lpc/0/temperature/0","fan_rpm":2400,"fan_id":"/lpc/0/fan/1",
/// "sensors":[{"id":"/lpc/0/fan/1","hardware":"NCT6799D",
/// "hardware_type":"SuperIO","name":"Fan #2","kind":"fan","value":2400}]}`
/// (values may be `null`; `cpu_temp_id`, `fan_id` and `sensors` are optional,
/// and malformed entries are skipped). A light-mode line ends with
/// `"mode":"light"` and lists only the polled groups' sensors.
///
/// Returns `None` if the line is not an object with the `cpu_temp_c` key.
pub(crate) fn parse_sensor_line(line: &str) -> Option<SidecarLine> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let obj = v.as_object()?;
    if !obj.contains_key("cpu_temp_c") {
        return None;
    }
    let temp_c = obj.get("cpu_temp_c").and_then(|x| x.as_f64()).map(|v| v as f32);
    let fan_rpm = obj.get("fan_rpm").and_then(|x| x.as_f64()).map(|v| v.round() as u32);
    let id = |k: &str| obj.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let (cpu_temp_id, fan_id) = (id("cpu_temp_id"), id("fan_id"));
    let sensors = obj
        .get("sensors")
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(parse_sensor_entry).collect())
        .unwrap_or_default();
    let light = crate::light::is_light_line(obj);
    Some(SidecarLine { temp_c, fan_rpm, cpu_temp_id, fan_id, sensors, light })
}

fn parse_sensor_entry(v: &serde_json::Value) -> Option<SensorValue> {
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let kind = match v.get("kind")?.as_str()? {
        "temp" => SensorKind::Temp,
        "fan" => SensorKind::Fan,
        _ => return None,
    };
    let value = v.get("value")?.as_f64()?;
    if !value.is_finite() {
        return None;
    }
    Some(SensorValue {
        id: text("id")?,
        hardware: text("hardware").unwrap_or_default(),
        hardware_type: text("hardware_type").unwrap_or_default(),
        name: text("name")?,
        kind,
        value: value as f32,
    })
}

#[cfg(windows)]
mod sidecar {
    use super::{parse_sensor_line, SensorValue};
    use std::io::{BufRead, BufReader, Write};
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct Shared {
        temp_c: Option<f32>,
        fan_rpm: Option<u32>,
        cpu_temp_id: Option<String>,
        fan_id: Option<String>,
        sensors: Vec<SensorValue>,
        light: bool,
        at: Option<Instant>,
    }

    /// Latest sidecar data.
    #[derive(Default)]
    pub struct Latest {
        /// A recent line was available (the rest is empty otherwise).
        pub fresh: bool,
        pub temp_c: Option<f32>,
        pub fan_rpm: Option<u32>,
        pub cpu_temp_id: Option<String>,
        pub fan_id: Option<String>,
        pub sensors: Vec<SensorValue>,
        /// The line came from light mode (partial `sensors`).
        pub light: bool,
    }

    /// Running sidecar process; terminated on `Drop`.
    pub struct Sidecar {
        child: Child,
        state: Arc<Mutex<Shared>>,
        /// Commands for the sidecar's stdin, written by a thread of their own
        /// so a sidecar that stops reading can never block the caller.
        commands: Option<mpsc::Sender<String>>,
    }

    /// Resolves the path of `kalkan-sensord.exe`: env `KALKAN_SENSORD`, next to
    /// the executable, or development paths in the repository.
    fn resolve_path() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("KALKAN_SENSORD") {
            let p = PathBuf::from(p);
            if p.is_file() {
                return Some(p);
            }
        }
        let mut cands: Vec<PathBuf> = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                cands.push(dir.join("kalkan-sensord.exe"));
            }
        }
        cands.push(PathBuf::from(
            "sidecar/kalkan-sensord/bin/publish-multi/kalkan-sensord.exe",
        ));
        cands.push(PathBuf::from(
            "sidecar/kalkan-sensord/bin/Release/net8.0/win-x64/publish/kalkan-sensord.exe",
        ));
        cands.into_iter().find(|p| p.is_file())
    }

    impl Sidecar {
        /// Spawns the sidecar and starts the reader thread; `None` if it does not exist.
        pub fn spawn() -> Option<Self> {
            use std::os::windows::process::CommandExt;
            // Without this, Windows opens a console window for the sidecar
            // when the parent is a windowed app (the UI).
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;

            let path = resolve_path()?;
            let mut child = Command::new(&path)
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            let stdout = child.stdout.take()?;
            // Mode commands (see `crate::light`). The sidecar ignores EOF on
            // stdin; it still ends when this process kills it (`Drop`) or,
            // if this process dies, on its next write to the broken stdout.
            let commands = child.stdin.take().map(|mut stdin| {
                let (tx, rx) = mpsc::channel::<String>();
                std::thread::spawn(move || {
                    for cmd in rx {
                        if stdin.write_all(cmd.as_bytes()).and_then(|_| stdin.flush()).is_err() {
                            break;
                        }
                    }
                });
                tx
            });
            let state = Arc::new(Mutex::new(Shared::default()));
            let st = state.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if let Some(l) = parse_sensor_line(&line) {
                        if let Ok(mut s) = st.lock() {
                            s.temp_c = l.temp_c;
                            s.fan_rpm = l.fan_rpm;
                            s.cpu_temp_id = l.cpu_temp_id;
                            s.fan_id = l.fan_id;
                            s.sensors = l.sensors;
                            s.light = l.light;
                            s.at = Some(Instant::now());
                        }
                    }
                }
            });
            Some(Self { child, state, commands })
        }

        /// Queues commands (newline-terminated lines) for the sidecar's stdin.
        pub fn send(&self, cmd: String) {
            if let Some(tx) = &self.commands {
                let _ = tx.send(cmd);
            }
        }

        /// Latest reading, if recent (≤ 5 s); otherwise empty.
        pub fn latest(&self) -> Latest {
            if let Ok(s) = self.state.lock() {
                if matches!(s.at, Some(at) if at.elapsed() < Duration::from_secs(5)) {
                    return Latest {
                        fresh: true,
                        light: s.light,
                        temp_c: s.temp_c,
                        fan_rpm: s.fan_rpm,
                        cpu_temp_id: s.cpu_temp_id.clone(),
                        fan_id: s.fan_id.clone(),
                        sensors: s.sensors.clone(),
                    };
                }
            }
            Latest::default()
        }
    }

    impl Drop for Sidecar {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Windows sensor source.
///
/// CPU/RAM load come from `sysinfo`. Temperature and fan come, in order, from the
/// **LibreHardwareMonitor sidecar** (`kalkan-sensord.exe`, with the PawnIO driver
/// — works under HVCI); otherwise from the **original app's HWiNFO engine**
/// (`config.dat`, if installed); otherwise from **HWiNFO Shared Memory** (if
/// HWiNFO is running); otherwise `None`. See `docs/sensors-windows.md`.
#[cfg(windows)]
pub struct WindowsSource {
    sys: SysinfoSource,
    sidecar: Option<sidecar::Sidecar>,
    started: std::time::Instant,
    hwinfo: Option<hwinfo_dll::HwinfoSource>,
    hwinfo_tried: bool,
    /// Light/full mode of the sidecar and the last full sensor list.
    planner: crate::light::Planner,
}

/// How long to wait for the sidecar to deliver its first reading before turning
/// to the HWiNFO engine (LibreHardwareMonitor takes a few seconds to open).
#[cfg(windows)]
const SIDECAR_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(windows)]
impl WindowsSource {
    pub fn new() -> Self {
        Self {
            sys: SysinfoSource::new(),
            sidecar: sidecar::Sidecar::spawn(),
            started: std::time::Instant::now(),
            hwinfo: None,
            hwinfo_tried: false,
            planner: Default::default(),
        }
    }
}

#[cfg(windows)]
impl Default for WindowsSource {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
impl SensorSource for WindowsSource {
    fn read(&mut self) -> Result<Reading> {
        let mut r = self.sys.read_load();

        let latest = match &self.sidecar {
            Some(sc) => sc.latest(),
            None => Default::default(),
        };
        let (mut temp, mut fan) = (latest.temp_c, latest.fan_rpm);
        let (mut temp_id, mut fan_id) = (latest.cpu_temp_id, latest.fan_id);
        // A light line only lists the polled groups: show it over the last
        // full list so the selectors keep their entries (see `Planner::merge`).
        let mut sensors = if !latest.fresh {
            latest.sensors
        } else if latest.light {
            self.planner.merge(&latest.sensors)
        } else {
            self.planner.learn_full(&latest.sensors, temp_id.as_deref(), fan_id.as_deref());
            latest.sensors
        };

        // Sidecar missing, or no data after the grace period (e.g. HVCI without
        // PawnIO): try the original app's HWiNFO engine once.
        let sidecar_silent = self.sidecar.is_none() || self.started.elapsed() > SIDECAR_GRACE;
        if temp.is_none() && fan.is_none() && sidecar_silent && !self.hwinfo_tried {
            self.hwinfo_tried = true;
            self.hwinfo = hwinfo_dll::HwinfoSource::new().ok();
            if self.hwinfo.is_some() {
                self.sidecar = None; // shut down the useless sidecar
            }
        }
        if let Some(hw) = &mut self.hwinfo {
            let s = hw.read();
            if temp.is_none() && s.temp_c.is_some() {
                temp = s.temp_c;
                temp_id = hw.temp_id();
            }
            if fan.is_none() && s.fan_rpm.is_some() {
                fan = s.fan_rpm;
                fan_id = hw.fan_id();
            }
            if sensors.is_empty() {
                sensors = hw.sensors();
            }
        }

        // HWiNFO Shared Memory fallback for whatever is still missing.
        if temp.is_none() || fan.is_none() {
            if let Some(hw) = hwinfo_shm::read() {
                temp = temp.or(hw.temp_c);
                fan = fan.or(hw.fan_rpm);
            }
        }

        // Values from the HWiNFO shared memory carry no sensor id.
        // sysinfo's estimate is only read when nothing better exists, since
        // refreshing its components is the costly part.
        match temp {
            Some(t) => {
                r.cpu_temp_c = Some(t);
                r.cpu_temp_id = temp_id;
            }
            None => self.sys.fill_cpu_temp(&mut r),
        }
        if let Some(f) = fan {
            r.fan_rpm = Some(f);
            r.fan_id = fan_id;
        }
        r.sensors = sensors;
        Ok(r)
    }

    fn set_light(&mut self, light: bool, temp: &TempSource, fan: &FanSource) {
        if let Some(sc) = &self.sidecar {
            if let Some(cmd) = self.planner.update(light, temp, fan) {
                sc.send(cmd);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a synthetic SHM block (header + N readings) for the parser.
    fn synth(readings: &[(u32, &str, f64)]) -> Vec<u8> {
        let size = R_MIN_LEN; // element size in this test
        let sec = HEADER_MIN_LEN;
        let mut buf = vec![0u8; sec + readings.len() * size];
        buf[0..4].copy_from_slice(HWINFO_SIG);
        buf[H_OFF_READING_SECTION..H_OFF_READING_SECTION + 4]
            .copy_from_slice(&(sec as u32).to_le_bytes());
        buf[H_OFF_READING_SIZE..H_OFF_READING_SIZE + 4]
            .copy_from_slice(&(size as u32).to_le_bytes());
        buf[H_OFF_READING_COUNT..H_OFF_READING_COUNT + 4]
            .copy_from_slice(&(readings.len() as u32).to_le_bytes());
        for (i, (kind, label, value)) in readings.iter().enumerate() {
            let base = sec + i * size;
            buf[base + R_OFF_TYPE..base + R_OFF_TYPE + 4].copy_from_slice(&kind.to_le_bytes());
            let lb = label.as_bytes();
            let n = lb.len().min(127);
            buf[base + R_OFF_LABEL_USER..base + R_OFF_LABEL_USER + n].copy_from_slice(&lb[..n]);
            buf[base + R_OFF_VALUE..base + R_OFF_VALUE + 8].copy_from_slice(&value.to_le_bytes());
        }
        buf
    }

    #[test]
    fn hwinfo_picks_cpu_temp_and_pump_fan() {
        let buf = synth(&[
            (READING_TYPE_TEMP, "Motherboard", 30.0),
            (READING_TYPE_TEMP, "CPU (Tctl/Tdie)", 42.0),
            (READING_TYPE_FAN, "Chassis Fan #1", 800.0),
            (READING_TYPE_FAN, "Pump", 2400.0),
            (3, "unit?", -1.0), // invalid fan (<=0) ignored
        ]);
        let s = parse_hwinfo(&buf);
        assert_eq!(s.temp_c, Some(42.0)); // preferred the CPU label
        assert_eq!(s.fan_rpm, Some(2400)); // preferred "Pump"
    }

    /// The original's rule (`FUN_1400023b0`), with real labels from a Ryzen 9800X3D
    /// + Nuvoton NCT6799D read by the HWiNFO engine.
    #[cfg(windows)]
    #[test]
    fn hwinfo_dll_pick_matches_original_rule() {
        let r = |kind, label: &str, value| HwinfoReading {
            kind,
            sensor: 0,
            index: 0,
            sensor_name: String::new(),
            label: label.into(),
            value,
        };
        let all = [
            r(READING_TYPE_TEMP, "CPU (Tctl/Tdie)", 47.0),
            r(READING_TYPE_TEMP, "CPU Die (average)", 49.9),
            r(READING_TYPE_TEMP, "AUXTIN1", 118.0),
            r(READING_TYPE_FAN, "AUXFANIN0", 3409.0),
            r(READING_TYPE_FAN, "CPUFANIN0", 1197.0),
        ];
        let (t, f) = hwinfo_dll::pick(&all);
        assert_eq!(t.map(|x| x.label.as_str()), Some("CPU Die (average)"));
        assert_eq!(f.map(|x| x.label.as_str()), Some("CPUFANIN0"));
    }

    #[test]
    fn hwinfo_falls_back_to_first_plausible() {
        let buf = synth(&[
            (READING_TYPE_TEMP, "VRM", 50.0),
            (READING_TYPE_FAN, "Sys Fan", 1000.0),
        ]);
        let s = parse_hwinfo(&buf);
        assert_eq!(s.temp_c, Some(50.0)); // no CPU label → first temp
        assert_eq!(s.fan_rpm, Some(1000)); // no pump/cpu → first fan
    }

    #[test]
    fn hwinfo_rejects_bad_signature() {
        let mut buf = synth(&[(READING_TYPE_TEMP, "CPU", 40.0)]);
        buf[0] = b'X';
        let s = parse_hwinfo(&buf);
        assert_eq!(s.temp_c, None);
        assert_eq!(s.fan_rpm, None);
    }

    fn line(t: Option<f32>, f: Option<u32>) -> SidecarLine {
        SidecarLine { temp_c: t, fan_rpm: f, ..Default::default() }
    }

    #[test]
    fn sidecar_line_parse_values() {
        // Old format (no `sensors` key) still parses.
        assert_eq!(
            parse_sensor_line("{\"cpu_temp_c\":42.5,\"fan_rpm\":2400}"),
            Some(line(Some(42.5), Some(2400)))
        );
    }

    #[test]
    fn sidecar_line_parse_picked_ids() {
        let l = parse_sensor_line(
            r#"{"cpu_temp_c":42,"cpu_temp_id":"/amdcpu/0/temperature/2","fan_rpm":2400,"fan_id":"/lpc/nct6799d/0/fan/1","sensors":[]}"#,
        )
        .unwrap();
        assert_eq!(l.cpu_temp_id.as_deref(), Some("/amdcpu/0/temperature/2"));
        assert_eq!(l.fan_id.as_deref(), Some("/lpc/nct6799d/0/fan/1"));
        // Null ids (nothing picked) and the old format carry no id.
        let n = parse_sensor_line(
            r#"{"cpu_temp_c":null,"cpu_temp_id":null,"fan_rpm":null,"fan_id":null}"#,
        )
        .unwrap();
        assert_eq!((n.cpu_temp_id, n.fan_id), (None, None));
        let old = parse_sensor_line(r#"{"cpu_temp_c":42,"fan_rpm":2400,"sensors":[]}"#).unwrap();
        assert_eq!((old.cpu_temp_id, old.fan_id), (None, None));
    }

    #[test]
    fn hwinfo_ids_match_the_sensor_list() {
        let r = HwinfoReading {
            kind: READING_TYPE_FAN,
            sensor: 3,
            index: 7,
            sensor_name: "Board".into(),
            label: "CPUFANIN0".into(),
            value: 1197.0,
        };
        assert_eq!(hwinfo_id(&r), "hwinfo/3/7/3");
        assert_eq!(sensors_from_hwinfo(std::slice::from_ref(&r))[0].id, hwinfo_id(&r));
    }

    #[test]
    fn sidecar_line_parse_nulls() {
        assert_eq!(
            parse_sensor_line("{\"cpu_temp_c\":null,\"fan_rpm\":null}"),
            Some(line(None, None))
        );
    }

    #[test]
    fn sidecar_line_parse_partial_and_junk() {
        // temperature only
        assert_eq!(
            parse_sensor_line("{\"cpu_temp_c\":40,\"fan_rpm\":null}"),
            Some(line(Some(40.0), None))
        );
        // invalid line
        assert_eq!(parse_sensor_line("iniciando..."), None);
        assert_eq!(parse_sensor_line("{\"outra\":1}"), None);
    }

    #[test]
    fn sidecar_line_parse_sensor_list() {
        let l = parse_sensor_line(
            r#"{"cpu_temp_c":42,"fan_rpm":2400,"sensors":[
            {"id":"/lpc/nct6799d/0/fan/1","hardware":"NCT6799D","hardware_type":"SuperIO","name":"Fan #2","kind":"fan","value":2400},
            {"id":"/gpu-nvidia/0/temperature/0","hardware":"RTX 4090","hardware_type":"GpuNvidia","name":"GPU Core","kind":"temp","value":61.5},
            {"id":"/x","hardware":"H","hardware_type":"T","name":"bad kind","kind":"load","value":1},
            {"id":"/y","name":"no value","kind":"temp"},
            {"id":"/z","hardware":"H","hardware_type":"Cpu","name":"null value","kind":"temp","value":null}]}"#,
        )
        .unwrap();
        assert_eq!(l.temp_c, Some(42.0));
        assert_eq!(l.fan_rpm, Some(2400));
        assert_eq!(l.sensors.len(), 2); // malformed entries skipped
        assert_eq!(l.sensors[0].id, "/lpc/nct6799d/0/fan/1");
        assert_eq!(l.sensors[0].kind, SensorKind::Fan);
        assert_eq!(l.sensors[0].value, 2400.0);
        assert_eq!(l.sensors[1].hardware_type, "GpuNvidia");
        assert_eq!(l.sensors[1].label(), "RTX 4090 — GPU Core");
    }

    #[test]
    fn sidecar_line_escapes_decode() {
        let l = parse_sensor_line(
            r#"{"cpu_temp_c":null,"fan_rpm":null,"sensors":[{"id":"/a","hardware":"Núcleo","hardware_type":"Cpu","name":"\"Q\"","kind":"temp","value":30}]}"#,
        )
        .unwrap();
        assert_eq!(l.sensors[0].hardware, "N\u{fa}cleo");
        assert_eq!(l.sensors[0].name, "\"Q\"");
    }

    #[test]
    fn hwinfo_readings_to_sensors() {
        let r = |kind, sensor_name: &str, label: &str, value| HwinfoReading {
            kind,
            sensor: 2,
            index: 5,
            sensor_name: sensor_name.into(),
            label: label.into(),
            value,
        };
        let s = sensors_from_hwinfo(&[
            r(READING_TYPE_TEMP, "GPU [#0]: NVIDIA RTX", "GPU Temperature", 60.0),
            r(READING_TYPE_TEMP, "CPU [#0]: AMD", "CPU (Tctl/Tdie)", 47.0),
            r(READING_TYPE_TEMP, "ASUS board", "AUXTIN1", 118.0),
            r(READING_TYPE_TEMP, "ASUS board", "Bogus", 300.0), // out of range
            r(READING_TYPE_FAN, "ASUS board", "CPUFANIN0", 1197.0),
            r(READING_TYPE_FAN, "ASUS board", "Stopped", 0.0), // 0 RPM ignored
        ]);
        assert_eq!(s.len(), 4);
        assert_eq!(s[0].hardware_type, "Gpu");
        assert_eq!(s[1].hardware_type, "Cpu");
        assert_eq!(s[2].hardware_type, "Other");
        assert_eq!(s[3].kind, SensorKind::Fan);
        assert_eq!(s[3].id, "hwinfo/2/5/3");
    }
}
