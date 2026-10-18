//! System tray icon (Windows, macOS and Linux).
//!
//! Closing the window only hides it; the app keeps sending to the display from
//! the tray. Left click (or "Open Kalkan Monitor") brings the window back;
//! "Close monitor" quits for real.
//!
//! On Linux the icon is a StatusNotifierItem published over D-Bus (the `ksni`
//! backend of `tray-icon`): no GTK or appindicator libraries are needed, but
//! the desktop must provide a StatusNotifier host (see [`host_available`]).

use anyhow::Result;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
#[cfg(not(target_os = "macos"))]
use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

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
            // The tray may call from another thread (StatusNotifier on Linux).
            let _ = slint::invoke_from_event_loop(|| {
                let _ = slint::quit_event_loop();
            });
        }
    }));
    // On macOS a click opens the menu instead (see `with_menu_on_left_click` below).
    #[cfg(not(target_os = "macos"))]
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

    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        // Menu-bar convention on macOS: a click opens the menu. Elsewhere a left
        // click reopens the window and the menu is on the right click.
        .with_menu_on_left_click(cfg!(target_os = "macos"))
        .with_tooltip("Kalkan Monitor");
    // macOS: a monochrome template image, which the system recolors for the
    // light or dark menu bar. Elsewhere the colored icon.
    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_templated(menu_bar_icon()?);
    #[cfg(not(target_os = "macos"))]
    let builder = builder.with_icon(icon()?);
    // StatusNotifier hosts show the title next to (or instead of) the tooltip.
    #[cfg(target_os = "linux")]
    let builder = builder.with_title("Kalkan Monitor");
    Ok(builder.build()?)
}

/// 32×32 icon drawn in code (accent disc with a white thermometer), so there
/// is no image asset to ship.
#[cfg_attr(target_os = "macos", allow(dead_code))]
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

/// Monochrome thermometer for the macOS menu bar: black with an alpha
/// channel, 64×64 (the menu bar scales it to its height). Used as a template
/// image, so only the alpha matters; the system colors it for light and dark
/// menu bars. Drawn with signed distances for smooth edges.
fn menu_bar_rgba() -> Vec<u8> {
    const N: usize = 64;
    // Distance from (px, py) to the segment a-b.
    let seg = |px: f32, py: f32, a: (f32, f32), b: (f32, f32)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let t = (((px - a.0) * dx + (py - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
        ((px - a.0 - t * dx).powi(2) + (py - a.1 - t * dy).powi(2)).sqrt()
    };
    let mut rgba = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let stem = seg(px, py, (28.0, 15.0), (28.0, 44.0)) - 7.0;
            let bulb = ((px - 28.0).powi(2) + (py - 47.0).powi(2)).sqrt() - 13.0;
            let ticks = [14.0, 23.0, 32.0]
                .iter()
                .map(|&ty| seg(px, py, (42.0, ty), (50.0, ty)) - 2.0)
                .fold(f32::MAX, f32::min);
            let d = stem.min(bulb).min(ticks);
            rgba[(y * N + x) * 4 + 3] = ((0.5 - d).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    rgba
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn menu_bar_icon() -> Result<Icon> {
    Ok(Icon::from_rgba(menu_bar_rgba(), 64, 64)?)
}

/// Whether a StatusNotifier host is running, i.e. whether a tray icon would
/// actually be visible. Stock GNOME has none (it needs the AppIndicator
/// extension); KDE Plasma, XFCE, Cinnamon, MATE and others do.
///
/// The icon itself is published through the `ksni` backend of `tray-icon`,
/// which does not check for a host, so the app asks the session bus first:
/// `org.kde.StatusNotifierWatcher` must exist and report a registered host.
/// With `wait`, retries for a few seconds, since a login-time autostart can
/// run before the panel is up.
#[cfg(target_os = "linux")]
pub fn host_available(wait: bool) -> bool {
    let tries = if wait { 12 } else { 1 };
    for i in 0..tries {
        if i > 0 {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        if watcher_has_host() {
            return true;
        }
    }
    false
}

#[cfg(target_os = "linux")]
fn watcher_has_host() -> bool {
    use zbus::blocking::{fdo::DBusProxy, Connection, Proxy};
    use zbus::names::BusName;
    const NAME: &str = "org.kde.StatusNotifierWatcher";
    let Ok(conn) = Connection::session() else { return false };
    let owned = BusName::try_from(NAME)
        .ok()
        .zip(DBusProxy::new(&conn).ok())
        .and_then(|(name, bus)| bus.name_has_owner(name).ok())
        .unwrap_or(false);
    if !owned {
        return false;
    }
    // An old watcher without the property: assume it has a host.
    Proxy::new(&conn, NAME, "/StatusNotifierWatcher", NAME)
        .and_then(|p| p.get_property::<bool>("IsStatusNotifierHostRegistered"))
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_bar_icon_is_a_black_silhouette() {
        let px = menu_bar_rgba();
        assert_eq!(px.len(), 64 * 64 * 4);
        assert!(px.chunks(4).all(|p| p[..3] == [0, 0, 0]), "template images are black + alpha");
        let alpha = |x: usize, y: usize| px[(y * 64 + x) * 4 + 3];
        assert_eq!(alpha(28, 30), 255); // inside the stem
        assert_eq!(alpha(28, 50), 255); // inside the bulb
        assert_eq!(alpha(46, 14), 255); // on a tick mark
        assert_eq!((alpha(0, 0), alpha(63, 63), alpha(40, 40)), (0, 0, 0)); // empty areas
        assert!(menu_bar_icon().is_ok());
        assert!(icon().is_ok());
    }
}
