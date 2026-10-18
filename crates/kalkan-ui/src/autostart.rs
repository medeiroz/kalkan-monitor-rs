//! "Start with the operating system".
//!
//! - **Windows**: a Task Scheduler task that runs at logon with the highest
//!   privileges, so the app starts elevated (needed by the sensor driver)
//!   without a UAC prompt. A Windows service can't be used: services run in
//!   session 0 and cannot show the window or the tray icon. Registering the
//!   task needs an elevated process, which the app normally is.
//! - **macOS**: a LaunchAgent in `~/Library/LaunchAgents`.
//! - **Linux**: an XDG autostart entry in `~/.config/autostart`.
//!
//! The app is launched with [`TRAY_ARG`], so it starts hidden in the tray.

use anyhow::Result;

/// Command-line flag: start hidden (tray only).
pub const TRAY_ARG: &str = "--tray";

#[cfg(windows)]
mod imp {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    use anyhow::{bail, Context, Result};

    const TASK_NAME: &str = "Kalkan Monitor";
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn schtasks(args: &[&str]) -> std::io::Result<std::process::Output> {
        Command::new("schtasks").args(args).creation_flags(CREATE_NO_WINDOW).output()
    }

    pub fn is_enabled() -> bool {
        schtasks(&["/Query", "/TN", TASK_NAME]).map(|o| o.status.success()).unwrap_or(false)
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    /// Task definition. Compared to `schtasks /SC ONLOGON` defaults, it removes
    /// the 72 h execution limit and the "only on AC power" conditions, and
    /// runs at normal priority.
    fn task_xml(user: &str, exe: &str) -> String {
        let (user, exe) = (xml_escape(user), xml_escape(exe));
        format!(
            r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Starts Kalkan Monitor at logon.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
      <Delay>PT10S</Delay>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>{tray}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
            tray = super::TRAY_ARG,
        )
    }

    pub fn enable() -> Result<()> {
        let exe = std::env::current_exe()?;
        let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
            (Ok(d), Ok(u)) => format!("{d}\\{u}"),
            (_, Ok(u)) => u,
            _ => bail!("could not determine the current user"),
        };
        // schtasks wants the XML file in UTF-16 (with BOM).
        let xml = task_xml(&user, &exe.to_string_lossy());
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
        let path = std::env::temp_dir().join(format!("kalkan-task-{}.xml", std::process::id()));
        std::fs::write(&path, bytes).context("writing the task file")?;
        let out = schtasks(&["/Create", "/TN", TASK_NAME, "/XML", &path.to_string_lossy(), "/F"]);
        let _ = std::fs::remove_file(&path);
        let out = out.context("running schtasks")?;
        if !out.status.success() {
            bail!(
                "schtasks failed (run Kalkan Monitor as administrator): {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    pub fn disable() -> Result<()> {
        if !is_enabled() {
            return Ok(());
        }
        let out = schtasks(&["/Delete", "/TN", TASK_NAME, "/F"]).context("running schtasks")?;
        if !out.status.success() {
            bail!("schtasks failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }
}

/// The LaunchAgent plist (macOS). Platform independent, so it is tested everywhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod launch_agent {
    pub const LABEL: &str = "io.github.medeiroz.kalkan-monitor";

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    /// The plist that starts `exe` (the executable inside the installed
    /// `Kalkan Monitor.app`) with `--tray` at login, in the GUI session.
    pub fn plist(exe: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>{exe}</string><string>{tray}</string></array>
  <key>RunAtLoad</key><true/>
  <key>LimitLoadToSessionType</key><string>Aqua</string>
  <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
"#,
            exe = xml_escape(exe),
            tray = super::TRAY_ARG,
        )
    }

    /// Whether `exe` is somewhere a LaunchAgent cannot rely on: a mounted disk
    /// image (the app was run from the .dmg) or Gatekeeper's randomized
    /// "App Translocation" copy. Both vanish, and the agent would point nowhere.
    pub fn is_unstable_location(exe: &str) -> bool {
        exe.starts_with("/Volumes/") || exe.contains("/AppTranslocation/")
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::path::PathBuf;

    use anyhow::{bail, Context, Result};

    use super::launch_agent::{is_unstable_location, plist, LABEL};

    fn plist_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
    }

    pub fn is_enabled() -> bool {
        plist_path().is_some_and(|p| p.exists())
    }

    pub fn enable() -> Result<()> {
        let path = plist_path().context("no home directory")?;
        // Inside the .app bundle this is `Kalkan Monitor.app/Contents/MacOS/kalkan-monitor`.
        let exe = std::env::current_exe()?.to_string_lossy().into_owned();
        if is_unstable_location(&exe) {
            bail!("Move Kalkan Monitor to the Applications folder and open it from there first");
        }
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, plist(&exe))?;
        Ok(())
    }

    pub fn disable() -> Result<()> {
        match plist_path() {
            Some(p) if p.exists() => Ok(std::fs::remove_file(p)?),
            _ => Ok(()),
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
    use std::path::PathBuf;

    use anyhow::{Context, Result};

    fn desktop_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("autostart").join("kalkan-monitor.desktop"))
    }

    pub fn is_enabled() -> bool {
        desktop_path().is_some_and(|p| p.exists())
    }

    pub fn enable() -> Result<()> {
        let path = desktop_path().context("no config directory")?;
        let exe = std::env::current_exe()?;
        let entry = format!(
            "[Desktop Entry]\nType=Application\nName=Kalkan Monitor\nExec=\"{}\" {}\nX-GNOME-Autostart-enabled=true\n",
            exe.to_string_lossy(),
            super::TRAY_ARG,
        );
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, entry)?;
        Ok(())
    }

    pub fn disable() -> Result<()> {
        match desktop_path() {
            Some(p) if p.exists() => Ok(std::fs::remove_file(p)?),
            _ => Ok(()),
        }
    }
}

/// Whether the app is currently registered to start with the system.
pub fn is_enabled() -> bool {
    imp::is_enabled()
}

/// Registers or unregisters the app to start with the system.
pub fn set_enabled(on: bool) -> Result<()> {
    if on {
        imp::enable()
    } else {
        imp::disable()
    }
}

#[cfg(test)]
mod tests {
    use super::launch_agent::*;

    #[test]
    fn plist_runs_the_bundle_executable_in_the_tray() {
        let p = plist("/Applications/Kalkan Monitor.app/Contents/MacOS/kalkan-monitor");
        assert!(p.contains(
            "<string>/Applications/Kalkan Monitor.app/Contents/MacOS/kalkan-monitor</string><string>--tray</string>"
        ));
        assert!(p.contains(&format!("<string>{LABEL}</string>")));
        assert!(p.contains("<key>RunAtLoad</key><true/>"));
        assert!(plist("/a&b/<x>").contains("/a&amp;b/&lt;x&gt;"));
    }

    #[test]
    fn disk_images_and_translocated_copies_are_not_stable() {
        assert!(is_unstable_location("/Volumes/Kalkan Monitor/Kalkan Monitor.app/Contents/MacOS/kalkan-monitor"));
        assert!(is_unstable_location("/private/var/folders/x/AppTranslocation/ABC/d/Kalkan Monitor.app/Contents/MacOS/kalkan-monitor"));
        assert!(!is_unstable_location("/Applications/Kalkan Monitor.app/Contents/MacOS/kalkan-monitor"));
        assert!(!is_unstable_location("/Users/me/Applications/Kalkan Monitor.app/Contents/MacOS/kalkan-monitor"));
    }
}
