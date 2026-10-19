# Kalkan SmartMonitor **1.3** — reverse-engineering notes

**English** | [Português (Brasil)](protocol-PT.md)

Analyzed target: `Kalkan PC SmartMonitor DZ V1.3.exe` (Alpha line Rev.2, which
the vendor says covers units purchased from Jan/2026, and Delta V2). SHA-256 of
the original binary: `faf27352654837fa95558c2b5f0e9db3755b763e2c08540fed15a7d37af39897`.
The official installers "Kalkan PC SmartMonitor (Linha Delta V2) V1.3" and
"Kalkan PC SmartMonitor (Linha Alpha Rev.2) V1.3" (SHA-256
`b98d03b730548a089716831f35e44e32a6e861889bde67b6aae96c82c94d1589`) both ship
this exact binary (same SHA-256; checked by unpacking with `innoextract`, never
executed).
Tool: Ghidra 12.1.4 via REA. Everything below was **observed statically** in the
binary (pseudocode + x86-64 assembly).

> The reverse engineering was done on a local copy with a 1-byte patch in
> `main` (disables the app's anti-UAC — see the note at the end); the protocol
> code is identical to the original binary's and is not touched by that patch.

## What changed from 1.2 to 1.3

The 1.3 is **incompatible** with the 1.2 ([notes](../v1.2/protocol.md)) and **much simpler**:

| Aspect | 1.2 | 1.3 |
|---|---|---|
| Transport | HID only (`hidapi`; its `QSerialPort` strings are unreferenced) | **Serial only** (`QSerialPort`) |
| Device selection | VID:PID `0416:1100` fixed in code | VID/PID fixed in code (below) |
| Serial parameters | — | **1 Mbps, 8N1, no flow control** |
| Handshake / seed | packet `01 81 E3`, finds `0xE3` → seed | **none** |
| Frame | 64 bytes `01 86 E0 idx temp fanHi fanLo cpu …` | **5 bytes** `74 00 temp fanHi fanLo` |
| Scrambling | bitwise with an evolving seed | **XOR `0x02`** from byte 2 onward |
| `cpu_load` in the frame | yes | **no** |
| frame index | yes | **no** |

## Architecture (classes)

- **`Sysctl`** (vftable `0x140036008`): the controller exposed to QML as
  `sys_controller`. Reads sensors (via the QML type `HwiHandler`), builds the
  frame, scrambles it and **emits `emitSendData(QByteArray)`** (signal, index 4 →
  `FUN_14002fcc0`). It also handles config (`GENERAL`/`Auto Run`/`Show Fah`/
  `Language`/`Default Temp`/`Default Sped`) and language.
- **`Transport`** (vftable `0x1400364e0`): wraps a single `QSerialPort`
  (at `this+0x10`). Slots `slotSendData(QByteArray)` (`FUN_140008640` — writes
  to the serial port) and `slotRead()`. `Sysctl::emitSendData` is connected to
  `Transport::slotSendData` in the `Sysctl` constructor (`FUN_140005330`).

## Opening the device — `FUN_140008300` → `FUN_1400083d0`

`Sysctl::connect` tries two (VID, PID) pairs, in this order:

1. **`1F3A:0008`**
2. **`1A86:484A`** (CH340 family — a common USB-serial bridge)

`FUN_1400083d0(this, vid, pid)`:

1. `QSerialPortInfo::availablePorts()`; iterates matching `vendorIdentifier()` and
   `productIdentifier()` against (vid, pid); keeps the `portName()`.
2. If found: `setPortName`, opens (`open`, ReadWrite), and configures:
   - `setBaudRate(1000000, AllDirections)`
   - `setDataBits(8)`
   - `setParity(NoParity)`
   - `setStopBits(OneStop)`
   - `setFlowControl(NoFlowControl)`
3. Logs `"open ok"` / `"open error"`. Returns success only if all the `set*`
   calls succeed.

**No handshake packet is sent.** After opening, the app starts sending frames
right away.

## Data frame and scrambling — `Sysctl::processTrans`, `FUN_1400062b0`

Called periodically (timer). Summary:

```c
if (!opened) {                         // this+0x1c
    opened = Transport::open();        // FUN_140008300 (see above)
    if (opened) delay = 6;             // this+0x98: skips 6 cycles after opening
    emit openedChanged();
}
if (opened) {
    if (delay != 0) { delay--; return; }
    if (Transport::bytesToWrite() == 0) {      // FUN_1400083a0 == 0
        char  temp = *(u8  *)(this + 0x10);    // temperature
        u16   fan  = *(u16 *)(this + 0x12);    // speed (big-endian in the frame)

        buf.clear();
        buf.append(0x74);  // 't'
        buf.append(0x00);
        buf.append(temp);
        buf.append(fan >> 8);
        buf.append(fan & 0xff);

        for (i = 2; i < buf.size(); i++)   // header (2 bytes) stays literal
            buf[i] ^= 0x02;

        emitSendData(buf);                 // -> Transport writes to the serial port
    }
}
```

### Frame layout (5 bytes)

| offset | content | on the wire (after XOR) |
|--------|---------|-------------------------|
| 0 | `0x74` (`'t'`) | `0x74` (literal) |
| 1 | `0x00` | `0x00` (literal) |
| 2 | `temp` (°C) | `temp ^ 0x02` |
| 3 | `fan` high byte | `(fan>>8) ^ 0x02` |
| 4 | `fan` low byte | `(fan&0xff) ^ 0x02` |

Example (`temp=40`, `fan=1200`): raw `74 00 28 04 B0` → wire `74 00 2A 06 B2`.

## Rust port

Implemented and tested in `crates/kalkan-core/src/protocol.rs`:
`build_frame(temp, fan)`, `scramble(frame)` and `KalkanBuilder` (stateless). A
regression vector (`74 00 2A 06 B2`) pins the behavior. Opening by VID/PID +
baud rate lives in `device.rs` (`find_display_port`, `DISPLAY_IDS`) and
`transport.rs` (`SerialTransport::open_display`), with the `open_display()`
shortcut in `lib.rs`. Run `kalkan frame` (CLI) for the pipeline running offline
and `kalkan list` to find the cooler's port.

## Hardware-dependent

Nothing in the protocol **content** depends on observation anymore: frame, XOR,
VID/PID and serial parameters are all reconstructed. The only step that needs
the device is confirming that it enumerates with one of the VID/PIDs in
[`DISPLAY_IDS`] (`kalkan list`), since the test cooler only exists in version 1.3.

## Troubleshooting / which version do I have

The vendor's `LEIA-ME.txt` (shipped with the Alpha installers, Portuguese)
ties the software to the hardware revision:

> "Kalkan PC SmartMonitor (Linha Alpha Rev.2) 1.3: Produtos da linha Alpha
> comprados a partir de Janeiro/2026 são da Revisão 2."
>
> "Kalkan PC SmartMonitor (Linha Alpha) V1.2: Produtos da linha Alpha vendidos
> antes de Janeiro/2026 são da Revisão 1."

(Alpha units bought from January 2026 onward are Revision 2 → 1.3, this
protocol; units sold before January 2026 are Revision 1 → [1.2](../v1.2/protocol.md).)
The purchase date is the vendor's rule of thumb; the box label is more precise:
the serial right above the barcode holds the SKU followed by the manufacturing
date as year/month/day (vendor example: `KLK00047 - 20251106XXX`).

If the display **stays stuck at 88°**, the vendor says the wrong version is
installed:

> "Caso execute o software e o mesmo não mostre a temperatura no visor, ficando
> travado em 88°, desinstale a versão atual instalada e tente instalar a outra
> versão disponível nesta pasta."

(Uninstall the current version and install the other one; the vendor also asks
to install and run its software as Administrator.) With this app: if
`kalkan list` shows no port with `1F3A:0008` / `1A86:484A`, the cooler is
probably a Rev.1 (1.2) unit, supported experimentally over USB HID (it shows up
in `kalkan list` as a `0416:1100` HID candidate; see [1.2](../v1.2/protocol.md)).

## Note: anti-UAC (outside the protocol)

The 1.3's `main` turns UAC off (`EnableLUA=0` in
`HKLM\…\Policies\System`) on every start — behavior removed separately, by a
1-byte patch on the installed `.exe`. It has no relation to the display protocol
and is **not** reproduced in this port.
