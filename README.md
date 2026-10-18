# Kalkan Monitor RS

**English** | [Português (Brasil)](README-PT.md)

> [!IMPORTANT]
> **This repository targets version 1.3 of the Kalkan PC SmartMonitor**, used in
> products bought **from January 2026 onward**. Version 1.2 (older products)
> uses a different protocol and is **not compatible** with this code — and vice
> versa. It is a fork adapted from the 1.2 repository.

Cross-platform (Windows / macOS / Linux) Rust reimplementation of the driver +
interface for the **Kalkan PC SmartMonitor 1.3** watercooler. A new project — it
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

### Publishing a release

```bash
git tag v0.1.0 && git push origin v0.1.0
```

GitHub Actions (`.github/workflows/release.yml`) builds the installer and
creates the Release with it attached. Every push/PR runs the tests on Windows,
Linux and macOS (`ci.yml`).

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
- [x] **Installer** (Inno Setup, PawnIO bundled) and **CI/CD** on GitHub Actions.
- [ ] Sensor backends for Linux (lm-sensors/hwmon) and macOS (SMC) — temp/fan
      show `--` on those platforms for now (CPU load is real).
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
