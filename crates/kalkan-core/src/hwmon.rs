//! Linux sensors: the kernel's `hwmon` interface (`/sys/class/hwmon`).
//!
//! Every monitoring driver (k10temp, coretemp, nct6775, amdgpu, nvme, ...)
//! registers a `hwmonN` directory with one file per reading:
//! `tempN_input` (millidegrees Celsius) and `fanN_input` (RPM), optionally
//! with a `tempN_label`/`fanN_label`. Files such as `temp1_max` or
//! `temp1_crit` are thresholds, not readings, and are never read.
//!
//! The parsing takes a root path, so it compiles and is tested on every
//! platform against a fake tree; only [`LinuxSource`] is wired in as the
//! default source on Linux (see `docs/sensors-linux.md`).
//!
//! NVIDIA's proprietary driver has no hwmon. When `nvidia-smi` exists it is
//! queried from a background thread (rate limited, with a timeout), and only
//! while the GPU is awake, so a powered-down hybrid-graphics GPU is not
//! woken up by the monitor.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::sensors::{Reading, SensorKind, SensorSource, SensorValue, SysinfoSource};

/// Where the kernel exposes the hwmon devices.
pub const HWMON_ROOT: &str = "/sys/class/hwmon";

/// A sensor together with the hwmon chip (`name` file) it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct HwmonSensor {
    pub chip: String,
    pub sensor: SensorValue,
}

fn read_trim(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Hardware type for a chip name, using the vocabulary of `naming::classify`
/// (`Cpu`, `GpuAmd`, `Storage`, `Memory`, `Cooler`, `Motherboard`). Empty for
/// chips that are not recognized, so the name heuristics decide.
pub fn hardware_type(chip: &str) -> &'static str {
    let c = chip.to_ascii_lowercase();
    let starts = |keys: &[&str]| keys.iter().any(|k| c.starts_with(k));
    if c.starts_with("corsairpsu") {
        "" // a power supply, not a pump
    } else if starts(&["nzxt", "corsair", "aquacomputer", "gigabyte_waterforce", "asus_rog_ryujin"]) {
        "Cooler"
    } else if starts(&["k10temp", "zenpower", "coretemp", "cpu_thermal", "x86_pkg_temp"]) {
        "Cpu"
    } else if starts(&["amdgpu", "radeon"]) {
        "GpuAmd"
    } else if starts(&["nouveau"]) {
        "GpuNvidia"
    } else if c == "i915" || c == "xe" {
        "GpuIntel"
    } else if starts(&["nvme", "drivetemp"]) {
        "Storage"
    } else if starts(&["spd5118", "jc42"]) {
        "Memory"
    } else if starts(&[
        "nct6", "nct7", "it87", "it86", "it8", "asus", "gigabyte_wmi", "w83", "f71", "sch5",
        "acpitz", "pch_", "dell_smm", "thinkpad", "applesmc",
    ]) {
        "Motherboard"
    } else {
        ""
    }
}

/// Friendly hardware name when the device exposes no model string.
fn default_hardware(chip: &str) -> String {
    match chip.to_ascii_lowercase().as_str() {
        "k10temp" | "zenpower" => "AMD CPU".into(),
        "coretemp" => "Intel CPU".into(),
        "amdgpu" | "radeon" => "AMD GPU".into(),
        "nouveau" => "NVIDIA GPU".into(),
        "nvme" => "NVMe drive".into(),
        _ => chip.to_string(),
    }
}

/// Stable identifier of the device behind a hwmon directory: the last
/// component of its `device` symlink (PCI address, `nvme0`, `coretemp.0`...),
/// else the `hwmonN` name. The `hwmonN` index itself can change between boots.
fn device_id(dir: &Path, index: u32) -> String {
    let link = dir.join("device");
    // A link such as `../../..` has no file name of its own: resolve it.
    let from_link = fs::read_link(&link).ok().and_then(|t| {
        t.file_name().map(|n| n.to_string_lossy().into_owned()).or_else(|| {
            let real = fs::canonicalize(&link).ok()?;
            Some(real.file_name()?.to_string_lossy().into_owned())
        })
    });
    from_link.unwrap_or_else(|| format!("hwmon{index}"))
}

/// Model string of the device (NVMe model, disk model, GPU product name).
fn device_model(dir: &Path) -> Option<String> {
    ["model", "product_name"]
        .iter()
        .filter_map(|f| read_trim(dir.join("device").join(f)))
        .find(|s| !s.is_empty())
}

/// `tempN_input` / `fanN_input` → (kind, N).
fn parse_input_name(file: &str) -> Option<(SensorKind, u32)> {
    let (kind, rest) = if let Some(r) = file.strip_prefix("temp") {
        (SensorKind::Temp, r)
    } else if let Some(r) = file.strip_prefix("fan") {
        (SensorKind::Fan, r)
    } else {
        return None;
    };
    Some((kind, rest.strip_suffix("_input")?.parse().ok()?))
}

/// Sensor name from the driver's label, or a generic one. GPU labels are
/// renamed to the vocabulary the "Max CPU/GPU" source understands
/// (`edge` is the core, `junction` the hot spot).
fn sensor_name(kind: SensorKind, hw_type: &str, n: u32, label: Option<&str>) -> String {
    let is_gpu = hw_type.starts_with("Gpu");
    let label = label.filter(|l| !l.is_empty());
    match (kind, label) {
        (SensorKind::Temp, Some(l)) if is_gpu => match l.to_ascii_lowercase().as_str() {
            "edge" => "GPU Core".into(),
            "junction" => "GPU Hot Spot".into(),
            "mem" => "GPU Memory".into(),
            _ => l.to_string(),
        },
        (_, Some(l)) => l.to_string(),
        (SensorKind::Temp, None) if is_gpu && n == 1 => "GPU Core".into(),
        (SensorKind::Temp, None) if hw_type == "Cpu" && n == 1 => "CPU Temperature".into(),
        (SensorKind::Temp, None) => format!("Temperature #{n}"),
        (SensorKind::Fan, None) if is_gpu => "GPU Fan".into(),
        (SensorKind::Fan, None) => format!("Fan #{n}"),
    }
}

/// Reads every temperature and fan reading under `root`
/// (normally [`HWMON_ROOT`]). Unreadable or implausible values are skipped.
pub fn read_hwmon(root: &Path) -> Vec<HwmonSensor> {
    let mut dirs: Vec<(u32, PathBuf)> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            Some((name.strip_prefix("hwmon")?.parse().ok()?, e.path()))
        })
        .collect();
    dirs.sort_by_key(|d| d.0);

    let mut out: Vec<HwmonSensor> = Vec::new();
    for (index, dir) in dirs {
        let Some(chip) = read_trim(dir.join("name")).filter(|n| !n.is_empty()) else { continue };
        let hw_type = hardware_type(&chip);
        let devid = device_id(&dir, index);
        let hardware = device_model(&dir).unwrap_or_else(|| default_hardware(&chip));

        let mut inputs: Vec<(SensorKind, u32)> = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| parse_input_name(&e.file_name().to_string_lossy()))
            .collect();
        inputs.sort_by_key(|(k, n)| (*k == SensorKind::Fan, *n));

        for (kind, n) in inputs {
            let (prefix, word) = match kind {
                SensorKind::Temp => ("temp", "temp"),
                SensorKind::Fan => ("fan", "fan"),
            };
            let Some(raw) = read_trim(dir.join(format!("{prefix}{n}_input")))
                .and_then(|s| s.parse::<i64>().ok())
            else {
                continue;
            };
            let value = match kind {
                SensorKind::Temp => {
                    let c = raw as f32 / 1000.0;
                    if !(-50.0..=200.0).contains(&c) {
                        continue;
                    }
                    c
                }
                SensorKind::Fan => {
                    if !(0..=100_000).contains(&raw) {
                        continue;
                    }
                    raw as f32
                }
            };
            let label = read_trim(dir.join(format!("{prefix}{n}_label")));
            let mut id = format!("hwmon/{chip}/{devid}/{word}{n}");
            if out.iter().any(|s| s.sensor.id == id) {
                id = format!("{id}#{index}"); // two chips behind the same device
            }
            out.push(HwmonSensor {
                chip: chip.clone(),
                sensor: SensorValue {
                    id,
                    hardware: hardware.clone(),
                    hardware_type: hw_type.into(),
                    name: sensor_name(kind, hw_type, n, label.as_deref()),
                    kind,
                    value,
                },
            });
        }
    }
    out
}

/// The automatic CPU temperature: k10temp `Tctl` (or `Tdie`), else coretemp
/// `Package id 0`, else zenpower `Tdie`, else the first CPU sensor.
pub fn pick_cpu_temp(sensors: &[HwmonSensor]) -> Option<f32> {
    let temps = || sensors.iter().filter(|s| s.sensor.kind == SensorKind::Temp);
    let find = |chip: &str, names: &[&str]| {
        names.iter().find_map(|n| {
            temps().find(|s| s.chip.starts_with(chip) && s.sensor.name.eq_ignore_ascii_case(n))
        })
    };
    find("k10temp", &["Tctl", "Tdie"])
        .or_else(|| find("coretemp", &["Package id 0"]))
        .or_else(|| find("zenpower", &["Tdie"]))
        .or_else(|| temps().find(|s| s.sensor.hardware_type == "Cpu"))
        .map(|s| s.sensor.value)
}

/// The automatic fan/pump: a running fan labeled pump, else one labeled CPU,
/// else the first running motherboard fan. A fan reading 0 RPM is an empty
/// header, so it is never picked automatically.
pub fn pick_fan(sensors: &[HwmonSensor]) -> Option<u32> {
    let fans = || {
        sensors
            .iter()
            .filter(|s| s.sensor.kind == SensorKind::Fan && s.sensor.value >= 1.0)
    };
    let labeled = |key: &str| fans().find(|s| s.sensor.name.to_ascii_lowercase().contains(key));
    labeled("pump")
        .or_else(|| labeled("cpu"))
        .or_else(|| fans().find(|s| s.sensor.hardware_type == "Motherboard"))
        .map(|s| s.sensor.value.round() as u32)
}

// -- CPU model name -----------------------------------------------------------

/// First `model name` of `/proc/cpuinfo` (x86), e.g. `AMD Ryzen 7 5800X`.
pub fn parse_cpu_model(cpuinfo: &str) -> Option<String> {
    cpuinfo.lines().find_map(|l| {
        let (key, val) = l.split_once(':')?;
        (key.trim() == "model name").then(|| val.trim().to_string())
    })
    .filter(|s| !s.is_empty())
}

// -- NVIDIA (nvidia-smi) --------------------------------------------------------

/// Parses `nvidia-smi --query-gpu=temperature.gpu,name --format=csv,noheader`
/// (one `45, NVIDIA GeForce RTX 3080` line per GPU).
pub fn parse_nvidia_smi(out: &str) -> Vec<SensorValue> {
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .filter_map(|(i, line)| {
            let (temp, name) = line.split_once(',')?;
            let value: f32 = temp.trim().parse().ok()?;
            if !(-50.0..=200.0).contains(&value) {
                return None;
            }
            Some(SensorValue {
                id: format!("nvidia-smi/{i}/temp1"),
                hardware: name.trim().to_string(),
                hardware_type: "GpuNvidia".into(),
                name: "GPU Core".into(),
                kind: SensorKind::Temp,
                value,
            })
        })
        .collect()
}

/// Whether an NVIDIA display device is powered up (or its power state is
/// unknown). A suspended GPU (hybrid graphics) must not be woken just to read
/// its temperature.
pub fn nvidia_gpu_awake(pci_root: &Path) -> bool {
    let mut devices = fs::read_dir(pci_root).into_iter().flatten().flatten();
    devices.any(|e| {
        let p = e.path();
        let is_nvidia_gpu = read_trim(p.join("vendor")).as_deref() == Some("0x10de")
            && read_trim(p.join("class")).is_some_and(|c| c.starts_with("0x03"));
        is_nvidia_gpu
            && !matches!(
                read_trim(p.join("power/runtime_status")).as_deref(),
                Some("suspended") | Some("suspending")
            )
    })
}

const NVIDIA_POLL: Duration = Duration::from_secs(5);
const NVIDIA_TIMEOUT: Duration = Duration::from_secs(3);

/// Runs `nvidia-smi` once. `Ok(None)` means it is not installed.
fn query_nvidia_smi() -> std::io::Result<Option<Vec<SensorValue>>> {
    use std::process::{Command, Stdio};
    let mut child = match Command::new("nvidia-smi")
        .args(["--query-gpu=temperature.gpu,name", "--format=csv,noheader"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let started = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if started.elapsed() > NVIDIA_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut text = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut text);
    }
    Ok(Some(if status.success() { parse_nvidia_smi(&text) } else { Vec::new() }))
}

/// Background poller of `nvidia-smi`; the latest values are read without blocking.
struct NvidiaSmi {
    latest: Arc<Mutex<Vec<SensorValue>>>,
}

impl NvidiaSmi {
    fn spawn() -> Self {
        let latest = Arc::new(Mutex::new(Vec::new()));
        let shared = latest.clone();
        let _ = std::thread::Builder::new().name("nvidia-smi".into()).spawn(move || {
            // Ends when the source is dropped or nvidia-smi does not exist.
            while Arc::strong_count(&shared) > 1 {
                let found = if nvidia_gpu_awake(Path::new("/sys/bus/pci/devices")) {
                    match query_nvidia_smi() {
                        Ok(Some(v)) => v,
                        Ok(None) => return,
                        Err(_) => Vec::new(),
                    }
                } else {
                    Vec::new()
                };
                if let Ok(mut g) = shared.lock() {
                    *g = found;
                }
                std::thread::sleep(NVIDIA_POLL);
            }
        });
        Self { latest }
    }

    fn latest(&self) -> Vec<SensorValue> {
        self.latest.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

// -- the Linux source -----------------------------------------------------------

/// Default source on Linux: CPU/RAM usage from `sysinfo`, temperatures and
/// fans from hwmon (plus NVIDIA GPUs through `nvidia-smi` when present).
pub struct LinuxSource {
    sys: SysinfoSource,
    root: PathBuf,
    cpu_model: Option<String>,
    nvidia: Option<NvidiaSmi>,
}

impl LinuxSource {
    pub fn new() -> Self {
        let cpu_model = fs::read_to_string("/proc/cpuinfo").ok().and_then(|s| parse_cpu_model(&s));
        let mut s = Self::with_root(HWMON_ROOT.into());
        s.cpu_model = cpu_model;
        s.nvidia = Some(NvidiaSmi::spawn());
        s
    }

    /// A source reading hwmon from `root` (no `nvidia-smi`), for tests.
    pub fn with_root(root: PathBuf) -> Self {
        Self { sys: SysinfoSource::new(), root, cpu_model: None, nvidia: None }
    }
}

impl Default for LinuxSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SensorSource for LinuxSource {
    fn read(&mut self) -> Result<Reading> {
        let mut r = self.sys.read()?;
        let found = read_hwmon(&self.root);
        // sysinfo's guess stays as the fallback when no hwmon CPU sensor exists.
        r.cpu_temp_c = pick_cpu_temp(&found).or(r.cpu_temp_c);
        r.fan_rpm = pick_fan(&found);
        r.sensors = found
            .into_iter()
            .map(|mut h| {
                if h.sensor.hardware_type == "Cpu" {
                    if let Some(m) = &self.cpu_model {
                        h.sensor.hardware = m.clone();
                    }
                }
                h.sensor
            })
            .collect();
        if let Some(n) = &self.nvidia {
            r.sensors.extend(n.latest());
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::naming::{classify, Category};

    /// A scratch directory removed on drop.
    struct Tmp(PathBuf);

    impl Tmp {
        fn new(name: &str) -> Self {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("kalkan-hwmon-{}-{n}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }

        fn put(&self, rel: &str, content: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn by_id<'a>(v: &'a [HwmonSensor], id: &str) -> &'a HwmonSensor {
        v.iter().find(|s| s.sensor.id == id).unwrap_or_else(|| panic!("missing {id}"))
    }

    fn fake_machine() -> Tmp {
        let t = Tmp::new("machine");
        // AMD CPU
        t.put("hwmon0/name", "k10temp\n");
        t.put("hwmon0/temp1_input", "61250\n");
        t.put("hwmon0/temp1_label", "Tctl\n");
        t.put("hwmon0/temp2_input", "55000\n");
        t.put("hwmon0/temp2_label", "Tccd1\n");
        t.put("hwmon0/temp1_max", "95000\n");
        t.put("hwmon0/temp1_crit", "100000\n");
        // NVMe (with a model string)
        t.put("hwmon1/name", "nvme\n");
        t.put("hwmon1/device/model", "Samsung SSD 980 PRO 1TB      \n");
        t.put("hwmon1/temp1_input", "38850\n");
        t.put("hwmon1/temp1_label", "Composite\n");
        t.put("hwmon1/temp1_alarm", "0\n");
        // AMD GPU
        t.put("hwmon2/name", "amdgpu\n");
        t.put("hwmon2/temp1_input", "47000\n");
        t.put("hwmon2/temp1_label", "edge\n");
        t.put("hwmon2/temp2_input", "52000\n");
        t.put("hwmon2/temp2_label", "junction\n");
        t.put("hwmon2/fan1_input", "0\n");
        // Super I/O: no labels, one empty header and one running fan
        t.put("hwmon3/name", "nct6798\n");
        t.put("hwmon3/temp1_input", "34000\n");
        t.put("hwmon3/fan1_input", "0\n");
        t.put("hwmon3/fan2_input", "1180\n");
        t.put("hwmon3/fan3_input", "n/a\n"); // unreadable: skipped
        t.put("hwmon3/pwm1", "128\n");
        // AIO
        t.put("hwmon4/name", "nzxt_kraken3\n");
        t.put("hwmon4/temp1_input", "31000\n");
        t.put("hwmon4/temp1_label", "Coolant temp\n");
        t.put("hwmon4/fan1_input", "2650\n");
        t.put("hwmon4/fan1_label", "Pump speed\n");
        // Noise that must be ignored
        t.put("hwmon5/temp1_input", "20000\n"); // no name file
        t.put("hwmonX/name", "bogus\n");
        t.put("hwmon6/name", "acpitz\n");
        t.put("hwmon6/temp1_input", "-273000\n"); // implausible
        t
    }

    #[test]
    fn reads_inputs_only_with_stable_ids() {
        let t = fake_machine();
        let v = read_hwmon(&t.0);
        let ids: Vec<&str> = v.iter().map(|s| s.sensor.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "hwmon/k10temp/hwmon0/temp1",
                "hwmon/k10temp/hwmon0/temp2",
                "hwmon/nvme/hwmon1/temp1",
                "hwmon/amdgpu/hwmon2/temp1",
                "hwmon/amdgpu/hwmon2/temp2",
                "hwmon/amdgpu/hwmon2/fan1",
                "hwmon/nct6798/hwmon3/temp1",
                "hwmon/nct6798/hwmon3/fan1",
                "hwmon/nct6798/hwmon3/fan2",
                "hwmon/nzxt_kraken3/hwmon4/temp1",
                "hwmon/nzxt_kraken3/hwmon4/fan1",
            ]
        );
        let tctl = by_id(&v, "hwmon/k10temp/hwmon0/temp1");
        assert_eq!(tctl.sensor.value, 61.25);
        assert_eq!(tctl.sensor.name, "Tctl");
        assert_eq!(by_id(&v, "hwmon/nvme/hwmon1/temp1").sensor.hardware, "Samsung SSD 980 PRO 1TB");
        assert_eq!(by_id(&v, "hwmon/k10temp/hwmon0/temp1").sensor.hardware, "AMD CPU");
    }

    #[test]
    fn categories_match_the_selector_groups() {
        let t = fake_machine();
        let v = read_hwmon(&t.0);
        let cat = |id: &str| classify(&by_id(&v, id).sensor);
        assert_eq!(cat("hwmon/k10temp/hwmon0/temp1"), Category::Cpu);
        assert_eq!(cat("hwmon/nvme/hwmon1/temp1"), Category::Storage);
        assert_eq!(cat("hwmon/amdgpu/hwmon2/temp2"), Category::Gpu);
        assert_eq!(cat("hwmon/nct6798/hwmon3/fan2"), Category::Motherboard);
        assert_eq!(cat("hwmon/nzxt_kraken3/hwmon4/fan1"), Category::LiquidPump);
        assert_eq!(cat("hwmon/nzxt_kraken3/hwmon4/temp1"), Category::LiquidPump);
    }

    #[test]
    fn chip_names_map_to_hardware_types() {
        for (chip, ty) in [
            ("k10temp", "Cpu"),
            ("zenpower", "Cpu"),
            ("coretemp", "Cpu"),
            ("amdgpu", "GpuAmd"),
            ("radeon", "GpuAmd"),
            ("nouveau", "GpuNvidia"),
            ("nvme", "Storage"),
            ("drivetemp", "Storage"),
            ("spd5118", "Memory"),
            ("jc42", "Memory"),
            ("nct6799", "Motherboard"),
            ("it8688", "Motherboard"),
            ("asusec", "Motherboard"),
            ("asus_wmi_sensors", "Motherboard"),
            ("gigabyte_wmi", "Motherboard"),
            ("nzxt_smart2", "Cooler"),
            ("corsaircpro", "Cooler"),
            ("aquacomputer_d5next", "Cooler"),
            ("asus_rog_ryujin", "Cooler"),
            ("corsairpsu", ""),
            ("mystery", ""),
        ] {
            assert_eq!(hardware_type(chip), ty, "{chip}");
        }
    }

    #[test]
    fn gpu_labels_feed_max_cpu_gpu() {
        let t = fake_machine();
        let v = read_hwmon(&t.0);
        assert_eq!(by_id(&v, "hwmon/amdgpu/hwmon2/temp1").sensor.name, "GPU Core");
        assert_eq!(by_id(&v, "hwmon/amdgpu/hwmon2/temp2").sensor.name, "GPU Hot Spot");
        let r = Reading { cpu_temp_c: Some(40.0), sensors: v.into_iter().map(|h| h.sensor).collect(), ..Default::default() };
        assert_eq!(crate::select_temp(&crate::TempSource::MaxCpuGpu, &r), Some(52.0));
    }

    #[test]
    fn auto_picks() {
        let t = fake_machine();
        let v = read_hwmon(&t.0);
        // Tctl wins over Tccd1; the pump wins over the motherboard fan.
        assert_eq!(pick_cpu_temp(&v), Some(61.25));
        assert_eq!(pick_fan(&v), Some(2650));
        // Without the AIO: the first running motherboard fan (fan1 reads 0).
        let no_aio: Vec<_> = v.into_iter().filter(|s| s.chip != "nzxt_kraken3").collect();
        assert_eq!(pick_fan(&no_aio), Some(1180));
    }

    #[test]
    fn cpu_temp_fallback_order() {
        let mk = |chip: &str, ty: &str, name: &str, value: f32| HwmonSensor {
            chip: chip.into(),
            sensor: SensorValue {
                id: format!("{chip}/{name}"),
                hardware: String::new(),
                hardware_type: ty.into(),
                name: name.into(),
                kind: SensorKind::Temp,
                value,
            },
        };
        // k10temp without Tctl uses Tdie.
        let v = [mk("k10temp", "Cpu", "Tccd1", 50.0), mk("k10temp", "Cpu", "Tdie", 44.0)];
        assert_eq!(pick_cpu_temp(&v), Some(44.0));
        // coretemp: the package, not core 0.
        let v = [mk("coretemp", "Cpu", "Core 0", 50.0), mk("coretemp", "Cpu", "Package id 0", 58.0)];
        assert_eq!(pick_cpu_temp(&v), Some(58.0));
        // zenpower Tdie.
        let v = [mk("zenpower", "Cpu", "Tctl", 70.0), mk("zenpower", "Cpu", "Tdie", 50.0)];
        assert_eq!(pick_cpu_temp(&v), Some(50.0));
        // Anything else CPU: the first one.
        let v = [mk("nvme", "Storage", "Composite", 30.0), mk("cpu_thermal", "Cpu", "CPU Temperature", 48.0)];
        assert_eq!(pick_cpu_temp(&v), Some(48.0));
        // No CPU sensor at all.
        assert_eq!(pick_cpu_temp(&v[..1]), None);
        assert_eq!(pick_fan(&[]), None);
    }

    #[test]
    fn labeled_cpu_fan_beats_motherboard_order() {
        let t = Tmp::new("cpufan");
        t.put("hwmon0/name", "nct6798\n");
        t.put("hwmon0/fan1_input", "900\n");
        t.put("hwmon0/fan2_input", "1300\n");
        t.put("hwmon0/fan2_label", "cpu_fan\n");
        assert_eq!(pick_fan(&read_hwmon(&t.0)), Some(1300));
    }

    #[test]
    fn missing_root_is_empty() {
        assert!(read_hwmon(Path::new("/definitely/not/here")).is_empty());
    }

    #[test]
    fn linux_source_reads_a_fake_tree() {
        let t = fake_machine();
        let mut src = LinuxSource::with_root(t.0.clone());
        let r = src.read().unwrap();
        assert_eq!(r.cpu_temp_c, Some(61.25));
        assert_eq!(r.fan_rpm, Some(2650));
        assert_eq!(r.sensors.len(), 11);
    }

    #[test]
    fn nvidia_smi_output() {
        let v = parse_nvidia_smi("45, NVIDIA GeForce RTX 3080\n61, NVIDIA GeForce RTX 3060, Ti\n[N/A], broken\n\n");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].id, "nvidia-smi/0/temp1");
        assert_eq!(v[0].value, 45.0);
        assert_eq!(v[0].hardware, "NVIDIA GeForce RTX 3080");
        assert_eq!(v[1].hardware, "NVIDIA GeForce RTX 3060, Ti");
        assert_eq!(classify(&v[0]), Category::Gpu);
    }

    #[test]
    fn nvidia_gpu_power_state() {
        let t = Tmp::new("pci");
        assert!(!nvidia_gpu_awake(&t.0)); // no NVIDIA device
        t.put("pci_01_0/vendor", "0x10de\n");
        t.put("pci_01_0/class", "0x030000\n");
        t.put("pci_01_0/power/runtime_status", "suspended\n");
        assert!(!nvidia_gpu_awake(&t.0));
        t.put("pci_01_0/power/runtime_status", "active\n");
        assert!(nvidia_gpu_awake(&t.0));
        // Other vendors and audio functions do not count.
        let u = Tmp::new("pci2");
        u.put("pci_02_0/vendor", "0x1002\n");
        u.put("pci_02_0/class", "0x030000\n");
        u.put("pci_01_1/vendor", "0x10de\n");
        u.put("pci_01_1/class", "0x040300\n");
        assert!(!nvidia_gpu_awake(&u.0));
    }

    #[test]
    fn cpu_model_from_cpuinfo() {
        let s = "processor\t: 0\nvendor_id\t: AuthenticAMD\nmodel name\t: AMD Ryzen 7 5800X 8-Core Processor\n";
        assert_eq!(parse_cpu_model(s).as_deref(), Some("AMD Ryzen 7 5800X 8-Core Processor"));
        assert_eq!(parse_cpu_model("processor : 0\n"), None);
    }
}
