//! Descoberta de dispositivos — portas seriais (1.3) e HID (legado/diagnóstico).
//!
//! A 1.3 fala por **porta serial USB**. O app original acha o cooler
//! enumerando `QSerialPortInfo::availablePorts()` e casando por VID/PID
//! (`FUN_1400083d0`), tentando dois pares conhecidos (ver [`DISPLAY_IDS`]).

use anyhow::Result;
use serialport::SerialPortType;

/// Pares (VID, PID) do display 1.3, na ordem em que o original os tenta.
///
/// De `Sysctl::connect` (`FUN_140008300`): primeiro `1F3A:0008`, senão
/// `1A86:484A` (família CH340 — ponte USB-serial comum).
pub const DISPLAY_IDS: &[(u16, u16)] = &[(0x1F3A, 0x0008), (0x1A86, 0x484A)];

/// Uma porta serial vista na enumeração do sistema (multiplataforma).
#[derive(Debug, Clone)]
pub struct SerialDeviceInfo {
    /// Nome da porta (ex.: `COM5` no Windows, `/dev/ttyUSB0` no Linux).
    pub port_name: String,
    /// VID/PID quando a porta é USB (serialport só expõe para portas USB).
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
}

impl SerialDeviceInfo {
    /// `true` se o VID/PID casa com algum par de [`DISPLAY_IDS`].
    pub fn is_display(&self) -> bool {
        match (self.vendor_id, self.product_id) {
            (Some(v), Some(p)) => DISPLAY_IDS.contains(&(v, p)),
            _ => false,
        }
    }
}

/// Lista todas as portas seriais do sistema, com VID/PID quando USB.
pub fn available_serial_ports() -> Result<Vec<SerialDeviceInfo>> {
    let ports = serialport::available_ports()?;
    Ok(ports
        .into_iter()
        .map(|p| {
            let (vendor_id, product_id, manufacturer, product, serial_number) = match p.port_type {
                SerialPortType::UsbPort(info) => (
                    Some(info.vid),
                    Some(info.pid),
                    info.manufacturer,
                    info.product,
                    info.serial_number,
                ),
                _ => (None, None, None, None, None),
            };
            SerialDeviceInfo {
                port_name: p.port_name,
                vendor_id,
                product_id,
                manufacturer,
                product,
                serial_number,
            }
        })
        .collect())
}

/// `true` se a porta de nome `port_name` casa com [`DISPLAY_IDS`].
pub fn is_display_port(info: &SerialDeviceInfo) -> bool {
    info.is_display()
}

/// Acha o nome da primeira porta serial cujo VID/PID casa com [`DISPLAY_IDS`].
///
/// Respeita a ordem de [`DISPLAY_IDS`] (igual ao original: tenta `1F3A:0008`
/// antes de `1A86:484A`).
pub fn find_display_port() -> Result<Option<String>> {
    let ports = available_serial_ports()?;
    for &(vid, pid) in DISPLAY_IDS {
        if let Some(p) = ports
            .iter()
            .find(|p| p.vendor_id == Some(vid) && p.product_id == Some(pid))
        {
            return Ok(Some(p.port_name.clone()));
        }
    }
    Ok(None)
}

// -- HID (legado 1.2 / diagnóstico) ------------------------------------------

/// Um dispositivo HID visto na enumeração do sistema (multiplataforma).
#[derive(Debug, Clone)]
pub struct HidDeviceInfo {
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
    pub path: String,
}

/// Lista todos os dispositivos HID do sistema.
///
/// A 1.3 não usa HID (o transporte é serial); fica como ferramenta de
/// diagnóstico, útil ao inspecionar hardware desconhecido.
pub fn enumerate() -> Result<Vec<HidDeviceInfo>> {
    let api = hidapi::HidApi::new()?;
    Ok(api
        .device_list()
        .map(|d| HidDeviceInfo {
            vendor_id: d.vendor_id(),
            product_id: d.product_id(),
            manufacturer: d.manufacturer_string().map(str::to_owned),
            product: d.product_string().map(str::to_owned),
            serial_number: d.serial_number().map(str::to_owned),
            path: d.path().to_string_lossy().into_owned(),
        })
        .collect())
}

/// Heurística para destacar candidatos a "display de cooler" numa lista HID.
pub fn looks_like_display(info: &HidDeviceInfo) -> bool {
    let hay = format!(
        "{} {}",
        info.manufacturer.as_deref().unwrap_or(""),
        info.product.as_deref().unwrap_or("")
    )
    .to_lowercase();
    ["lcd", "screen", "display", "monitor", "vtf", "com"]
        .iter()
        .any(|k| hay.contains(k))
}
