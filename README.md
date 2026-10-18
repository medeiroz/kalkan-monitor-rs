# Kalkan Monitor RS

> [!IMPORTANT]
> **Este repositório é da versão 1.3 do Kalkan PC SmartMonitor**, usada nos
> produtos comprados **a partir de janeiro/2026**. A versão 1.2 (produtos mais
> antigos) usa um protocolo diferente e **não é compatível** com este código —
> e vice-versa. Fork adaptado do repositório 1.2.

Reimplementação multiplataforma (Windows / macOS / Linux), em Rust, do driver +
interface do watercooler **Kalkan PC SmartMonitor 1.3**. Projeto novo — não é o
`.exe` original traduzido; é uma reconstrução baseada em engenharia reversa do
comportamento dele (ver [`docs/protocol.md`](docs/protocol.md)), com uma UI
moderna nova por cima.

## Por que existe

O software original é Windows-only: usa HWiNFO para sensores e um app Qt/QML
para a UI (além de, na versão instalada, desligar o UAC — removido à parte).
Aqui a lógica útil — ler sensores e falar com o display do cooler por serial —
vira um núcleo portátil, e a interface é refeita do zero.

## O que é diferente na 1.3

Comparada à 1.2, a 1.3 é **mais simples** (detalhes em `docs/protocol.md`):

- transporte **só serial** (USB-serial), sem o caminho HID da 1.2;
- dispositivo achado por **VID/PID fixos** (`1F3A:0008` ou `1A86:484A`), aberto
  a **1 Mbps, 8N1, sem controle de fluxo**;
- **sem handshake / sem seed**;
- frame de **5 bytes** `74 00 temp fanHi fanLo`, embaralhado com um **XOR `0x02`**
  do byte 2 em diante (sem `cpu_load`, sem índice de frame).

## Estrutura

```
crates/
  kalkan-core/   # núcleo: transporte serial, sensores, protocolo 1.3 completo
  kalkan-cli/    # utilitário `kalkan` (list, sensors, frame, hid)
  kalkan-ui/     # dashboard moderno (Slint, tema claro) → bin `kalkan-monitor`
docs/
  protocol.md    # achados da engenharia reversa (protocolo 1.3 reconstruído)
```

## Compilar e rodar

```bash
cargo build
cargo run -p kalkan-ui              # abre o dashboard
./target/debug/kalkan list          # lista portas seriais — acha o cooler (★)
./target/debug/kalkan sensors       # leitura de sensores atual
./target/debug/kalkan frame         # monta+embaralha um frame e mostra o hex
cargo test                          # testes do protocolo + sensores (vetores de regressão)
```

Sensores no Windows (opcional — temp/fan reais): compile o sidecar .NET e deixe
`kalkan-sensord.exe` ao lado do executável (ver [`docs/sensors-windows.md`](docs/sensors-windows.md)):

```bash
dotnet publish sidecar/kalkan-sensord -c Release -r win-x64 --self-contained
# em dev, o app também acha o sidecar em sidecar/kalkan-sensord/bin/.../publish/
```

## Estado

- [x] Núcleo compila e roda em Windows, macOS e Linux (mesmas crates).
- [x] Descoberta por porta serial + match de VID/PID (`1F3A:0008`, `1A86:484A`).
- [x] Transporte serial com os parâmetros exatos do cooler (1 Mbps / 8N1).
- [x] **Protocolo completo** 1.3 reconstruído da estática: frame de 5 bytes +
      XOR `0x02` (porte verificado contra o assembly, com vetor de regressão).
- [x] UI moderna (Slint, tema claro): cards de CPU / temperatura / fan, chip de
      status e dispositivo. Cadência fixa de **1 s** (igual ao original; sem
      seletor de frequência) e descoberta por serial.
- [x] **Backend de sensores no Windows (autossuficiente)**: temperatura e fan via
      um **sidecar LibreHardwareMonitor** (`kalkan-sensord`, .NET self-contained,
      lado a lado com o .exe), com fallback para **HWiNFO Shared Memory**. CPU/RAM
      load vêm do `sysinfo`. A UI se auto-eleva (UAC), como o original.
      ⚠️ Com a **Integridade de Memória (HVCI)** ligada, o driver do
      LibreHardwareMonitor não carrega e temp/fan ficam `--`; use HWiNFO ou PawnIO
      (driver compatível com HVCI). Ver [`docs/sensors-windows.md`](docs/sensors-windows.md).
- [ ] Backends de sensor para Linux (lm-sensors/hwmon) e macOS (SMC) — temp/fan
      `--` nessas plataformas por ora (CPU load é real).
- [ ] Envio ponta a ponta validado no hardware 1.3 (depende do cooler plugado;
      a cadência é sempre 1 s para não sobrecarregar o display).

## Próximo passo (depende do hardware)

Rodar `kalkan list` na máquina com o cooler 1.3 plugado e confirmar que a porta
dele aparece marcada com ★ (VID/PID casando). Com a porta certa, `open_display()`
abre e o envio de frames fica ativo ponta a ponta — o protocolo já está pronto e
testado offline.
