//! System tray icon (Windows/macOS).
//!
//! Closing the window only hides it; the app keeps sending to the display from
//! the tray. Left click (or "Open Kalkan Monitor") brings the window back;
//! "Close monitor" quits for real.

use anyhow::Result;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// Creates the tray icon. `show` brings the window back; it is called from
/// the event loop thread. On macOS this must run after the event loop started.
pub fn create(show: impl Fn() + Clone + Send + Sync + 'static) -> Result<TrayIcon> {
    let open = MenuItem::new("Open Kalkan Monitor", true, None);
    let quit = MenuItem::new("Close monitor", true, None);
    let menu = Menu::new();
    menu.append(&open)?;
    menu.append(&quit)?;

    let (open_id, quit_id) = (open.id().clone(), quit.id().clone());
    let show_menu = show.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == open_id {
            show_menu();
        } else if e.id == quit_id {
            let _ = slint::quit_event_loop();
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = e
        {
            show();
        }
    }));

    Ok(TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("Kalkan Monitor")
        .with_icon(icon()?)
        .build()?)
}

/// 32×32 icon drawn in code (accent disc with a white thermometer), so there
/// is no image asset to ship.
fn icon() -> Result<Icon> {
    const N: usize = 32;
    let mut rgba = vec![0u8; N * N * 4];
    let c = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32, y as f32);
            let disc = ((fx - c).powi(2) + (fy - c).powi(2)).sqrt() <= 15.5;
            // thermometer: vertical stem + bulb
            let stem = (fx - c).abs() <= 2.5 && (6.0..=20.0).contains(&fy);
            let bulb = ((fx - c).powi(2) + (fy - 22.0).powi(2)).sqrt() <= 5.0;
            let px = match (disc, stem || bulb) {
                (true, true) => [0xff, 0xff, 0xff, 0xff],
                (true, false) => [0x2f, 0x6d, 0xf6, 0xff], // Palette.accent
                _ => [0, 0, 0, 0],
            };
            rgba[(y * N + x) * 4..][..4].copy_from_slice(&px);
        }
    }
    Ok(Icon::from_rgba(rgba, N as u32, N as u32)?)
}
