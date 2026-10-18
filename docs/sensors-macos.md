# Sensors on macOS (temperature and fan)

**English** | [Português (Brasil)](sensors-macos-PT.md)

Like Linux, macOS needs no driver of ours and no root: IOKit hands the sensor
readings to any user. `MacSource` (`crates/kalkan-core/src/macos.rs`, with the
FFI in `macos/iokit.rs`) reads them once per second; CPU/RAM load come from
`sysinfo`, as on the other platforms. There are two sources, depending on the
Mac:

| Mac | Temperatures | Fans |
|---|---|---|
| Apple Silicon (M1 and later) | IOKit HID temperature sensors | SMC |
| Intel | SMC | SMC |

Both are probed on every Mac; whatever a machine does not have is skipped. If
IOKit cannot be reached at all, the app falls back to `sysinfo`'s CPU
temperature estimate (and no sensor list).

## Apple Silicon: HID sensors

The temperature sensors are HID services of the private event system
(`IOHIDEventSystemClient`, matching `PrimaryUsagePage` `0xff00` and
`PrimaryUsage` 5, event type `kIOHIDEventTypeTemperature`). A service is named
by its `Product` property, which is what the app maps to a friendly name and a
category:

| `Product` | Name shown | Category |
|---|---|---|
| `PMU tdie1`, `PMU tdie2`, ... | `CPU Die N` | CPU |
| `PMU tdev1`, ... | `SoC Device N` | CPU |
| `pACC MTR Temp Sensor0`, ... | `CPU Performance N` | CPU |
| `eACC MTR Temp Sensor0`, ... | `CPU Efficiency N` | CPU |
| `SOC MTR Temp Sensor0`, ... | `SoC N` | CPU |
| `GPU MTR Temp Sensor1`, ... | `GPU Core N` | GPU |
| `NAND CH0 temp`, ... | `NAND CH0` | Storage |
| `gas gauge battery` | `Battery` | Other |
| `ANE MTR Temp Sensor1` | `Neural Engine N` | Other |
| anything else | the `Product` text | Other |

`PMU tcal` is a calibration value, not a temperature, and is dropped. Readings
outside 0..150 °C (disconnected sensors report absurd values) are skipped. Which
sensors exist depends on the chip generation; the `tdie`/`tdev` ones are on all
of them, the `MTR` ones on newer chips.

Ids are `iohid/<Product>`, for example `iohid/PMU tdie1`; they are what
`~/Library/Application Support/KalkanMonitor/settings.json` stores for a chosen
sensor. (If two services share a `Product`, the second gets `#2`.)

## Intel: SMC keys

The SMC is the `AppleSMC` IOKit service, opened as a user client. A key is read
with two calls (key info, then the bytes), through the 80-byte `SMCKeyData`
struct. At start-up the app probes a fixed table of keys and keeps those that
exist:

| Keys | Name shown | Category |
|---|---|---|
| `TC0P`, `TC0D`, `TC0E`, `TC0F`, `TC0H`, `TCXC` | CPU Proximity / Die / Die (virtual) / Die (filtered) / Heatsink / PECI | CPU |
| `TC1C` to `TC8C` | `CPU Core N` | CPU |
| `TG0P`, `TG0D`, `TG0H` | GPU Core (proximity) / Core (diode) / Heatsink | GPU |
| `Tm0P`, `TM0P` | Memory Proximity / Slot Proximity | Memory |
| `TH0P` | Drive Proximity | Storage |
| `TPCD`, `TN0P`, `TN0D` | Platform Controller Hub, Northbridge | Motherboard |
| `TB0T`, `TB1T` | Battery 1 / 2 | Other |
| `TA0P`, `Ts0P`, `TW0P`, `TL0P`, `Th0H` to `Th2H` | Ambient, Palm Rest, Wi-Fi, Display, Heatpipes | Other |

Ids are `smc/<KEY>`, for example `smc/TC0P`. A value of 0 means "no sensor
there" and is skipped. The GPU "Core" names are on purpose: **Max CPU/GPU**
looks for a GPU core or hot-spot temperature.

Values come in the SMC's own formats, decoded in `decode_smc`:

| Type | Meaning |
|---|---|
| `sp78` | signed fixed point, 7 integer + 8 fraction bits, big-endian (most temperatures) |
| `fpe2` | unsigned fixed point, 14 + 2 bits, big-endian (fan RPM on older Macs) |
| `flt ` | little-endian 32-bit float (fan RPM and temperatures on newer Macs) |
| `ui8 `, `ui16`, `ui32`, `si8 `, `si16` | big-endian integers |
| `spXY`, `fpXY` | the same fixed-point family in other widths |

## Fans

`FNum` gives the number of fans; for each, `F<n>Ac` is the actual speed in RPM
and `F<n>ID` (when present) its name (`Left side`, `Right side`). They are shown
as `Fan 1 (Left side)`, with the id `smc/F0Ac` for the first. This works on
Intel Macs and on Apple Silicon Macs that have fans (MacBook Pro, Mac Studio,
iMac, ...). A fanless Mac (MacBook Air) reports zero fans: the RPM selector
lists nothing and the RPM field of the display stays at 0. A Mac with fans
stops them when cool, so 0 RPM is a real reading and is sent as such.

## Categories

The hardware type decides the group in the selectors (the vocabulary that
`naming.rs` classifies): `Cpu`, `GpuApple`/`Gpu`, `Storage`, `Memory`,
`Motherboard` (fans, chipset), `Battery` and `Other`. Batteries and the
remaining sensors show under **Other**.

## Auto picks

**Temperature** (first match wins):

1. Apple Silicon: the **average of the CPU die sensors** (`CPU Die (average)`,
   id `iohid/cpu-die-average`): the mean of all `PMU tdie*`; on chips without
   them, of the `pACC`/`eACC` core sensors
2. Intel: `TC0P`, else `TC0D`, `TC0E`, `TC0F`
3. otherwise the `sysinfo` estimate

**Fan/pump:** `F0` (`smc/F0Ac`), else the first fan. A Mac without fans has no
Auto fan.

## Limits

- The HID event system functions are private API. They have been stable across
  macOS releases and are what the open-source monitors use, but Apple could
  change them.
- The SMC key table is a best-known list; some Macs name sensors differently, so
  a key can be missing. Open an issue with `kalkan sensors` output if a useful
  one is not listed.
- Written without a Mac at hand: the decoding, naming and picking logic is
  covered by unit tests that run on every OS, and the IOKit code is
  type-checked for `aarch64-apple-darwin` and `x86_64-apple-darwin`, but it has
  not been run on real hardware yet.

## Serial driver

The display's USB-serial chip is from WCH's CH34x family (`1A86:484A`). Recent
macOS versions include a driver for the common CH340/CH341 chips and the port
appears as `/dev/cu.usbserial-*`; older versions and other variants need
WCH's CH34xVCPDriver (port `/dev/cu.wchusbserial*`). macOS lists each port
twice, `/dev/cu.*` and `/dev/tty.*`; the app always opens the `cu.` one. When no
port is found, the error banner checks the USB bus (`ioreg`) and says whether
the display is not detected at all (check the cable) or detected without a
serial port (install the driver, allow it in System Settings → Privacy &
Security, replug).

For the other platforms see [`sensors-windows.md`](sensors-windows.md) and
[`sensors-linux.md`](sensors-linux.md).
