//! Kalkan Monitor — app de interface (shell com sidebar, tema claro).
//!
//! UI nova, desacoplada do núcleo: consome `kalkan-core` (sensores + protocolo)
//! e mostra CPU/temperatura/fan em tempo real, além de deixar o usuário escolher
//! qual fonte alimenta cada um dos 3 campos do display do cooler, com prévia.
//!
//! A matemática de clamp/arredondamento da prévia vive aqui (não no Slint) para
//! bater exatamente com o que `kalkan-core` serializa no frame.

// Em release, app de janela (sem console atrás); em debug o console fica, para logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use kalkan_core::{
    available_serial_ports, build_frame, default_source, open_display, scramble, Reading,
    SerialTransport, Transport,
};
use slint::{ModelRc, SharedString, Timer, VecModel};

slint::include_modules!();

/// Mapa das fontes escolhidas para cada campo do display (índice no ComboBox).
#[derive(Clone, Copy)]
struct Mapping {
    temp: usize,
    fan: usize,
    load: usize,
}

impl Default for Mapping {
    fn default() -> Self {
        Self { temp: 0, fan: 0, load: 0 }
    }
}

fn model(items: &[&str]) -> ModelRc<SharedString> {
    let v: Vec<SharedString> = items.iter().map(|s| (*s).into()).collect();
    ModelRc::new(VecModel::from(v))
}

// -- seleção de fonte por campo (índice → valor da leitura) -------------------

fn temp_value(r: &Reading, idx: usize) -> Option<f32> {
    match idx {
        0 => r.cpu_temp_c, // CPU (die)
        _ => None,         // GPU / água: sem backend
    }
}

fn fan_value(r: &Reading, _idx: usize) -> Option<u32> {
    r.fan_rpm // nenhuma fonte de fan disponível ainda (todas caem aqui)
}

fn load_value(r: &Reading, idx: usize) -> Option<f32> {
    match idx {
        0 => Some(r.cpu_load), // Uso de CPU
        1 => Some(r.ram_load), // Uso de RAM
        _ => None,             // GPU: sem backend
    }
}

// -- níveis (normal / alerta / crítico) para o dashboard ----------------------

fn level(value: f32, warn: f32, crit: f32) -> i32 {
    if value >= crit { 2 } else if value >= warn { 1 } else { 0 }
}

fn update_dashboard(ui: &AppWindow, r: &Reading) {
    // CPU
    ui.set_cpu_load(r.cpu_load);
    ui.set_cpu_load_str(format!("{:.0}", r.cpu_load).into());
    let cl = level(r.cpu_load, 70.0, 90.0);
    ui.set_cpu_level(cl);
    ui.set_cpu_state(match cl { 2 => "Crítico", 1 => "Alto", _ => "Normal" }.into());

    // Temperatura
    match r.cpu_temp_c {
        Some(t) => {
            ui.set_cpu_temp_str(format!("{t:.0}").into());
            ui.set_temp_fraction((t / 100.0).clamp(0.0, 1.0));
            let tl = level(t, 60.0, 80.0);
            ui.set_temp_level(tl);
            ui.set_temp_state(match tl { 2 => "Crítico", 1 => "Quente", _ => "Normal" }.into());
        }
        None => {
            ui.set_cpu_temp_str("--".into());
            ui.set_temp_fraction(0.0);
            ui.set_temp_level(0);
            ui.set_temp_state("Sem leitura".into());
        }
    }

    // Fan
    match r.fan_rpm {
        Some(f) => {
            ui.set_fan_str(format!("{f}").into());
            ui.set_fan_fraction((f as f32 / 3000.0).clamp(0.0, 1.0));
            ui.set_fan_state("Normal".into());
        }
        None => {
            ui.set_fan_str("--".into());
            ui.set_fan_fraction(0.0);
            ui.set_fan_state("Sem sensor nesta plataforma".into());
        }
    }
}

fn update_display(ui: &AppWindow, m: Mapping, r: &Reading, frame_index: i32, sending: bool) {
    // --- campo temperatura (byte 0..255) ---
    match temp_value(r, m.temp) {
        Some(t) => {
            let out = t.round().clamp(0.0, 255.0) as i32;
            let sat = t > 255.0 || t < 0.0;
            ui.set_prev_temp(format!("{out}").into());
            ui.set_temp_sends(
                if sat { format!("Envia: {out} (saturado)") } else { format!("Envia: {out} (0–255)") }.into(),
            );
            ui.set_temp_sends_warn(sat);
        }
        None => {
            ui.set_prev_temp("--".into());
            ui.set_temp_sends("Envia: 0 · sem fonte nesta plataforma".into());
            ui.set_temp_sends_warn(true);
        }
    }

    // --- campo fan (2 bytes, 0..65535) ---
    match fan_value(r, m.fan) {
        Some(f) => {
            let out = f.min(0xFFFF) as i32;
            let sat = f > 0xFFFF;
            ui.set_prev_fan(format!("{out}").into());
            ui.set_fan_sends(
                if sat { format!("Envia: {out} (saturado)") } else { format!("Envia: {out} (0–65535)") }.into(),
            );
            ui.set_fan_sends_warn(sat);
        }
        None => {
            ui.set_prev_fan("--".into());
            ui.set_fan_sends("Envia: 0 · sem fonte nesta plataforma".into());
            ui.set_fan_sends_warn(true);
        }
    }

    // --- campo uso % (byte, clamp 0..100) ---
    match load_value(r, m.load) {
        Some(l) => {
            let out = l.round().clamp(0.0, 100.0) as i32;
            ui.set_prev_load(format!("{out}").into());
            ui.set_load_sends(format!("Envia: {out} (0–100)").into());
            ui.set_load_sends_warn(false);
        }
        None => {
            ui.set_prev_load("--".into());
            ui.set_load_sends("Envia: 0 · sem fonte nesta plataforma".into());
            ui.set_load_sends_warn(true);
        }
    }

    ui.set_prev_frame_index(frame_index);
    ui.set_prev_sending(sending);
}

/// Monta o frame 1.3 com os mesmos valores da prévia (`update_display`).
fn display_frame(m: Mapping, r: &Reading) -> Vec<u8> {
    let temp = temp_value(r, m.temp).map(|t| t.round().clamp(0.0, 255.0) as u8).unwrap_or(0);
    let fan = fan_value(r, m.fan).map(|f| f.min(0xFFFF) as u16).unwrap_or(0);
    scramble(&build_frame(temp, fan))
}

/// Envia um frame ao display, (re)abrindo a porta quando preciso.
///
/// Sem porta aberta, tenta `open_display()` a cada tick (como o original, que
/// reconecta sozinho quando o cooler é plugado). Erro de escrita derruba a
/// conexão; o próximo tick tenta de novo.
fn send_frame(ui: &AppWindow, port: &mut Option<SerialTransport>, pkt: &[u8]) {
    if port.is_none() {
        match open_display() {
            Ok(t) => {
                *port = Some(t);
                ui.set_hid_error("".into());
            }
            Err(e) => {
                ui.set_conn_state(ConnState::Disconnected);
                ui.set_hid_error(format!("{e}").into());
                return;
            }
        }
    }
    let Some(t) = port.as_mut() else { return };
    match t.write(pkt) {
        Ok(_) => ui.set_conn_state(ConnState::Connected),
        Err(e) => {
            *port = None;
            ui.set_conn_state(ConnState::Error);
            ui.set_hid_error(format!("falha ao enviar: {e}").into());
        }
    }
}

/// No Windows, garante que o app rode elevado (como o original): se não estiver,
/// relança a si mesmo via UAC e encerra a instância atual. O processo elevado é
/// o que consegue carregar o driver de kernel do sidecar de sensores. Se o
/// usuário recusar o UAC, segue sem elevação (sensores caem no fallback).
/// Defina `KALKAN_NO_ELEVATE=1` para desabilitar (desenvolvimento).
#[cfg(windows)]
fn ensure_elevated() {
    use core::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    const TOKEN_QUERY: u32 = 0x0008;
    const TOKEN_ELEVATION: i32 = 20;
    const SW_SHOWNORMAL: i32 = 1;

    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> i32;
        fn GetTokenInformation(
            token: *mut c_void,
            class: i32,
            info: *mut c_void,
            len: u32,
            ret_len: *mut u32,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn CloseHandle(h: *mut c_void) -> i32;
    }
    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: *mut c_void,
            op: *const u16,
            file: *const u16,
            params: *const u16,
            dir: *const u16,
            show: i32,
        ) -> *mut c_void;
    }

    // SAFETY: chamadas FFI padrão; tratamos retornos e fechamos o token.
    unsafe {
        let mut token: *mut c_void = core::ptr::null_mut();
        let mut elevated = false;
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) != 0 {
            let mut elev: u32 = 0;
            let mut ret_len: u32 = 0;
            if GetTokenInformation(
                token,
                TOKEN_ELEVATION,
                &mut elev as *mut u32 as *mut c_void,
                4,
                &mut ret_len,
            ) != 0
            {
                elevated = elev != 0;
            }
            CloseHandle(token);
        }
        if elevated || std::env::var_os("KALKAN_NO_ELEVATE").is_some() {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let mut file: Vec<u16> = exe.as_os_str().encode_wide().collect();
        file.push(0);
        let mut op: Vec<u16> = "runas".encode_utf16().collect();
        op.push(0);
        let r = ShellExecuteW(
            core::ptr::null_mut(),
            op.as_ptr(),
            file.as_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            SW_SHOWNORMAL,
        );
        // ShellExecuteW retorna > 32 em sucesso: a instância elevada assumiu.
        if (r as isize) > 32 {
            std::process::exit(0);
        }
        // Recusado/falhou: segue sem elevação.
    }
}

#[cfg(not(windows))]
fn ensure_elevated() {}

fn main() -> anyhow::Result<()> {
    ensure_elevated();
    let ui = AppWindow::new()?;

    // Atalho de teste: KALKAN_PAGE=0|1|2 abre direto numa tela.
    if let Ok(p) = std::env::var("KALKAN_PAGE") {
        ui.set_active_page(p.parse().unwrap_or(0));
    }

    // Modelos das fontes de cada campo (indisponíveis ficam marcadas).
    ui.set_temp_sources(model(&["CPU (die)", "GPU · sem backend", "Água/líquido · sem backend"]));
    ui.set_fan_sources(model(&["Bomba · sem backend", "Fan CPU · sem backend", "Fan gabinete · sem backend"]));
    ui.set_load_sources(model(&["Uso de CPU", "Uso de RAM", "GPU · sem backend"]));

    // Descoberta best-effort do display 1.3 nas portas seriais (VID/PID).
    match available_serial_ports() {
        Ok(ports) => {
            if let Some(d) = ports.iter().find(|p| p.is_display()) {
                ui.set_device_label(
                    d.product.clone().unwrap_or_else(|| "Kalkan SmartMonitor 1.3".into()).into(),
                );
                ui.set_device_detail(
                    format!(
                        "{}  ·  VID {:#06x}  ·  PID {:#06x}  ·  serial",
                        d.port_name,
                        d.vendor_id.unwrap_or(0),
                        d.product_id.unwrap_or(0),
                    )
                    .into(),
                );
                ui.set_conn_state(ConnState::Disconnected); // só Connected após abrir a porta
            } else {
                ui.set_device_label("nenhum display reconhecido".into());
                ui.set_device_detail("plugue o cooler e rode `kalkan list`".into());
                ui.set_conn_state(ConnState::Disconnected);
            }
        }
        Err(e) => {
            ui.set_device_label("serial indisponível".into());
            ui.set_hid_error(format!("serial indisponível: {e}").into());
            ui.set_conn_state(ConnState::Error);
        }
    }

    // Estado compartilhado entre o timer e o callback de troca de slot.
    let mapping = Rc::new(RefCell::new(Mapping::default()));
    let last = Rc::new(RefCell::new(Reading::default()));
    let frame = Rc::new(RefCell::new(0i32));

    // Callback: usuário trocou a fonte de um campo → recalcula a prévia na hora.
    {
        let (ui_w, map, lastr, framr) = (ui.as_weak(), mapping.clone(), last.clone(), frame.clone());
        ui.on_slot_changed(move |slot, index| {
            {
                let mut m = map.borrow_mut();
                match slot {
                    0 => m.temp = index as usize,
                    1 => m.fan = index as usize,
                    _ => m.load = index as usize,
                }
            }
            if let Some(ui) = ui_w.upgrade() {
                let sending = ui.get_conn_state() == ConnState::Connected;
                update_display(&ui, *map.borrow(), &lastr.borrow(), *framr.borrow(), sending);
            }
        });
    }

    // Primeira amostra (descartada — sysinfo precisa de duas para o %CPU).
    let source = Rc::new(RefCell::new(default_source()));
    let _ = source.borrow_mut().read();

    // Intervalo fixo de 1 s, igual ao app original (sem seletor de frequência:
    // a cadência PC → display é sempre a mesma).
    let timer = Timer::default();
    {
        let (ui_w, source, mapping, last, frame) =
            (ui.as_weak(), source.clone(), mapping.clone(), last.clone(), frame.clone());
        let mut port: Option<SerialTransport> = None;
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(1000),
            move || {
                let Ok(reading) = source.borrow_mut().read() else { return };
                *last.borrow_mut() = reading.clone();
                let fi = { let mut f = frame.borrow_mut(); *f = f.wrapping_add(1); *f };
                if let Some(ui) = ui_w.upgrade() {
                    send_frame(&ui, &mut port, &display_frame(*mapping.borrow(), &reading));
                    update_dashboard(&ui, &reading);
                    let sending = ui.get_conn_state() == ConnState::Connected;
                    update_display(&ui, *mapping.borrow(), &reading, fi, sending);
                }
            },
        );
    }

    ui.run()?;
    Ok(())
}
