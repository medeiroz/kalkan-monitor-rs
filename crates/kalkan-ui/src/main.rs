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
    select_fan, select_temp, temp_options, FanSource, MaxTracker, Reading, SerialTransport, Settings,
    same_layout, SourceOption, TempSource, Transport,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, VecModel};

mod autostart;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod instance;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod tray;

slint::include_modules!();

/// Map of the sources chosen for each display field. Temperature and fan hold
/// the actual choice (a sensor id survives the sensor list changing); the
/// usage field is still an index into its ComboBox.
#[derive(Clone, Default)]
struct Mapping {
    temp: TempSource,
    fan: FanSource,
    load: usize,
    /// Memory of Max CPU/GPU between readings (see `MaxTracker`).
    max: MaxTracker,
}

/// Options currently shown in the temperature and fan selectors, with the
/// models bound to the UI.
struct Options {
    temp: Vec<SourceOption<TempSource>>,
    fan: Vec<SourceOption<FanSource>>,
    temp_model: Rc<VecModel<SourceItem>>,
    fan_model: Rc<VecModel<SourceItem>>,
    /// Nothing pushed to the selectors yet.
    first: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            temp: Vec::new(),
            fan: Vec::new(),
            temp_model: Rc::new(VecModel::default()),
            fan_model: Rc::new(VecModel::default()),
            first: true,
        }
    }
}

/// UI rows for a list of options; a group header is flagged on the first row
/// of each group.
fn rows<S>(opts: &[SourceOption<S>]) -> Vec<SourceItem> {
    let mut prev = "";
    opts.iter()
        .map(|o| {
            let header = !o.group.is_empty() && o.group != prev;
            prev = &o.group;
            SourceItem {
                primary: o.primary.as_str().into(),
                secondary: o.secondary.as_str().into(),
                group: o.group.as_str().into(),
                value: o.value_text.as_str().into(),
                header,
            }
        })
        .collect()
}

/// Brings a model in line with new options. When only the live values changed
/// the rows are updated in place, so an open popup keeps its scroll position
/// and stays open; the model is only rebuilt when the set of entries changes.
fn sync_model<S: PartialEq>(
    model: &VecModel<SourceItem>,
    old: &[SourceOption<S>],
    new: &[SourceOption<S>],
    rebuild: bool,
) {
    let new_rows = rows(new);
    if rebuild || !same_layout(old, new) {
        model.set_vec(new_rows);
        return;
    }
    for (i, (o, n)) in old.iter().zip(new).enumerate() {
        if o.value_text != n.value_text {
            model.set_row_data(i, new_rows[i].clone());
        }
    }
}

/// Pushes the current options into the selectors, keeping each selection.
///
/// The selected index follows the chosen source (a sensor id), not a position
/// that shifts as sensors appear.
fn refresh_options(ui: &AppWindow, m: &Mapping, opts: &mut Options, r: &Reading) {
    let temp = temp_options(r, &m.temp, &m.max);
    let fan = fan_options(r, &m.fan);
    if opts.first {
        ui.set_temp_sources(opts.temp_model.clone().into());
        ui.set_fan_sources(opts.fan_model.clone().into());
    }
    sync_model(&opts.temp_model, &opts.temp, &temp, opts.first);
    sync_model(&opts.fan_model, &opts.fan, &fan, opts.first);
    ui.set_temp_source_index(temp.iter().position(|o| o.source == m.temp).unwrap_or(0) as i32);
    ui.set_fan_source_index(fan.iter().position(|o| o.source == m.fan).unwrap_or(0) as i32);
    opts.temp = temp;
    opts.fan = fan;
    opts.first = false;
}

/// A plain list of entries without secondary text (the usage selector).
fn plain_model(items: &[&str]) -> ModelRc<SourceItem> {
    let v: Vec<SourceItem> = items
        .iter()
        .map(|s| SourceItem {
            primary: (*s).into(),
            secondary: SharedString::default(),
            group: SharedString::default(),
            value: SharedString::default(),
            header: false,
        })
        .collect();
    ModelRc::new(VecModel::from(v))
}

// -- source selection per field (index → reading value) -----------------------

fn temp_value(r: &Reading, src: &TempSource, max: &MaxTracker) -> Option<f32> {
    select_temp(src, r, max)
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
    match temp_value(r, &m.temp, &m.max) {
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
    match temp_value(r, &m.temp, &m.max) {
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
    let temp = temp_value(r, &m.temp, &m.max).map(|t| t.round().clamp(0.0, 255.0) as u8).unwrap_or(0);
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
            ui.set_hid_error(format!("Failed to send: {e}").into());
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
        // Forward the arguments (e.g. `--tray`) to the elevated instance.
        let args: Vec<String> = std::env::args().skip(1).map(|a| format!("\"{a}\"")).collect();
        let mut params: Vec<u16> = args.join(" ").encode_utf16().collect();
        params.push(0);
        let r = ShellExecuteW(
            core::ptr::null_mut(),
            op.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
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

    // Already running (e.g. hidden in the tray)? Bring it up instead. Checked
    // before elevating, so reopening never shows a UAC prompt.
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    if instance::signal_existing() {
        return Ok(());
    }

    ensure_elevated();
    let start_hidden = std::env::args().any(|a| a == autostart::TRAY_ARG);
    let ui = AppWindow::new()?;

    // Brings the window back (from the tray or from a second instance); safe
    // to call from any thread.
    let show = {
        let ui_w = ui.as_weak();
        move || {
            let _ = ui_w.upgrade_in_event_loop(|ui| {
                let _ = ui.show();
                ui.window().set_minimized(false);
                // A background app keeps its window behind the others otherwise.
                #[cfg(target_os = "macos")]
                macos::activate();
            });
        }
    };
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    instance::listen(show.clone());

    // "Start with the operating system" (reflects the real OS registration).
    ui.set_autostart(autostart::is_enabled());
    {
        let ui_w = ui.as_weak();
        ui.on_autostart_toggled(move |on| {
            let Some(ui) = ui_w.upgrade() else { return };
            match autostart::set_enabled(on) {
                Ok(()) => ui.set_autostart_error("".into()),
                Err(e) => ui.set_autostart_error(e.to_string().into()),
            }
            ui.set_autostart(autostart::is_enabled());
        });
    }

    // Test shortcut: KALKAN_PAGE=0|1|2 opens straight on a screen.
    if let Ok(p) = std::env::var("KALKAN_PAGE") {
        ui.set_active_page(p.parse().unwrap_or(0));
    }

    // The temperature and fan lists are filled from the sensors as they appear
    // (see `refresh_options`); usage is a fixed list.
    ui.set_load_sources(plain_model(&["CPU usage", "RAM usage", "GPU · no backend"]));

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
                ui.set_device_label("No display recognized".into());
                ui.set_device_detail("Plug in the cooler and run `kalkan list`".into());
                ui.set_conn_state(ConnState::Disconnected);
            }
        }
        Err(e) => {
            ui.set_device_label("Serial unavailable".into());
            ui.set_hid_error(format!("Serial unavailable: {e}").into());
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
        max: MaxTracker::default(),
    }));
    let options = Rc::new(RefCell::new(Options::default()));
    refresh_options(&ui, &mapping.borrow(), &mut options.borrow_mut(), &Reading::default());
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
                mapping.borrow_mut().max.update(&reading);
                let fi = { let mut f = frame.borrow_mut(); *f = f.wrapping_add(1); *f };
                if let Some(ui) = ui_w.upgrade() {
                    refresh_options(&ui, &mapping.borrow(), &mut options.borrow_mut(), &reading);
                    send_frame(&ui, &mut port, &display_frame(&mapping.borrow(), &reading));
                    update_dashboard(&ui, &mapping.borrow(), &reading);
                    let sending = ui.get_conn_state() == ConnState::Connected;
                    update_display(&ui, &mapping.borrow(), &reading, fi, sending);
                }
            },
        );
    }

    // Whether a tray icon can be shown at all: always on Windows/macOS; on
    // Linux only when the desktop has a StatusNotifier host (stock GNOME does
    // not). When started at logon, wait a few seconds for the panel to come up.
    #[cfg(target_os = "linux")]
    let tray_available = tray::host_available(start_hidden);
    #[cfg(any(windows, target_os = "macos"))]
    let tray_available = true;
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    let tray_available = false;

    // Tray icon: created once the event loop runs (required on macOS). The
    // close button then hides the window instead of quitting; "Close monitor"
    // in the tray menu quits. Without a tray (no StatusNotifier host on Linux,
    // or if creating it fails) closing the window quits as usual and the
    // window is shown even with `--tray`.
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    let tray_icon: Rc<RefCell<Option<tray_icon::TrayIcon>>> = Rc::new(RefCell::new(None));
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    if tray_available {
        let (holder, ui_w, show) = (tray_icon.clone(), ui.as_weak(), show.clone());
        Timer::single_shot(Duration::ZERO, move || match tray::create(show) {
            Ok(t) => *holder.borrow_mut() = Some(t),
            Err(e) => {
                eprintln!("tray icon unavailable: {e}");
                if let Some(ui) = ui_w.upgrade() {
                    let _ = ui.show();
                }
            }
        });
    }
    // macOS: a click on the Dock icon shows the hidden window again. Needs the
    // application delegate, which exists once the event loop runs.
    #[cfg(target_os = "macos")]
    {
        let show = show.clone();
        Timer::single_shot(Duration::ZERO, move || {
            if !macos::install_reopen_handler(show) {
                eprintln!("Dock icon reopen hook unavailable; use the menu-bar icon");
            }
        });
    }
    {
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        let has_tray = {
            let t = tray_icon.clone();
            move || t.borrow().is_some()
        };
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        let has_tray = || false;
        ui.window().on_close_requested(move || {
            if !has_tray() {
                let _ = slint::quit_event_loop();
            }
            slint::CloseRequestResponse::HideWindow
        });
    }

    // Started at logon: stay in the tray (only where there is one).
    if !(start_hidden && tray_available) {
        ui.show()?;
    }
    slint::run_event_loop_until_quit()?;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    instance::release();
    let _ = show; // only used by the tray/instance hooks on some platforms
    Ok(())
}
