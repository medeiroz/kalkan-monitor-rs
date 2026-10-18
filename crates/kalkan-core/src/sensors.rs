//! Fontes de sensores.
//!
//! O original lê CPU/temperatura/fan do **HWiNFO** (via `HWiNFO64.dll`), que só
//! existe no Windows. Para ser multiplataforma trocamos essa fonte por `sysinfo`
//! (CPU load e, onde disponível, temperaturas/fans) e deixamos o trait aberto
//! para fontes específicas por SO (lm-sensors no Linux, IOKit/SMC no macOS,
//! LibreHardwareMonitor no Windows) adicionadas depois.

use anyhow::Result;

/// Uma leitura instantânea do que o display costuma mostrar.
#[derive(Debug, Clone, Default)]
pub struct Reading {
    /// Uso de CPU em porcentagem (0..=100).
    pub cpu_load: f32,
    /// Uso de memória RAM em porcentagem (0..=100).
    pub ram_load: f32,
    /// Temperatura da CPU em °C, quando a plataforma expõe.
    pub cpu_temp_c: Option<f32>,
    /// Rotação de fan/bomba em RPM, quando disponível.
    pub fan_rpm: Option<u32>,
}

/// Qualquer coisa capaz de produzir uma `Reading`.
pub trait SensorSource: Send {
    fn read(&mut self) -> Result<Reading>;
}

/// Fonte multiplataforma baseada em `sysinfo`.
///
/// Entrega **uso de CPU** em qualquer SO e **temperatura de CPU** onde o
/// `sysinfo` expõe componentes térmicos (no macOS Apple Silicon são os
/// sensores `PMU tdie*`; no Linux, `coretemp`/`k10temp`; etc.). A rotação de
/// fan **não** vem do `sysinfo` — precisa de backend específico (SMC no macOS,
/// lm-sensors no Linux), então fica `None` aqui.
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

    /// Estima a temperatura de CPU a partir dos componentes térmicos.
    ///
    /// Seleciona sensores cujo rótulo sugere die/CPU e cuja leitura é
    /// plausível (0..150 °C — descarta os sensores desconectados que o macOS
    /// reporta como ~-9199 °C) e tira a média.
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

        // Nenhum rótulo bateu: cai para a média de qualquer sensor plausível.
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

impl SensorSource for SysinfoSource {
    fn read(&mut self) -> Result<Reading> {
        self.sys.refresh_cpu_usage();
        let cpu_load = self.sys.global_cpu_usage();

        self.sys.refresh_memory();
        let total = self.sys.total_memory();
        let ram_load = if total > 0 {
            self.sys.used_memory() as f32 / total as f32 * 100.0
        } else {
            0.0
        };

        self.components.refresh(false);
        let cpu_temp_c = self.cpu_temperature();

        Ok(Reading {
            cpu_load,
            ram_load,
            cpu_temp_c,
            // Fan: sysinfo não expõe RPM; no Windows vem do HWiNFO (ver abaixo).
            fan_rpm: None,
        })
    }
}

/// Fonte padrão da plataforma.
///
/// No Windows usa [`WindowsSource`] (CPU/RAM do `sysinfo` + temperatura e fan do
/// HWiNFO); nas demais plataformas, [`SysinfoSource`].
pub fn default_source() -> Box<dyn SensorSource> {
    #[cfg(windows)]
    {
        Box::new(WindowsSource::new())
    }
    #[cfg(not(windows))]
    {
        Box::new(SysinfoSource::new())
    }
}

// ============================================================================
// Backend do Windows — HWiNFO Shared Memory (`Global\HWiNFO_SENS_SM2`)
// ============================================================================
//
// Mesma fonte que o app original (HwiHandler → HWiNFO64.dll). O HWiNFO, com
// "Shared Memory Support" ligado, publica um bloco de memória compartilhada com
// um cabeçalho e N "reading elements". Cada elemento tem um tipo (1=temp,
// 3=fan), rótulos e um `double` com o valor. Layout do SDK oficial
// (`HWiNFO_SENSORS_SHARED_MEM2` / `HWiNFO_SENSORS_READING_ELEMENT`).
//
// Leitura não exige administrador; exige o HWiNFO rodando com SHM habilitado.

/// Valores extraídos de uma amostra do HWiNFO.
#[derive(Debug, Default, Clone, Copy)]
pub struct HwinfoSnapshot {
    pub temp_c: Option<f32>,
    pub fan_rpm: Option<u32>,
}

const HWINFO_SIG: &[u8; 4] = b"SiWH";
const READING_TYPE_TEMP: u32 = 1;
const READING_TYPE_FAN: u32 = 3;

// Offsets no cabeçalho.
const H_OFF_READING_SECTION: usize = 32;
const H_OFF_READING_SIZE: usize = 36;
const H_OFF_READING_COUNT: usize = 40;
const HEADER_MIN_LEN: usize = 44;

// Offsets dentro de um reading element.
const R_OFF_TYPE: usize = 0;
const R_OFF_LABEL_USER: usize = 140; // após tReading/index/id (12) + szLabelOrig (128)
const R_OFF_VALUE: usize = 284; // após szLabelUser (128) + szUnit (16)
const R_MIN_LEN: usize = 292;

fn rd_u32(buf: &[u8], off: usize) -> Option<u32> {
    buf.get(off..off + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn rd_f64(buf: &[u8], off: usize) -> Option<f64> {
    buf.get(off..off + 8).map(|b| {
        f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
    })
}

/// Lê um rótulo C (terminado em NUL) como string (bytes >=0x20, latin-1 simples).
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

/// Parser **puro** do bloco de SHM do HWiNFO (sem chamadas de SO — testável).
///
/// Escolhe:
///   * **temperatura**: rótulo com `cpu/tctl/tdie/core/package` e valor
///     plausível (0..=150 °C); senão a primeira temperatura plausível.
///   * **fan**: rótulo com `pump/cpu/aio/water/liquid`; senão o primeiro fan > 0.
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
    // Teto de leitura: o bloco real do HWiNFO tem ~alguns MB no máximo.
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

    /// Lê uma amostra do HWiNFO, ou `None` se a SHM não estiver disponível.
    pub fn read() -> Option<HwinfoSnapshot> {
        let mut name: Vec<u16> = SHM_NAME.encode_utf16().collect();
        name.push(0);

        // SAFETY: nomes válidos; tratamos ponteiros nulos; a view é somente
        // leitura e desmapeada/fechada antes de retornar.
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
            // Mapeamos a view inteira (0 = mapping todo). Não sabemos o tamanho
            // exato aqui; lemos o cabeçalho e deduzimos o fim da seção de
            // readings, limitado por MAX_MAP_BYTES.
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
// Backend do Windows — motor HWiNFO embutido (`config.dat` = `HWiNFO64.dll`)
// ============================================================================
//
// O que o app original faz (`RunHwinfo`, `FUN_140002d80`): `LoadLibraryA(
// "./config.dat")` — que é a DLL do SDK do HWiNFO renomeada, com driver próprio
// compatível com HVCI — e resolve as funções **por ordinal**:
//
//   590 Init(flags=0x40) -> int (0 = ok)      883 NumSensors() -> int
//   726 Update()                               749 SelectSensor(int)
//   106 GetReading(type, sensor, index, *Raw) -> bool
//
// O laço de leitura (`FUN_1400027e0`) percorre sensor × índice (0..0x200) ×
// tipo ∈ {1 temp, 3 fan}. Exige administrador (o driver é carregado na hora).

/// Uma leitura do motor HWiNFO (rótulo + valor), para diagnóstico/escolha.
#[derive(Debug, Clone)]
pub struct HwinfoReading {
    pub kind: u32,
    pub sensor: i32,
    pub index: i32,
    pub sensor_name: String,
    pub label: String,
    pub value: f64,
}

#[cfg(windows)]
pub mod hwinfo_dll {
    use super::{HwinfoReading, HwinfoSnapshot, READING_TYPE_FAN, READING_TYPE_TEMP};
    use anyhow::{anyhow, Result};
    use core::ffi::c_void;
    use std::path::PathBuf;

    const ORD_INIT: usize = 590;
    const ORD_NUM_SENSORS: usize = 883;
    const ORD_UPDATE: usize = 726;
    const ORD_SELECT_SENSOR: usize = 749;
    const ORD_GET_READING: usize = 106;

    /// Índices máximos por sensor que o original varre (`0x200`).
    const MAX_INDEX: i32 = 0x200;

    /// Struct de saída de `GetReading` — 0x1D0 bytes (copiada inteira no original).
    const RAW_LEN: usize = 0x1D0;
    const RAW_OFF_VALUE: usize = 0x08; // f64
    const RAW_OFF_SENSOR_NAME: usize = 0x3C; // char[256]
    const RAW_OFF_LABEL: usize = 0x148; // char[128] — onde o original busca "package"/"cpu"
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

    /// Onde procurar a DLL, em ordem: `KALKAN_HWINFO_DLL`; `config.dat` /
    /// `HWiNFO64.dll` ao lado do executável; a instalação do app original.
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

    /// Motor HWiNFO carregado e inicializado. (A DLL fica carregada até o fim
    /// do processo, como no original.)
    pub struct Hwinfo {
        update: FnUpdate,
        num_sensors: FnNumSensors,
        select_sensor: FnSelectSensor,
        get_reading: FnGetReading,
        pub path: PathBuf,
    }

    // SAFETY: ponteiros de função para uma DLL que nunca é descarregada; o uso
    // fica serializado pelo `&mut self` do chamador.
    unsafe impl Send for Hwinfo {}

    impl Hwinfo {
        /// Carrega a primeira DLL encontrada e chama `Init(0x40)`.
        pub fn load() -> Result<Self> {
            let path = candidates()
                .into_iter()
                .find(|p| p.is_file())
                .ok_or_else(|| anyhow!("DLL do HWiNFO (config.dat) não encontrada"))?;
            let mut wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
            wide.push(0);

            // SAFETY: caminho terminado em NUL; ordinais conferidos contra o
            // binário original; checamos cada ponteiro antes do transmute.
            unsafe {
                let module = LoadLibraryW(wide.as_ptr());
                if module.is_null() {
                    return Err(anyhow!("falha ao carregar {}", path.display()));
                }
                let proc = |ord: usize| -> Result<*mut c_void> {
                    let p = GetProcAddress(module, ord as *const u8);
                    if p.is_null() {
                        Err(anyhow!("ordinal {ord} ausente em {}", path.display()))
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
                        "Init do HWiNFO falhou ({rc}) — o app precisa rodar como administrador"
                    ));
                }
                (hw.update)();
                Ok(hw)
            }
        }

        /// Atualiza e lista todas as leituras de temperatura e fan.
        pub fn readings(&mut self) -> Vec<HwinfoReading> {
            let mut out = Vec::new();
            let mut raw = vec![0u8; RAW_LEN];
            // SAFETY: `raw` tem o tamanho que o original reserva para a struct.
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

        /// Temperatura e fan da CPU, com a mesma escolha do original
        /// (`FUN_1400023b0`): temp com "package"/"average" no rótulo; fan com
        /// "cpu". Sem casamento, cai na primeira leitura plausível.
        pub fn snapshot(&mut self) -> HwinfoSnapshot {
            let all = self.readings();
            let (t, f) = pick(&all);
            HwinfoSnapshot {
                temp_c: t.map(|r| r.value as f32),
                fan_rpm: f.map(|r| r.value.round() as u32),
            }
        }

        /// Relê só uma leitura já conhecida (após `update`).
        fn value_of(&mut self, key: (u32, i32, i32)) -> Option<f64> {
            let (kind, sensor, index) = key;
            let mut raw = vec![0u8; RAW_LEN];
            // SAFETY: idem `readings`.
            unsafe {
                (self.select_sensor)(sensor);
                if !(self.get_reading)(kind, sensor, index, raw.as_mut_ptr()) || raw[0] == 0 {
                    return None;
                }
            }
            Some(f64::from_le_bytes(raw[RAW_OFF_VALUE..RAW_OFF_VALUE + 8].try_into().unwrap()))
        }
    }

    /// Fonte de temp/fan sobre o motor HWiNFO: varre tudo uma vez para escolher
    /// as leituras (como o original no `RunHwinfo`) e depois relê só essas duas.
    pub struct HwinfoSource {
        hw: Hwinfo,
        temp_key: Option<(u32, i32, i32)>,
        fan_key: Option<(u32, i32, i32)>,
    }

    impl HwinfoSource {
        pub fn new() -> Result<Self> {
            let mut hw = Hwinfo::load()?;
            let all = hw.readings();
            let (t, f) = pick(&all);
            let key = |r: &HwinfoReading| (r.kind, r.sensor, r.index);
            Ok(Self {
                temp_key: t.map(key),
                fan_key: f.map(key),
                hw,
            })
        }

        pub fn read(&mut self) -> HwinfoSnapshot {
            // SAFETY: ponteiro de função válido enquanto a DLL estiver carregada.
            unsafe { (self.hw.update)() };
            let temp = self.temp_key.and_then(|k| self.hw.value_of(k));
            let fan = self.fan_key.and_then(|k| self.hw.value_of(k));
            HwinfoSnapshot {
                temp_c: temp.filter(|v| (0.0..=150.0).contains(v)).map(|v| v as f32),
                fan_rpm: fan.filter(|v| *v >= 0.0 && *v < 100_000.0).map(|v| v.round() as u32),
            }
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
// Backend autossuficiente — sidecar LibreHardwareMonitor (`kalkan-sensord.exe`)
// ============================================================================
//
// Caminho principal (self-contained, como o original): um processo auxiliar
// .NET que carrega o LibreHardwareMonitor (motor + driver de kernel próprio) e
// emite leituras como JSON por linha no stdout. Lemos essa saída. Fica ao lado
// do executável (ou via env KALKAN_SENSORD). Quando o sidecar não está presente
// ou não entrega dados, caímos no HWiNFO SHM e, por fim, em `None`.

/// Extrai `(temp_c, fan_rpm)` de uma linha JSON do sidecar:
/// `{"cpu_temp_c":42.0,"fan_rpm":2400}` (valores podem ser `null`).
///
/// Retorna `None` se a linha não for um objeto com a chave esperada.
pub(crate) fn parse_sensor_line(line: &str) -> Option<(Option<f32>, Option<u32>)> {
    let line = line.trim();
    if !line.starts_with('{') || !line.contains("cpu_temp_c") {
        return None;
    }
    let temp = json_num(line, "\"cpu_temp_c\"").map(|v| v as f32);
    let fan = json_num(line, "\"fan_rpm\"").map(|v| v.round() as u32);
    Some((temp, fan))
}

/// Lê o número associado a `key` num JSON simples e plano; `null` vira `None`.
fn json_num(s: &str, key: &str) -> Option<f64> {
    let after_key = &s[s.find(key)? + key.len()..];
    let after_colon = &after_key[after_key.find(':')? + 1..];
    let v = after_colon.trim_start();
    let end = v.find([',', '}']).unwrap_or(v.len());
    let tok = v[..end].trim();
    if tok.eq_ignore_ascii_case("null") {
        return None;
    }
    tok.parse::<f64>().ok()
}

#[cfg(windows)]
mod sidecar {
    use super::parse_sensor_line;
    use std::io::{BufRead, BufReader};
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[derive(Default, Clone, Copy)]
    struct Shared {
        temp_c: Option<f32>,
        fan_rpm: Option<u32>,
        at: Option<Instant>,
    }

    /// Processo sidecar em execução; encerrado no `Drop`.
    pub struct Sidecar {
        child: Child,
        state: Arc<Mutex<Shared>>,
    }

    /// Resolve o caminho do `kalkan-sensord.exe`: env `KALKAN_SENSORD`, ao lado
    /// do executável, ou caminhos de desenvolvimento no repositório.
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
        /// Spawna o sidecar e inicia a thread leitora; `None` se não existir.
        pub fn spawn() -> Option<Self> {
            use std::os::windows::process::CommandExt;
            // Sem isto, o Windows abre uma janela de console para o sidecar
            // quando o pai é um app de janela (a UI).
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;

            let path = resolve_path()?;
            let mut child = Command::new(&path)
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok()?;
            let stdout = child.stdout.take()?;
            let state = Arc::new(Mutex::new(Shared::default()));
            let st = state.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if let Some((t, f)) = parse_sensor_line(&line) {
                        if let Ok(mut s) = st.lock() {
                            s.temp_c = t;
                            s.fan_rpm = f;
                            s.at = Some(Instant::now());
                        }
                    }
                }
            });
            Some(Self { child, state })
        }

        /// Última leitura, se recente (≤ 5 s); senão `(None, None)`.
        pub fn latest(&self) -> (Option<f32>, Option<u32>) {
            if let Ok(s) = self.state.lock() {
                if matches!(s.at, Some(at) if at.elapsed() < Duration::from_secs(5)) {
                    return (s.temp_c, s.fan_rpm);
                }
            }
            (None, None)
        }
    }

    impl Drop for Sidecar {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Fonte de sensores do Windows.
///
/// CPU/RAM load vêm do `sysinfo`. Temperatura e fan vêm, em ordem: do **sidecar
/// LibreHardwareMonitor** (`kalkan-sensord.exe`, com o driver PawnIO — funciona
/// sob HVCI); senão do **motor HWiNFO do app original** (`config.dat`, se
/// instalado); senão do **HWiNFO Shared Memory** (se o HWiNFO estiver rodando);
/// senão `None`. Ver `docs/sensors-windows.md`.
#[cfg(windows)]
pub struct WindowsSource {
    sys: SysinfoSource,
    sidecar: Option<sidecar::Sidecar>,
    started: std::time::Instant,
    hwinfo: Option<hwinfo_dll::HwinfoSource>,
    hwinfo_tried: bool,
}

/// Quanto esperar o sidecar entregar a primeira leitura antes de recorrer ao
/// motor HWiNFO (o LibreHardwareMonitor leva alguns segundos para abrir).
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
        let mut r = self.sys.read()?;

        let (mut temp, mut fan) = match &self.sidecar {
            Some(sc) => sc.latest(),
            None => (None, None),
        };

        // Sidecar ausente, ou sem dados após a carência (ex.: HVCI sem PawnIO):
        // tenta uma vez o motor HWiNFO do app original.
        let sidecar_silent = self.sidecar.is_none() || self.started.elapsed() > SIDECAR_GRACE;
        if temp.is_none() && fan.is_none() && sidecar_silent && !self.hwinfo_tried {
            self.hwinfo_tried = true;
            self.hwinfo = hwinfo_dll::HwinfoSource::new().ok();
            if self.hwinfo.is_some() {
                self.sidecar = None; // encerra o sidecar inútil
            }
        }
        if let Some(hw) = &mut self.hwinfo {
            let s = hw.read();
            temp = temp.or(s.temp_c);
            fan = fan.or(s.fan_rpm);
        }

        // Fallback HWiNFO Shared Memory para o que ainda faltar.
        if temp.is_none() || fan.is_none() {
            if let Some(hw) = hwinfo_shm::read() {
                temp = temp.or(hw.temp_c);
                fan = fan.or(hw.fan_rpm);
            }
        }

        if let Some(t) = temp {
            r.cpu_temp_c = Some(t);
        }
        if let Some(f) = fan {
            r.fan_rpm = Some(f);
        }
        Ok(r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monta um bloco de SHM sintético (cabeçalho + N readings) para o parser.
    fn synth(readings: &[(u32, &str, f64)]) -> Vec<u8> {
        let size = R_MIN_LEN; // tamanho do elemento neste teste
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
            (3, "unit?", -1.0), // fan inválido (<=0) ignorado
        ]);
        let s = parse_hwinfo(&buf);
        assert_eq!(s.temp_c, Some(42.0)); // preferiu o rótulo de CPU
        assert_eq!(s.fan_rpm, Some(2400)); // preferiu "Pump"
    }

    /// Regra do original (`FUN_1400023b0`), com rótulos reais de um Ryzen 9800X3D
    /// + Nuvoton NCT6799D lidos pelo motor HWiNFO.
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
        assert_eq!(s.temp_c, Some(50.0)); // sem rótulo de CPU → primeira temp
        assert_eq!(s.fan_rpm, Some(1000)); // sem pump/cpu → primeiro fan
    }

    #[test]
    fn hwinfo_rejects_bad_signature() {
        let mut buf = synth(&[(READING_TYPE_TEMP, "CPU", 40.0)]);
        buf[0] = b'X';
        let s = parse_hwinfo(&buf);
        assert_eq!(s.temp_c, None);
        assert_eq!(s.fan_rpm, None);
    }

    #[test]
    fn sidecar_line_parse_values() {
        assert_eq!(
            parse_sensor_line("{\"cpu_temp_c\":42.5,\"fan_rpm\":2400}"),
            Some((Some(42.5), Some(2400)))
        );
    }

    #[test]
    fn sidecar_line_parse_nulls() {
        assert_eq!(
            parse_sensor_line("{\"cpu_temp_c\":null,\"fan_rpm\":null}"),
            Some((None, None))
        );
    }

    #[test]
    fn sidecar_line_parse_partial_and_junk() {
        // só temperatura
        assert_eq!(
            parse_sensor_line("{\"cpu_temp_c\":40,\"fan_rpm\":null}"),
            Some((Some(40.0), None))
        );
        // linha inválida
        assert_eq!(parse_sensor_line("iniciando..."), None);
        assert_eq!(parse_sensor_line("{\"outra\":1}"), None);
    }
}
