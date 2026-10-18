# Sensors on Linux (temperature and fan)

**English** | [Português (Brasil)](sensors-linux-PT.md)

Unlike Windows, Linux needs no driver of ours: the kernel already exposes every
monitoring chip through **hwmon** (`/sys/class/hwmon`), readable by any user.
`LinuxSource` (`crates/kalkan-core/src/hwmon.rs`) reads it once per second;
CPU/RAM load come from `sysinfo`, as on the other platforms.

The matching kernel module has to be loaded for a chip to show up: `k10temp`
(AMD CPUs), `coretemp` (Intel), `nct6775`/`nct6799`, `it87` or `asus-ec-sensors`
(motherboard), `amdgpu`, `nvme`, `drivetemp` (SATA disks), `spd5118`
(DDR5 modules), `nzxt-kraken3`, `corsair-cpro`, `aquacomputer_d5next` (liquid
coolers and fan controllers). The `sensors-detect` tool from lm-sensors helps
find the one for a motherboard. Check what the kernel sees with
`ls /sys/class/hwmon/*/name`.

## What is read

For each `hwmonN` directory, the chip name (`name`) plus:

| File | Meaning |
|---|---|
| `tempN_input` | temperature in millidegrees Celsius |
| `tempN_label` | name of the sensor (optional) |
| `fanN_input` | fan speed in RPM |
| `fanN_label` | name of the fan (optional) |

Only `*_input` files are readings. `*_max`, `*_crit`, `*_min`, `*_alarm`, `pwm*`
and similar are thresholds or controls and are never read. Unreadable files and
implausible values (temperature outside -50..200 °C, fan above 100000 RPM) are
skipped. A sensor without a label is named `Temperature #N` / `Fan #N`.

The hardware name is the device's `model` (NVMe/SATA model, GPU product name)
when it exists, else a friendly name for the chip (`AMD CPU`, `NVMe drive`, ...).
On the CPU it is replaced by the model from `/proc/cpuinfo`.

Sensor ids are stable across reboots: `hwmon/<chip>/<device>/<temp|fan><N>`,
where `<device>` is the last component of the `device` symlink (PCI address,
`nvme0`, `coretemp.0`, ...), not the `hwmonN` index, which can change. They are
what `~/.config/KalkanMonitor/settings.json` stores for a chosen sensor, e.g.
`hwmon/k10temp/0000:00:18.3/temp1`.

## Categories

The chip name decides the group in the selectors (the hardware type that
`naming.rs` classifies):

| Chip name | Category |
|---|---|
| `k10temp`, `zenpower`, `coretemp`, `cpu_thermal` | CPU |
| `amdgpu`, `radeon`, `nouveau`, `i915`, `xe` | GPU |
| `nvme`, `drivetemp` | Storage |
| `spd5118`, `jc42` | Memory |
| `nct6*`, `nct7*`, `it87*`/`it86*`/`it8*`, `asus*`, `gigabyte_wmi`, `w83*`, `f71*`, `acpitz`, `pch_*`, ... | Motherboard |
| `nzxt*`, `corsair*` (except `corsairpsu`), `aquacomputer*`, `gigabyte_waterforce`, `asus_rog_ryujin` | Liquid / Pump |

Any sensor whose label contains `pump`, `coolant`, `liquid`, `water` or `aio` is
shown as Liquid / Pump whatever its chip. Unknown chips fall back to the name
heuristics and otherwise show under Other.

AMD GPU labels are renamed so the **Max CPU/GPU** source sees them: `edge`
becomes `GPU Core`, `junction` becomes `GPU Hot Spot`, `mem` becomes
`GPU Memory` (memory is not considered for Max CPU/GPU).

## Auto picks

**Temperature** (first match wins):

1. `k10temp` `Tctl`, else `Tdie`
2. `coretemp` `Package id 0`
3. `zenpower` `Tdie`
4. the first CPU sensor (any CPU chip)

If there is no CPU sensor in hwmon at all, the `sysinfo` estimate is used.

**Fan/pump** (a fan reading 0 RPM is an empty header and is never picked):

1. a fan whose label contains `pump`
2. a fan whose label contains `cpu` (`cpu_fan`, `CPU Fan`)
3. the first running fan of a motherboard chip

Most Super-I/O drivers publish **no fan labels** (the names in `sensors` come
from the lm-sensors config files), so on many boards the third rule applies. If
it picks the wrong header, choose the right fan in **RPM source**; the choice is
saved by id.

## NVIDIA

NVIDIA's proprietary driver has no hwmon. When `nvidia-smi` is installed, a
background thread runs

```
nvidia-smi --query-gpu=temperature.gpu,name --format=csv,noheader
```

every 5 seconds (3 s timeout, never on the 1 s loop), and each GPU shows up as
`nvidia-smi/<index>/temp1` named `GPU Core`, so it counts for **Max CPU/GPU**.
It is skipped while the GPU is powered down (`power/runtime_status` of the PCI
device is `suspended`), so a hybrid-graphics laptop's dGPU is not woken just to
read its temperature. The open-source `nouveau` driver does expose hwmon.

## Serial port permissions

The display is a USB serial device (`/dev/ttyUSB*` or `/dev/ttyACM*`). Install
`linux/udev/60-kalkan-monitor.rules` (the `.deb` does it) or join the `dialout`
group (`uucp` on Arch) and log in again. The app shows this fix when opening the
port fails with "permission denied".

## Limits

- Sensors that need an out-of-tree kernel module (some NZXT/ASUS/Gigabyte
  boards) only appear once that module is loaded.
- hwmon exposes what the chip reports; some boards mislabel or miswire headers.
- Tested with unit tests on a fake hwmon tree (they run on every OS); not yet
  validated on a wide range of real hardware.

For the other platforms see [`sensors-windows.md`](sensors-windows.md) and
[`sensors-macos.md`](sensors-macos.md).
