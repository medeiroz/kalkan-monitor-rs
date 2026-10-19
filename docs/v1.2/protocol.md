# Kalkan SmartMonitor **1.2** — reverse-engineering notes

**English** | [Português (Brasil)](protocol-PT.md)

> [!WARNING]
> **Status: implemented in this app, experimental, untested on hardware.**
> Everything below comes from static analysis of the vendor binary. The app
> speaks this protocol (see [Rust port](#rust-port)), but no 1.2 cooler has
> been tested with it yet. Reports are welcome: run `kalkan list` and
> `kalkan handshake` and open an issue.

Analyzed target: `Kalkan PC SmartMonitor DZ V1.2.exe`, the app used by **Alpha
line Rev.1** coolers (the vendor says: units sold before Jan/2026; see
[Which version do I have?](#troubleshooting--which-version-do-i-have)).
SHA-256 of the original binary:
`bacda52c244b23a383d375a91749803ef2b78e64c90f83a11f58e2d28de60f4d`.
It is the binary shipped inside the official installer
`Kalkan PC SmartMonitor (Linha Alpha) V1.2.exe` (Inno Setup, SHA-256
`61a241988da69bb36fd2a75e22a38277454e87d84bf55d8efb573c38b1e314b4`).

Method: installer unpacked with `innoextract` (never executed), then static
analysis with Ghidra 12.1.4 via REA (Ghidra-style decompilation + x86-64
assembly). Function names below (`FUN_…`) are Ghidra's default names, i.e. the
function addresses in the original binary.

## What differs from 1.3

| Aspect | 1.2 | 1.3 |
|---|---|---|
| Transport | **USB HID** (`hidapi`) only; the `QSerialPort` strings are unreferenced | Serial only (`QSerialPort`) |
| Device selection | VID:PID **`0416:1100`** hardcoded | VID/PID `1F3A:0008` / `1A86:484A` hardcoded |
| Serial parameters | — | 1 Mbps, 8N1, no flow control |
| Handshake / seed | 64-byte request `01 81 E3 …`, reply carries the seed after `0xE3` | none |
| Frame | **64 bytes** `01 86 E0 idx temp fanHi fanLo cpuLoad 00…` | 5 bytes `74 00 temp fanHi fanLo` |
| Scrambling | bitwise, seeded by the handshake, from byte 3 onward | XOR `0x02` from byte 2 onward |
| `cpu_load` in the frame | yes | no |
| Frame index | yes (1 byte, wraps at 256) | no |
| Read-back after each write | yes (65 bytes, 2000 ms, only logged) | no |
| Checksum | none | none |

## Architecture (classes)

| Layer | How the vendor app does it | Evidence |
|---|---|---|
| UI | Qt6 / QML, embedded (`qrc:/VTF_COM/main.qml`) | QML strings, `QQmlApplicationEngine::load` |
| Sensors | **HWiNFO** via `HWiNFO64.dll` (Windows only) | `config.dat` is a DLL importing `HWiNFO64.dll`; `SensorThread` |
| USB transport | `hidapi.dll` (class `HidDeviceController`) | `hid_*` imports, `HidDeviceController` RTTI |
| Serial transport | **none live** | `QSerialPort` / `WCOM` strings exist but are not referenced by any code path |

`HidDeviceController` object layout (offsets from `this`):

| Offset | Field |
|---|---|
| `+0x10` | `hid_device*` (open handle; 0 = closed) |
| `+0x24` | `pid` (int) |
| `+0x28` | `vid` (int) |
| `+0x40` | `mFrameIndex` (byte, frame counter) |
| `+0x41` | `scramblerSeed` (byte, from the handshake) |

## Opening the device — `FUN_140002cb0` → `FUN_140002cf0` → `FUN_140001a70`

- `FUN_140002cb0` calls open with the literal constants **`0x416`, `0x1100`**
  (VID `0416`, PID `1100`). These IDs are **hardcoded in the vendor exe**; they
  do not come from a config file (this supersedes older notes that said so).
- `FUN_140002cf0` stores vid/pid in the object and calls `FUN_140001a70`, which
  calls **`hid_open(vid, pid, NULL)`**: first device matching the IDs, no serial
  number, **no usage-page or interface filter**.
- `0416` is a generic Nuvoton vendor ID used by many unrelated devices, so a port
  should filter more strictly (e.g. by usage page / interface / report size once
  those are known from a real unit) instead of blindly opening the first match.
  This app enumerates, keeps `0416:1100` only, opens each candidate by path and
  only streams frames after its handshake succeeds (see [Rust port](#rust-port)).

### `isOpen()` — `FUN_1400017f0`

Calls `hid_enumerate()` and walks the list comparing `vendor_id`/`product_id`
with the stored vid/pid. Before sending each frame the vendor app re-runs this
check and **closes the handle if the device is no longer listed** (unplug
detection).

### `writeData(QByteArray)` — `FUN_1400023e0` → `FUN_140002260`

Copies the `QByteArray` into a local buffer, logs `[writeData] sent: <hex>` and
calls the raw send: `hid_set_nonblocking(dev, 0)` (blocking) + `hid_write`.

## Handshake — `FUN_140002cf0`, `FUN_140002740`

1. Build the seed request (`GenerateScramblerSeed`, `FUN_140002740`): 64 bytes,
   `[0]=0x01 [1]=0x81 [2]=0xE3`, rest `0x00`.
2. Send it via `writeData`.
3. Read **65 bytes** with `hid_read_timeout(…, 2000)` (blocking, 2000 ms).
4. Look for the magic byte **`0xE3`** at index 0 (seed = byte 1) **or** index 1
   (seed = byte 2). The byte right after `0xE3` is the scrambler seed, stored at
   `+0x41`.
5. If `0xE3` is not found: logs `[open] Can't found 0xE3`, **closes the handle
   and sets the seed to 0, but open still returns true**.

## Data frame and scrambling

### Frame — `GenerateShowDevInfo`, `FUN_140002800`

Source debug string: `"[GenerateShowDevInfo] temp: … fan: … cpuload: … mFrameIndex: …"`.

| offset | content | on the wire |
|---|---|---|
| 0 | `0x01` | literal |
| 1 | `0x86` | literal |
| 2 | `0xE0` ("show info" command) | literal |
| 3 | `mFrameIndex` | scrambled |
| 4 | `temp` (°C, 1 byte) | scrambled |
| 5 | `fan` high byte (big-endian) | scrambled |
| 6 | `fan` low byte | scrambled |
| 7 | `cpu_load` (%, 1 byte) | scrambled |
| 8..63 | `0x00` | scrambled |

No checksum.

**Frame index:** one byte at `+0x40`, written into byte 3 and then incremented
(wraps at 256). The vendor app does **not** reset it on reopen. (A port that
resets it on every new handshake is a reasonable choice; the initial value is
unconfirmed, most likely 0.)

### Scrambler — `FUN_140002a20`

Source debug strings: `[Scrambler] init/src/out`. Checked against the assembly
(8-bit arithmetic with wraparound). Bytes 0–2 stay literal; from byte 3 onward:

```
seed = <handshake seed>             // byte, passed BY VALUE
for pos in 3..len:
    acc = 0                         // byte
    for bit in 0..8:
        v = ((src[pos] << bit) & 0x80) + ((seed & 0xf0) << 3) + (seed & 0x80)   // u8
        acc  = acc  + (v >> bit)                                                // u8
        seed = seed*2 + (v >> 6)                                                // u8
    out[pos] = acc
```

The scrambler takes the seed **by value** and only mutates a local copy. So:

- inside a frame, the seed evolves bit by bit and carries over from one byte
  position to the next;
- **every frame restarts from the handshake seed**; the stored seed only changes
  on a new handshake.

### Transmission loop — `processTrans`, `FUN_140008030`

Per timer cycle:

1. `isOpen()` check (see above); close the handle if the device disappeared.
2. Read temp (1 byte) and fan (2 bytes) from the sensor tables, build the frame
   (`GenerateShowDevInfo`), scramble it, send it (`send`, `FUN_140003180` →
   `writeData`). Logs `[process trans] source:` / `[process trans] scramb:`.
3. **Read back 65 bytes** (blocking, 2000 ms). The reply is **only logged**: not
   validated, not used for re-seeding, no retry. A silent device therefore
   stalls each cycle for up to 2 s. Whether the device actually needs this read
   is unknown without hardware.

## Golden vectors

Input `temp=45`, `fan=1500`, `cpu_load=20`, seed restarted for each frame.
Full 64-byte frames on the wire (hex):

| seed | idx | frame |
|---|---|---|
| `00` | 0 | `0186e0002fa0947b42afa51b8fe1de592044c5d6c19a9cf6855f4a371fc3bcb240898bad833539ed0abe946e3f8779648113175b066a73da157d28dc7f0ef2c9` |
| `00` | 1 | `0186e0013cb7cf7d28dc7f0ef2c902262eb60cd4e7b42afa51b8fe1de592044c5d6c19a9cf6855f4a371fc3bcb240898bad833539ed0abe946e3f87796481131` |
| `FF` | 0 | `0186e00756c4156855f4a371fc3bcb240898bad833539ed0abe946e3f8779648113175b066a73da157d28dc7f0ef2c902262eb60cd4e7b42afa51b8fe1de5920` |
| `FF` | 1 | `0186e00645d34e6e3f8779648113175b066a73da157d28dc7f0ef2c902262eb60cd4e7b42afa51b8fe1de592044c5d6c19a9cf6855f4a371fc3bcb240898bad8` |
| `2A` | 0 | `0186e07d077ceb75b066a73da157d28dc7f0ef2c902262eb60cd4e7b42afa51b8fe1de592044c5d6c19a9cf6855f4a371fc3bcb240898bad833539ed0abe946e` |
| `2A` | 1 | `0186e07c146bb073da157d28dc7f0ef2c902262eb60cd4e7b42afa51b8fe1de592044c5d6c19a9cf6855f4a371fc3bcb240898bad833539ed0abe946e3f87796` |

Regression vector (from the 1.2 repository's tests): seed `0x2A`, idx 1,
`temp=40`, `fan=1200`, `cpu_load=55` → first 16 bytes
`01 86 e0 7c 11 35 83 01 13 17 5b 06 6a 73 da 15`.

## Rust port

**Implemented, experimental, untested on hardware.** Ported from the earlier
1.2 repository; the golden vectors above are its unit tests.

| Piece | Where |
|---|---|
| Seed request, seed parsing, frame, scrambler, frame builder (pure, no I/O) | `crates/kalkan-core/src/v1_2/protocol.rs` |
| HID enumeration, candidate filter, `HidTransport` (hidapi) | `crates/kalkan-core/src/v1_2/hid.rs` |
| Handshake + send sequence, `V1_2Display` | `crates/kalkan-core/src/v1_2/display.rs` |
| Firmware selection (1.3 serial first, then 1.2 HID) | `crates/kalkan-core/src/firmware.rs` (`find`, `open`) |

How the app opens the display: firmware 1.3 always wins. HID is only
enumerated when no 1.3 serial port is present; the `0416:1100` entries with a
path are the candidates (vendor-defined usage pages `0xFF00+` first, then
enumeration order). Each one is opened **by path** (`hid_open_path`) and
handshaked; the first that answers is used. If a 1.3 port is found but fails to
open, that error is shown; there is no fallback to 1.2.

Implementation decisions (deviations from the vendor app are marked):

- **Missing `0xE3` is a handshake failure** (deviation). The vendor app closes
  the handle, uses seed 0 and still reports "open". Here no reply within
  2000 ms, a reply without `0xE3` at index 0/1, or an I/O error during the
  handshake is an error: nothing is streamed to a device that did not complete
  the handshake (with a generic VID it may not even be a cooler). The UI retries
  with backoff (0.9 s, 1.8 s, 3.6 s, then every 5 s).
- **Handshake timeout** stays at the vendor's 2000 ms; it runs on the UI's
  device worker thread, never on the UI thread.
- **Read-back after each frame is kept but short** (deviation in timing). The
  vendor blocks up to 2000 ms; this app reads 65 bytes with a **150 ms**
  timeout and ignores the result (data, nothing or an error), so the 1 s cadence
  holds even with a silent device. It is a constant (`READ_BACK`) plus
  `V1_2Display::set_read_back`, so it can be turned off if hardware shows it is
  not needed.
- **Frame index restarts at 0 on every new handshake** (deviation). The vendor
  keeps counting across reopens. The index is a `u8` that wraps at 256; the UI
  preview shows it while sending.
- **Seed restarts every frame**, as in the vendor (the scrambler takes it by
  value); it only changes on a new handshake.
- **Write error → new session**: the device is dropped and reopened, which runs
  a new handshake (fresh seed, index 0). Unplug detection relies on the write
  failing; the vendor instead re-enumerates before every frame (`isOpen`).
- **Blocking writes** as in the vendor (`hid_set_nonblocking(dev, 0)`); byte 0
  (`0x01`) goes to hidapi as the report ID, exactly like the vendor's
  `hid_write` of 64 bytes.
- **Only one program at a time.** Close the official software: on Windows HID
  handles are shared, so two apps would interleave frames (with different
  seeds); on macOS hidapi opens the device exclusively, so one of them fails to
  open it.
- **Linux**: the `.deb` ships a udev rule giving the logged-in user access to
  the `hidraw` node (`TAG+="uaccess"`). A permission error explains the udev
  fix (not the serial `dialout` group). **macOS**: no TCC permission is needed
  for a vendor-defined HID device; the only caveat is the exclusive open above.

Diagnostics for someone with a real unit: `kalkan list` (serial ports and
`0416:1100` HID candidates), `kalkan handshake` (sends only the seed request and
prints request, reply and parsed seed), `kalkan send [temp] [fan] [seconds]
[usage]` (fixed values, prints each frame and any reply) and `kalkan hid` (every
HID device).

## Hardware-dependent (open questions)

Only a real Rev.1 unit can answer these:

- Do real units enumerate as `0416:1100`? Which usage page / interface, and what
  input/output report length (the app writes 64 bytes and reads 65)?
- What does the real handshake reply look like (position of `0xE3`, seed value)?
- Does the device need the read-back after each write? Is there a minimum send
  interval?
- What exactly does the old screen show (temperature only, as on Rev.2?)
- Linux: does the shipped `hidraw` udev rule match the real device?
- macOS: does the device open without a driver or special entitlement (it
  should, as a vendor-defined HID device)?

## Troubleshooting / which version do I have

The vendor's `LEIA-ME.txt` (shipped with the Alpha installers, Portuguese)
says which software matches which hardware:

> "Kalkan PC SmartMonitor (Linha Alpha Rev.2) 1.3: Produtos da linha Alpha
> comprados a partir de Janeiro/2026 são da Revisão 2."
>
> "Kalkan PC SmartMonitor (Linha Alpha) V1.2: Produtos da linha Alpha vendidos
> antes de Janeiro/2026 são da Revisão 1."

(Alpha units bought from January 2026 onward are Revision 2 → software 1.3;
Alpha units sold before January 2026 are Revision 1 → software 1.2.)

The box label identifies the unit: the serial right above the barcode holds the
SKU followed by the manufacturing date as year/month/day (vendor example:
`KLK00047 - 20251106XXX`).

If the display **stays stuck at 88°**, the wrong version is installed:

> "Caso execute o software e o mesmo não mostre a temperatura no visor, ficando
> travado em 88°, desinstale a versão atual instalada e tente instalar a outra
> versão disponível nesta pasta."

(If the display does not show the temperature and stays stuck at 88°, uninstall
the current version and install the other one.) The vendor also says to install
and run its software as Administrator. In this app, the same symptom means the
display is not getting data it understands: close the official software and run
`kalkan list`. A 1.3 port (`1F3A:0008` / `1A86:484A`) means Rev.2 / Delta V2; a
`0416:1100` HID candidate means a Rev.1 / 1.2 unit, supported experimentally
(`kalkan handshake` shows whether it answers).
