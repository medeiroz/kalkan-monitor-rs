# Kalkan Monitor RS: monitor for Kalkan Alpha 360, 360-W and 240 AIO watercoolers

**English** | [Português (Brasil)](README-PT.md)

Open-source, cross-platform (Windows / macOS / Linux) monitor for the **Kalkan
Alpha 360**, **Kalkan Alpha 360-W** and **Kalkan Alpha 240** AIO liquid
coolers. It reads your **CPU temperature** and shows it on the cooler's round
LCD, as a free, open-source alternative to the official Kalkan PC SmartMonitor
software. Written in Rust.

## Supported coolers

| Cooler | Notes |
| --- | --- |
| Kalkan Alpha 360 | 360 mm AIO watercooler with a round LCD |
| Kalkan Alpha 360-W | White version of the Alpha 360 |
| Kalkan Alpha 240 | 240 mm AIO watercooler with a round LCD |

All three have a **round display that shows only the CPU temperature**. Fan
RPM and load are not shown on the screen (the app still sends them in the
protocol frame, but these models do not display them).

> [!IMPORTANT]
> **This repository targets version 1.3 of the Kalkan PC SmartMonitor
> software/protocol**, used in products bought **from January 2026 onward**. Version 1.2 (older products)
> uses a different protocol and is **not compatible** with this code — and vice
> versa. It is a fork adapted from the 1.2 repository.

It is a Rust reimplementation of the driver + interface for the **Kalkan
SmartMonitor 1.3** protocol used by these coolers. A new project — it
is not the original `.exe` translated; it is a reconstruction based on reverse
engineering its behavior (see [`docs/protocol.md`](docs/protocol.md)), with a
new modern UI on top.

## Why it exists

The original software is Windows-only: it uses HWiNFO for sensors and a Qt/QML
app for the UI (and, in the installed version, it also disables UAC — removed
separately). Here the useful logic — reading sensors and talking to the cooler's
display over serial — becomes a portable core, and the interface is rebuilt from
scratch.

## What is different in 1.3

Compared to 1.2, the 1.3 is **simpler** (details in `docs/protocol.md`):

- **serial-only** transport (USB-serial), without the 1.2's HID path;
- device found by **fixed VID/PID** (`1F3A:0008` or `1A86:484A`), opened at
  **1 Mbps, 8N1, no flow control**;
- **no handshake / no seed**;
- **5-byte** frame `74 00 temp fanHi fanLo`, scrambled with an **XOR `0x02`**
  from byte 2 onward (no `cpu_load`, no frame index).

## Structure

```
crates/
  kalkan-core/   # core: serial transport, sensors, full 1.3 protocol
  kalkan-cli/    # `kalkan` utility (list, sensors, frame, send, hwinfo, hid)
  kalkan-ui/     # modern dashboard (Slint, light theme) → `kalkan-monitor` bin
sidecar/
  kalkan-sensord/ # Windows sensors via LibreHardwareMonitor (.NET)
windows/
  installer/    # Windows installer (Inno Setup) + build.ps1
linux/          # .deb packaging files: udev rule, .desktop, icons, maintainer scripts
macos/          # .app/.dmg packaging: Info.plist template + build.sh
docs/
  protocol.md    # reverse-engineering findings (reconstructed 1.3 protocol)
```

## Install (Windows)

Download `KalkanMonitor-Setup-<version>.exe` from
[Releases](https://github.com/medeiroz/kalkan-monitor-rs-v1.3/releases). It installs the
dashboard, the sensor sidecar and the **PawnIO** driver (needed to read
temperature/fan, including with Memory Integrity turned on). Nothing else to
install separately.

To build the installer locally: `./windows/installer/build.ps1 -Version 0.1.0`
(needs Rust, .NET 8 SDK and Inno Setup 6).

### Tray and start with the system

- Closing the window keeps the app running in the **tray** (it keeps updating
  the display). Left-click the icon to reopen it; right-click → **Close
  monitor** quits. Opening the app again while it runs just shows the window.
- **Settings → Start with the operating system** registers the app to start
  hidden in the tray at login:
  - Windows: a Task Scheduler task (`Kalkan Monitor`, at logon, highest
    privileges), so it starts elevated without a UAC prompt. The uninstaller
    removes it.
  - macOS: a LaunchAgent in `~/Library/LaunchAgents` that runs the executable
    inside the installed `Kalkan Monitor.app`, so enable it from the copy in
    Applications (see [Install (macOS)](#install-macos)).
  - Linux: an entry in `~/.config/autostart`. The tray needs a StatusNotifier
    host (see [Install (Linux)](#install-linux)); without one, closing the
    window quits and the window opens at login instead of staying hidden.

### Publishing a release

```bash
git tag v0.1.0 && git push origin v0.1.0
```

GitHub Actions (`.github/workflows/release.yml`) builds the Windows installer,
the Linux `.deb` and the macOS `.dmg` and creates the Release with them attached. Every push/PR runs the tests on Windows,
Linux and macOS (`ci.yml`).

## Install (Linux)

Download `kalkan-monitor_<version>_amd64.deb` from
[Releases](https://github.com/medeiroz/kalkan-monitor-rs-v1.3/releases) and install it:

```bash
sudo apt install ./kalkan-monitor_<version>_amd64.deb
```

The package installs the `kalkan-monitor` app, a desktop entry, an icon and the
**udev rule** that lets you open the display's serial port. Other distros: build
from source (below) and copy [`linux/udev/60-kalkan-monitor.rules`](linux/udev/60-kalkan-monitor.rules)
to `/etc/udev/rules.d/`.

**Serial port access.** The display shows up as `/dev/ttyUSB*` or `/dev/ttyACM*`,
which normal users cannot open by default. The udev rule grants access to the
user logged in at the machine (`TAG+="uaccess"`), with a `dialout` group
fallback. After installing it, **replug the display** (the package also reloads
the rules). Without the rule, add yourself to the group and log in again:

```bash
sudo usermod -aG dialout $USER   # "uucp" on Arch
```

If the port cannot be opened, the app says so in the error banner and repeats
this fix.

**Sensors** come from the kernel's hwmon interface (`/sys/class/hwmon`): CPU,
GPU, motherboard fans, NVMe drives, memory and liquid coolers, grouped by
category in the source selectors, with Auto picks for the CPU temperature and
the fan/pump. No driver or root is needed, but the matching kernel module must
be loaded (`k10temp`/`coretemp`, `nct6775`/`it87` for the motherboard, ...);
`sensors-detect` from lm-sensors helps find it. NVIDIA's proprietary driver has
no hwmon: GPU temperature comes from `nvidia-smi` when it is installed. See
[`docs/sensors-linux.md`](docs/sensors-linux.md).

**Tray.** The icon is a StatusNotifierItem (no GTK or appindicator libraries
needed). It works out of the box on KDE Plasma, XFCE, Cinnamon, MATE and similar.
**GNOME** needs the *AppIndicator and KStatusNotifierItem Support* extension
(Ubuntu ships it enabled). Without a StatusNotifier host the app shows no tray
icon: closing the window quits, and starting with `--tray` (autostart) opens the
window instead of running invisibly.

Opening the app while it is already running just shows its window (a socket in
`$XDG_RUNTIME_DIR` keeps a single instance).

## Install (macOS)

Download `KalkanMonitor-<version>.dmg` from
[Releases](https://github.com/medeiroz/kalkan-monitor-rs-v1.3/releases), open it
and drag **Kalkan Monitor** to **Applications**. It is a universal app (Apple
Silicon and Intel) for macOS 11 or later. Run it from Applications, not from the
disk image, so that "start with the operating system" can find it.

**First launch (Gatekeeper).** The app is signed ad hoc and not notarized (that
needs a paid Apple developer account), so macOS says it is from an
"unidentified developer" or cannot be opened. Approve it once:

- right-click (or Control-click) **Kalkan Monitor** in Applications → **Open** →
  **Open**; or
- on macOS 15 Sequoia and later, where that shortcut is gone: try to open the
  app, then go to **System Settings → Privacy & Security**, scroll down and
  click **Open Anyway** next to the Kalkan Monitor message; or
- in a terminal: `xattr -dr com.apple.quarantine "/Applications/Kalkan Monitor.app"`.

**Menu bar.** The app puts a thermometer icon in the menu bar (a monochrome
template image, light or dark as the menu bar needs). Click it for **Open Kalkan
Monitor** and **Close monitor**. Closing the window (red button or ⌘W) only
hides it: the app keeps sending to the display from the menu bar, and clicking
its Dock icon brings the window back (best effort: if your macOS version ignores
that hook, use the menu-bar icon). **Close monitor** or ⌘Q quits for real. Opening
the app again while it runs just shows its window (a socket in `$TMPDIR` keeps
a single instance). **Settings → Start with the operating system** adds a
LaunchAgent that starts it hidden in the menu bar at login (`--tray`).

**Sensors.** No root and no driver. On Apple Silicon the temperatures come from
the IOKit HID sensors (CPU die, GPU, SSD, battery), with the CPU die average as
Auto; on Intel Macs from the SMC (`TC0P`, `TG0D`, ...). Fan speeds come from the
SMC; a fanless Mac (MacBook Air) has none, so the RPM field shows `--`. See
[`docs/sensors-macos.md`](docs/sensors-macos.md).

**Serial driver (CH34x).** The display's USB-serial chip is from WCH's CH34x
family. Recent macOS versions include a driver for the common ones, and the
display then shows up as `/dev/cu.usbserial-*` (or `/dev/cu.wchusbserial*`).
Older versions, and some chip variants, need WCH's CH34xVCPDriver
([wch-ic.com](https://www.wch-ic.com/)); after
installing it, allow it in **System Settings → Privacy & Security** and replug
the display. If the display is plugged in but the app cannot find a port, the
error banner says whether the display is not detected at all (cable) or detected
without a port (driver).

## Build and run

```bash
cargo build
cargo run -p kalkan-ui              # opens the dashboard
./target/debug/kalkan list          # lists serial ports — finds the cooler (★)
./target/debug/kalkan sensors       # current sensor reading
./target/debug/kalkan frame         # builds+scrambles a frame and shows the hex
./target/debug/kalkan send 42 1234 15  # sends fixed temp/fan to the display for 15 s (hardware test)
./target/debug/kalkan hwinfo        # (admin) readings from the original app's HWiNFO engine
cargo test                          # protocol + sensor tests (regression vectors)
```

Sensors on Windows (in dev): build the .NET sidecar and have PawnIO installed
(the installer takes care of that; LibreHardwareMonitor also installs it). If the
original Kalkan app is installed, its `config.dat` serves as a fallback.
See [`docs/sensors-windows.md`](docs/sensors-windows.md):

```bash
dotnet publish sidecar/kalkan-sensord -c Release -r win-x64 --self-contained
# in dev, the app also finds the sidecar in sidecar/kalkan-sensord/bin/.../publish/
```

Linux build (needs `libudev-dev`, `libfontconfig1-dev` and `libxkbcommon-dev`
on Debian/Ubuntu) and the package (`cargo install cargo-deb`):

```bash
cargo build --release -p kalkan-ui
cargo deb -p kalkan-ui              # -> target/debian/kalkan-monitor_*.deb
```

macOS build (needs the Xcode command line tools) and the app/disk image:

```bash
./macos/build.sh 0.1.0              # -> target/macos/KalkanMonitor-0.1.0.dmg (universal)
```

## Status

- [x] Core builds and runs on Windows, macOS and Linux (same crates).
- [x] Serial port discovery + VID/PID match (`1F3A:0008`, `1A86:484A`).
- [x] Serial transport with the cooler's exact parameters (1 Mbps / 8N1).
- [x] **Complete 1.3 protocol** reconstructed from static analysis: 5-byte frame +
      XOR `0x02` (port verified against the assembly, with a regression vector).
- [x] Modern UI (Slint, light theme): CPU / temperature / fan cards, status chip
      and device. Fixed **1 s** cadence (same as the original; no frequency
      selector) and serial discovery.
- [x] **Windows sensor backend**: temperature and fan, in order, from the
      **LibreHardwareMonitor sidecar + PawnIO** (`kalkan-sensord`; works with
      HVCI), the **original app's HWiNFO engine** (`config.dat`, if installed) and
      **HWiNFO Shared Memory**. CPU/RAM load come from `sysinfo`. The UI
      self-elevates (UAC), like the original. See [`docs/sensors-windows.md`](docs/sensors-windows.md).
- [x] **Source selection**: pick which sensor feeds the display's temperature and RPM fields (Auto, Max CPU/GPU or any temp/fan sensor); saved in `%APPDATA%\KalkanMonitor\settings.json`. See [`docs/sensors-windows.md`](docs/sensors-windows.md).
- [x] **Installer** (Inno Setup, PawnIO bundled) and **CI/CD** on GitHub Actions.
- [x] **Linux**: sensors from hwmon (+ `nvidia-smi`), tray over StatusNotifier
      with a fallback when there is no host, single instance, udev rule and a
      `.deb`. See [`docs/sensors-linux.md`](docs/sensors-linux.md). Written without
      a Linux machine at hand: tested through unit tests on a fake hwmon tree, so
      reports from real hardware are welcome.
- [x] **macOS**: sensors from IOKit (HID temperature sensors on Apple Silicon,
      SMC on Intel, SMC fans), menu-bar icon, single instance, serial driver
      hints, and a universal `.app` in a `.dmg`. See
      [`docs/sensors-macos.md`](docs/sensors-macos.md). Written without a Mac at
      hand: the pure logic is unit-tested and the macOS code is type-checked
      for both architectures, but only a run on real hardware (or the macOS
      CI job) proves it, so reports are welcome.
- [x] **End-to-end sending validated on 1.3 hardware**: cooler at `1A86:484A`
      (COM3), `kalkan send 42 1234` showed 42 °C / 1234 RPM on the display. The
      display sends nothing back over serial. Cadence always 1 s.
- [x] **Dashboard sends to the display** with real sensors: opens the port (and
      reopens it if it drops) and sends temp/fan every 1 s. Validated on a Ryzen 7
      9800X3D with HVCI on (CPU Die average / CPUFANIN0).

## Testing the display

```bash
./target/debug/kalkan list             # the cooler's port shows up with ★
./target/debug/kalkan send 42 1234 30  # the display should show 42 °C and 1234 RPM
```

If nothing changes, close the original Kalkan app (it may be holding the port).
