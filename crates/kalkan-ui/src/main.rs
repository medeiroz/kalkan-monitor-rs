//! Kalkan Monitor — app de interface (shell com sidebar, tema claro).
//!
//! UI nova, desacoplada do núcleo: consome `kalkan-core` (sensores + protocolo)
//! e mostra CPU/temperatura/fan em tempo real, além de deixar o usuário escolher
//! qual fonte alimenta cada um dos 3 campos do display do cooler, com prévia.
//!
//! A matemática de clamp/arredondamento da prévia vive aqui (não no Slint) para
//! bater exatamente com o que `kalkan-core` serializa no frame.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use kalkan_core::{available_serial_ports, Reading, SensorSource, SysinfoSource};
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

fn main() -> anyhow::Result<()> {
    let ui = AppWindow::new()?;

    // Atalho de teste: KALKAN_PAGE=0|1|2 abre direto numa tela.
    if let Ok(p) = std::env::var("KALKAN_PAGE") {
        ui.set_active_page(p.parse().unwrap_or(0));
    }

    // Modelos das fontes de cada campo (indisponíveis ficam marcadas).
    ui.set_temp_sources(model(&["CPU (die)", "GPU · sem backend", "Água/líquido · sem backend"]));
    ui.set_fan_sources(model(&["Bomba · sem backend", "Fan CPU · sem backend", "Fan gabinete · sem backend"]));
    ui.set_load_sources(model(&["Uso de CPU", "Uso de RAM", "GPU · sem backend"]));

    // Opções de frequência de atualização (índice → intervalo).
    ui.set_interval_options(model(&["0,5 s", "1 s", "2 s", "5 s"]));
    ui.set_interval_index(1); // 1 s por padrão

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
    let source = Rc::new(RefCell::new(SysinfoSource::new()));
    let _ = source.borrow_mut().read();

    // Timer reiniciável: `restart_timer(ms)` (re)agenda o laço com o intervalo dado.
    let timer = Rc::new(Timer::default());
    let restart_timer: Rc<dyn Fn(u64)> = {
        let (timer, ui_w, source, mapping, last, frame) =
            (timer.clone(), ui.as_weak(), source.clone(), mapping.clone(), last.clone(), frame.clone());
        Rc::new(move |interval_ms: u64| {
            let (ui_w, source, mapping, last, frame) =
                (ui_w.clone(), source.clone(), mapping.clone(), last.clone(), frame.clone());
            timer.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(interval_ms),
                move || {
                    let Ok(reading) = source.borrow_mut().read() else { return };
                    *last.borrow_mut() = reading.clone();
                    let fi = { let mut f = frame.borrow_mut(); *f = f.wrapping_add(1); *f };
                    if let Some(ui) = ui_w.upgrade() {
                        update_dashboard(&ui, &reading);
                        let sending = ui.get_conn_state() == ConnState::Connected;
                        update_display(&ui, *mapping.borrow(), &reading, fi, sending);
                    }
                },
            );
        })
    };

    // Callback: usuário mudou a frequência → reinicia o timer com o novo intervalo.
    {
        let restart_timer = restart_timer.clone();
        ui.on_interval_changed(move |idx| {
            let ms = match idx {
                0 => 500,
                1 => 1000,
                2 => 2000,
                _ => 5000,
            };
            restart_timer(ms);
        });
    }

    restart_timer(1000); // intervalo inicial (1 s, igual ao interval-index padrão)

    ui.run()?;
    Ok(())
}
