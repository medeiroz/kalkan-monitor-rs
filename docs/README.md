# Documentation

Documentation is organized per firmware version of the Kalkan PC SmartMonitor
software, so it is clear what applies to which cooler.

| Folder | Firmware | Status | Contents |
|--------|----------|--------|----------|
| [`v1.3/`](v1.3/) | 1.3 (serial, 1 Mbaud, 5-byte frame) — Alpha Rev.2 and Delta V2 | Current, implemented | Reverse-engineered protocol ([EN](v1.3/protocol.md), [PT-BR](v1.3/protocol-PT.md)) |
| [`v1.2/`](v1.2/) | 1.2 (USB HID `0416:1100`, handshake + scrambled 64-byte frame) — Alpha Rev.1 | Documented; experimental, not implemented in this app, untested on hardware | Reverse-engineered protocol ([EN](v1.2/protocol.md), [PT-BR](v1.2/protocol-PT.md)) |

Not sure which firmware your cooler uses? See "Troubleshooting / which version
do I have" in either protocol document ([1.3](v1.3/protocol.md#troubleshooting--which-version-do-i-have),
[1.2](v1.2/protocol.md#troubleshooting--which-version-do-i-have)).

Documents that are independent of the firmware stay directly in this folder:

- Sensors on Linux: [EN](sensors-linux.md), [PT-BR](sensors-linux-PT.md)
- Sensors on macOS: [EN](sensors-macos.md), [PT-BR](sensors-macos-PT.md)
- Sensors on Windows: [EN](sensors-windows.md), [PT-BR](sensors-windows-PT.md)
