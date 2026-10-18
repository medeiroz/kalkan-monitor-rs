//! Kalkan Monitor — UI app (sidebar shell, light theme).
//!
//! New UI, decoupled from the core: consumes `kalkan-core` (sensors + protocol)
//! and shows CPU/temperature/fan in real time, and lets the user choose which
//! source feeds each of the 3 fields of the cooler's display, with a preview.
//!
//! The preview's clamp/rounding math lives here (not in Slint) so it matches
//! exactly what `kalkan-core` serializes into the frame.

// In release, a windowed app (no console behind it); in debug the console stays, for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use kalkan_core::{
    available_serial_ports, build_frame, default_source, fan_options, open_display, scramble,
    select_fan, select_temp, temp_options, FanSource, Reading, SerialTransport, Settings,
    SensorValue, SourceOption, TempSource, Transport,
};
use slint::{ModelRc, SharedString, Timer, VecModel};

slint::include_modules!();

/// Map of the sources chosen for each display field. Temperature and fan hold
/// the actual choice (a sensor id survives the sensor list changing); the
/// usage field is still an index into its ComboBox.
#[derive(Clone, Default)]
struct Mapping {
    temp: TempSource,
    fan: FanSource,
    load: usize,
}

/// Options currently shown in the temperature and fan selectors.
struct Options {
    temp: Vec<SourceOption<TempSource>>,
    fan: Vec<SourceOption<FanSource>>,
    /// Nothing pushed to the selectors yet.
    first: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { temp: Vec::new(), fan: Vec::new(), first: true }
    }
}

fn labels<S>(opts: &[SourceOption<S>]) -> Vec<SharedString> {
    opts.iter().map(|o| SharedString::from(o.label.as_str())).collect()
}

/// Pushes the current options into the selectors, keeping each selection.
///
/// A model is only replaced when its labels changed, so the popup is not
/// rebuilt every second. The selected index follows the chosen source (a
/// sensor id), not a position that shifts as sensors appear.
fn refresh_options(ui: &AppWindow, m: &Mapping, opts: &mut Options, sensors: &[SensorValue]) {
    let temp = temp_options(sensors, &m.temp);
    let fan = fan_options(sensors, &m.fan);
    let (tl, fl) = (labels(&temp), labels(&fan));
    if opts.first || tl != labels(&opts.temp) {
        ui.set_temp_sources(ModelRc::new(VecModel::from(tl)));
    }
    if opts.first || fl != labels(&opts.fan) {
        ui.set_fan_sources(ModelRc::new(VecModel::from(fl)));
    }
    ui.set_temp_source_index(temp.iter().position(|o| o.source == m.temp).unwrap_or(0) as i32);
    ui.set_fan_source_index(fan.iter().position(|o| o.source == m.fan).unwrap_or(0) as i32);
    opts.temp = temp;
    opts.fan = fan;
    opts.first = false;
}

fn model(items: &[&str]) -> ModelRc<SharedString> {
    let v: Vec<SharedString> = items.iter().map(|s| (*s).into()).collect();
    ModelRc::new(VecModel::from(v))
}

// -- source selection per field (index → reading value) -----------------------

fn temp_value(r: &Reading, src: &TempSource) -> Option<f32> {
    select_temp(src, r)
}

fn fan_value(r: &Reading, src: &FanSource) -> Option<u32> {
    select_fan(src, r)
}

fn load_value(r: &Reading, idx: usize) -> Option<f32> {
    match idx {
        0 => Some(r.cpu_load), // CPU usage
        1 => Some(r.ram_load), // RAM usage
        _ => None,             // GPU: no backend
    }
}

// -- levels (normal / warning / critical) for the dashboard -------------------

fn level(value: f32, warn: f32, crit: f32) -> i32 {
    if value >= crit { 2 } else if value >= warn { 1 } else { 0 }
}

fn update_dashboard(ui: &AppWindow, m: &Mapping, r: &Reading) {
    // CPU
    ui.set_cpu_load(r.cpu_load);
    ui.set_cpu_load_str(format!("{:.0}", r.cpu_load).into());
    let cl = level(r.cpu_load, 70.0, 90.0);
    ui.set_cpu_level(cl);
    ui.set_cpu_state(match cl { 2 => "Critical", 1 => "High", _ => "Normal" }.into());

    // Temperature (the source chosen for the display)
    match temp_value(r, &m.temp) {
        Some(t) => {
            ui.set_cpu_temp_str(format!("{t:.0}").into());
            ui.set_temp_fraction((t / 100.0).clamp(0.0, 1.0));
            let tl = level(t, 60.0, 80.0);
            ui.set_temp_level(tl);
            ui.set_temp_state(match tl { 2 => "Critical", 1 => "Hot", _ => "Normal" }.into());
        }
        None => {
            ui.set_cpu_temp_str("--".into());
            ui.set_temp_fraction(0.0);
            ui.set_temp_level(0);
            ui.set_temp_state("No reading".into());
        }
    }

    // Fan (the source chosen for the display)
    match fan_value(r, &m.fan) {
        Some(f) => {
            ui.set_fan_str(format!("{f}").into());
            ui.set_fan_fraction((f as f32 / 3000.0).clamp(0.0, 1.0));
            ui.set_fan_state("Normal".into());
        }
        None => {
            ui.set_fan_str("--".into());
            ui.set_fan_fraction(0.0);
            ui.set_fan_state("No sensor on this platform".into());
        }
    }
}

fn update_display(ui: &AppWindow, m: &Mapping, r: &Reading, frame_index: i32, sending: bool) {
    // --- temperature field (byte 0..255) ---
    match temp_value(r, &m.temp) {
        Some(t) => {
            let out = t.round().clamp(0.0, 255.0) as i32;
            let sat = t > 255.0 || t < 0.0;
            ui.set_prev_temp(format!("{out}").into());
            ui.set_temp_sends(
                if sat { format!("Sends: {out} (saturated)") } else { format!("Sends: {out} (0–255)") }.into(),
            );
            ui.set_temp_sends_warn(sat);
        }
        None => {
            ui.set_prev_temp("--".into());
            ui.set_temp_sends("Sends: 0 · no source on this platform".into());
            ui.set_temp_sends_warn(true);
        }
    }

    // --- fan field (2 bytes, 0..65535) ---
    match fan_value(r, &m.fan) {
        Some(f) => {
            let out = f.min(0xFFFF) as i32;
            let sat = f > 0xFFFF;
            ui.set_prev_fan(format!("{out}").into());
            ui.set_fan_sends(
                if sat { format!("Sends: {out} (saturated)") } else { format!("Sends: {out} (0–65535)") }.into(),
            );
            ui.set_fan_sends_warn(sat);
        }
        None => {
            ui.set_prev_fan("--".into());
            ui.set_fan_sends("Sends: 0 · no source on this platform".into());
            ui.set_fan_sends_warn(true);
        }
    }

    // --- usage % field (byte, clamp 0..100) ---
    match load_value(r, m.load) {
        Some(l) => {
            let out = l.round().clamp(0.0, 100.0) as i32;
            ui.set_prev_load(format!("{out}").into());
            ui.set_load_sends(format!("Sends: {out} (0–100)").into());
            ui.set_load_sends_warn(false);
        }
        None => {
            ui.set_prev_load("--".into());
            ui.set_load_sends("Sends: 0 · no source on this platform".into());
            ui.set_load_sends_warn(true);
        }
    }

    ui.set_prev_frame_index(frame_index);
    ui.set_prev_sending(sending);
}

/// Builds the 1.3 frame with the same values as the preview (`update_display`).
fn display_frame(m: &Mapping, r: &Reading) -> Vec<u8> {
    let temp = temp_value(r, &m.temp).map(|t| t.round().clamp(0.0, 255.0) as u8).unwrap_or(0);
    let fan = fan_value(r, &m.fan).map(|f| f.min(0xFFFF) as u16).unwrap_or(0);
    scramble(&build_frame(temp, fan))
}

/// Sends a frame to the display, (re)opening the port when needed.
///
/// With no open port, tries `open_display()` on every tick (like the original,
/// which reconnects by itself when the cooler is plugged in). A write error
/// drops the connection; the next tick tries again.
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
            ui.set_hid_error(format!("failed to send: {e}").into());
        }
    }
}

/// On Windows, makes sure the app runs elevated (like the original): if it is
/// not, relaunches itself via UAC and exits the current instance. The elevated
/// process is the one able to load the sensor sidecar's kernel driver. If the
/// user declines the UAC prompt, it carries on without elevation (sensors fall
/// back). Set `KALKAN_NO_ELEVATE=1` to disable (development).
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

    // SAFETY: standard FFI calls; we handle return values and close the token.
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
        // ShellExecuteW returns > 32 on success: the elevated instance took over.
        if (r as isize) > 32 {
            std::process::exit(0);
        }
        // Declined/failed: carry on without elevation.
    }
}

#[cfg(not(windows))]
fn ensure_elevated() {}

fn main() -> anyhow::Result<()> {
    // winit needs this thread to be an STA (OleInitialize) when it creates the
    // window, but sysinfo's WMI access (inside `default_source()`) would
    // initialize COM as MTA first and make that fail. Claim STA up front; the
    // later sysinfo attempt then fails gracefully (temps come from the sidecar
    // or HWiNFO on Windows anyway).
    #[cfg(windows)]
    {
        #[link(name = "ole32")]
        extern "system" {
            fn OleInitialize(reserved: *mut core::ffi::c_void) -> i32;
        }
        // SAFETY: plain FFI call with the required null argument; the result
        // is ignored on purpose (S_FALSE just means already initialized).
        unsafe {
            OleInitialize(core::ptr::null_mut());
        }
    }

    ensure_elevated();
    let ui = AppWindow::new()?;

    // Test shortcut: KALKAN_PAGE=0|1|2 opens straight on a screen.
    if let Ok(p) = std::env::var("KALKAN_PAGE") {
        ui.set_active_page(p.parse().unwrap_or(0));
    }

    // The temperature and fan lists are filled from the sensors as they appear
    // (see `refresh_options`); usage is a fixed list.
    ui.set_load_sources(model(&["CPU usage", "RAM usage", "GPU · no backend"]));

    // Best-effort discovery of the 1.3 display on the serial ports (VID/PID).
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
                ui.set_conn_state(ConnState::Disconnected); // Connected only after the port is opened
            } else {
                ui.set_device_label("no display recognized".into());
                ui.set_device_detail("plug in the cooler and run `kalkan list`".into());
                ui.set_conn_state(ConnState::Disconnected);
            }
        }
        Err(e) => {
            ui.set_device_label("serial unavailable".into());
            ui.set_hid_error(format!("serial unavailable: {e}").into());
            ui.set_conn_state(ConnState::Error);
        }
    }

    // State shared between the timer and the slot-change callback.
    // The saved choices (Auto/Auto when there is no usable settings file).
    let saved = Settings::load();
    let mapping = Rc::new(RefCell::new(Mapping {
        temp: saved.temp_source,
        fan: saved.fan_source,
        load: 0,
    }));
    let options = Rc::new(RefCell::new(Options::default()));
    refresh_options(&ui, &mapping.borrow(), &mut options.borrow_mut(), &[]);
    let last = Rc::new(RefCell::new(Reading::default()));
    let frame = Rc::new(RefCell::new(0i32));

    // Callback: the user changed a field's source → recompute the preview right away.
    {
        let (ui_w, map, lastr, framr, opts) =
            (ui.as_weak(), mapping.clone(), last.clone(), frame.clone(), options.clone());
        ui.on_slot_changed(move |slot, index| {
            let index = index.max(0) as usize;
            {
                let mut m = map.borrow_mut();
                let o = opts.borrow();
                match slot {
                    0 => {
                        if let Some(x) = o.temp.get(index) {
                            m.temp = x.source.clone();
                        }
                    }
                    1 => {
                        if let Some(x) = o.fan.get(index) {
                            m.fan = x.source.clone();
                        }
                    }
                    _ => m.load = index,
                }
                if slot < 2 {
                    let s = Settings { temp_source: m.temp.clone(), fan_source: m.fan.clone() };
                    if let Err(e) = s.save() {
                        eprintln!("could not save settings: {e}");
                    }
                }
            }
            if let Some(ui) = ui_w.upgrade() {
                let sending = ui.get_conn_state() == ConnState::Connected;
                update_display(&ui, &map.borrow(), &lastr.borrow(), *framr.borrow(), sending);
                update_dashboard(&ui, &map.borrow(), &lastr.borrow());
            }
        });
    }

    // First sample (discarded — sysinfo needs two for the CPU %).
    let source = Rc::new(RefCell::new(default_source()));
    let _ = source.borrow_mut().read();

    // Fixed 1 s interval, same as the original app (no frequency selector:
    // the PC → display cadence is always the same).
    let timer = Timer::default();
    {
        let (ui_w, source, mapping, last, frame, options) = (
            ui.as_weak(),
            source.clone(),
            mapping.clone(),
            last.clone(),
            frame.clone(),
            options.clone(),
        );
        let mut port: Option<SerialTransport> = None;
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(1000),
            move || {
                let Ok(reading) = source.borrow_mut().read() else { return };
                *last.borrow_mut() = reading.clone();
                let fi = { let mut f = frame.borrow_mut(); *f = f.wrapping_add(1); *f };
                if let Some(ui) = ui_w.upgrade() {
                    refresh_options(&ui, &mapping.borrow(), &mut options.borrow_mut(), &reading.sensors);
                    send_frame(&ui, &mut port, &display_frame(&mapping.borrow(), &reading));
                    update_dashboard(&ui, &mapping.borrow(), &reading);
                    let sending = ui.get_conn_state() == ConnState::Connected;
                    update_display(&ui, &mapping.borrow(), &reading, fi, sending);
                }
            },
        );
    }

    ui.run()?;
    Ok(())
}
