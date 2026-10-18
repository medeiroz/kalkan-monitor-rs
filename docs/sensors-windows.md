# Sensors on Windows (temperature and fan)

**English** | [Português (Brasil)](sensors-windows-PT.md)

Windows has **no API** for CPU temperature or fan RPM. Those values come from
processor MSRs and from the board's Super-I/O/Embedded Controller — both only
accessible in **ring 0 (kernel)**. Running "as administrator" is not enough
(admin is still ring 3); a **kernel driver** is required. That is why every
monitor (HWiNFO, LibreHardwareMonitor, Core Temp…) loads a driver, and why the
original Kalkan app asks for elevation: it embeds the HWiNFO engine (`config.dat`
**is** the SDK's `HWiNFO64.dll`, renamed, with the driver embedded as a resource).

The UI **self-elevates** at startup (relaunches via UAC), like the original, so
the drivers can be used. (`KALKAN_NO_ELEVATE=1` turns it off, in dev.)

Source order (`WindowsSource`): **LibreHardwareMonitor sidecar + PawnIO** →
**original app's HWiNFO engine** → **HWiNFO Shared Memory** → `None` (no error —
temp/fan show as `--`). CPU/RAM load always come from `sysinfo`.

macOS and Linux **do not use** any of this: they have native sources with no
driver for us to distribute (hwmon on Linux, SMC on macOS).

## Source 1 — LibreHardwareMonitor sidecar + PawnIO (default)

A `kalkan-sensord.exe` **sidecar** (.NET project in `sidecar/kalkan-sensord`,
published self-contained — no .NET install required) loads
**LibreHardwareMonitor** (MPL-2.0) and emits readings as one JSON line per
second on stdout:

```
{"cpu_temp_c":42.0,"fan_rpm":2400,"sensors":[{"id":"/lpc/nct6799d/0/fan/1","hardware":"NCT6799D","hardware_type":"SuperIO","name":"Fan #2","kind":"fan","value":2400},{"id":"/gpu-nvidia/0/temperature/0","hardware":"NVIDIA GeForce RTX 4090","hardware_type":"GpuNvidia","name":"GPU Core","kind":"temp","value":61.5}]}
```

`cpu_temp_c` / `fan_rpm` are the automatic picks (kept for older readers; either
can be `null`). `sensors` lists **every** temperature and fan sensor on all
hardware (CPU, GPU, motherboard/Super-I/O, fan controllers, storage, memory)
that passes the sanity filters (temperature 0–150 °C, fan 0–100000 RPM, 0 RPM
ignored). Each entry has a stable `id` (the LibreHardwareMonitor `Identifier`),
`hardware`, `hardware_type`, `name`, `kind` (`"temp"` or `"fan"`) and `value`.
Strings are ASCII-only (non-ASCII is `\uXXXX`-escaped). Lines without `sensors`
still parse (the list is just empty).

### Choosing what the display shows

The display takes exactly two values (a temperature byte and an RPM `u16`), but
each can come from any sensor. On the **Display** page, **Temperature source**
offers `Auto` (the automatic CPU temperature above), `Max CPU/GPU` (the higher
of the automatic CPU temperature and the hottest GPU core/hot-spot sensor) or a
specific sensor; **RPM source** offers `Auto` or a specific fan/pump sensor.
Sensors are labeled `<hardware> — <sensor>` and the lists refresh as sensors
appear. If the chosen sensor is missing, that field falls back to `Auto` rather
than sending 0. The choice is saved in `%APPDATA%\KalkanMonitor\settings.json`
(`~/.config/KalkanMonitor/settings.json` on Linux; a missing or corrupt file
means Auto/Auto):

```
{"temp_source":"max_cpu_gpu","fan_source":{"sensor":"/lpc/nct6799d/0/fan/1"}}
```

The HWiNFO `config.dat` fallback fills the same list (ids `hwinfo/<sensor>/<index>/<type>`);
the Shared Memory fallback does not enumerate sensors, so only `Auto` applies there.

`kalkan-core` spawns the sidecar (with no console window), reads that output on
a thread and combines it with `sysinfo`. It sits **side by side** with the
executable (or point to it via `KALKAN_SENSORD`).

LibreHardwareMonitor (0.9.5+) accesses the hardware through the **PawnIO**
driver: open source, signed by Microsoft and **compatible with Memory Integrity
(HVCI)**. The **installer** (`windows/installer/`) installs PawnIO 2.2.0 silently
(`PawnIO_setup.exe -install -silent`) when it is missing or < 2.0 — the same rule
as LibreHardwareMonitor — and does **not** remove it on uninstall (other programs
use the same driver). Validated on a Ryzen 7 9800X3D + Nuvoton NCT6799D with HVCI
on: Tctl/Tdie and "Fan #2" (= CPUFANIN0).

Without PawnIO, the sidecar runs but MSR/Super-I/O come back zeroed and
temp/fan are `None`; after 10 s with no data, `WindowsSource` tries Source 2.

Diagnostics: `kalkan-sensord.exe --dump` (elevated) lists all the hardware and
sensors LibreHardwareMonitor sees. Temperature/clock `0`/`null` with CPU Load
working = driver not active.

## Source 2 — the original app's HWiNFO engine (`config.dat`)

Fallback for when the original Kalkan app is installed. It also **works with
HVCI** (HWiNFO's driver is compatible). `kalkan-core` loads the DLL and calls the
functions **by ordinal**, like the `.exe`'s `RunHwinfo` (`FUN_140002d80`):

| ordinal | function | use |
|---|---|---|
| 590 | `Init(0x40) -> int` | 0 = ok; ≠ 0 no admin |
| 883 | `NumSensors() -> int` | |
| 726 | `Update()` | before each reading |
| 749 | `SelectSensor(int)` | |
| 106 | `GetReading(type, sensor, index, *raw) -> bool` | type 1 = temp, 3 = fan; index 0..0x200 |

Output struct (0x1D0 bytes): `+0x00` u8 valid, `+0x08` f64 value, `+0x3C`
char[256] sensor name, `+0x148` char[128] label, `+0x1C8` u32 type.

Selection (same as `FUN_1400023b0`): temperature = label with `package`/`average`;
fan = label with `cpu` (case-insensitive); otherwise the first plausible one.
It scans everything once and then re-reads only the two chosen readings every 1 s.

The DLL is searched for in: `KALKAN_HWINFO_DLL`; `config.dat`/`HWiNFO64.dll` next
to the executable; `C:\Program Files (x86)\Kalkan PC SmartMonitor DZ V1.3\config.dat`.
It is a proprietary HWiNFO binary: it is **not** versioned or redistributed here —
we use the copy from the original app's installation. It writes a `-debug.log` in
the current directory (ignored by git).

Diagnostics: `kalkan hwinfo` (elevated) lists every temp/fan reading and the pick.

## Source 3 — HWiNFO Shared Memory

If HWiNFO is open with "Shared Memory Support" on, we read
`Global\HWiNFO_SENS_SM2` (does not require admin).

## Turn off Memory Integrity?

**Not necessary** (Sources 1 and 2 work with HVCI) and **not recommended** — it
is a security downgrade.
