# Kalkan Monitor RS

[English](README.md) | **Português (Brasil)**

> [!IMPORTANT]
> **Este repositório é da versão 1.3 do Kalkan PC SmartMonitor**, usada nos
> produtos comprados **a partir de janeiro/2026**. A versão 1.2 (produtos mais
> antigos) usa um protocolo diferente e **não é compatível** com este código —
> e vice-versa. Fork adaptado do repositório 1.2.

Reimplementação multiplataforma (Windows / macOS / Linux), em Rust, do driver +
interface do watercooler **Kalkan PC SmartMonitor 1.3**. Projeto novo — não é o
`.exe` original traduzido; é uma reconstrução baseada em engenharia reversa do
comportamento dele (ver [`docs/protocol-PT.md`](docs/protocol-PT.md)), com uma UI
moderna nova por cima.

## Por que existe

O software original é Windows-only: usa HWiNFO para sensores e um app Qt/QML
para a UI (além de, na versão instalada, desligar o UAC — removido à parte).
Aqui a lógica útil — ler sensores e falar com o display do cooler por serial —
vira um núcleo portátil, e a interface é refeita do zero.

## O que é diferente na 1.3

Comparada à 1.2, a 1.3 é **mais simples** (detalhes em `docs/protocol-PT.md`):

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
  kalkan-cli/    # utilitário `kalkan` (list, sensors, frame, send, hwinfo, hid)
  kalkan-ui/     # dashboard moderno (Slint, tema claro) → bin `kalkan-monitor`
sidecar/
  kalkan-sensord/ # sensores no Windows via LibreHardwareMonitor (.NET)
windows/
  installer/    # instalador Windows (Inno Setup) + build.ps1
docs/
  protocol-PT.md # achados da engenharia reversa (protocolo 1.3 reconstruído)
```

## Instalar (Windows)

Baixe o `KalkanMonitor-Setup-<versão>.exe` em
[Releases](https://github.com/medeiroz/kalkan-monitor-rs-v1.3/releases). Ele instala o
dashboard, o sidecar de sensores e o driver **PawnIO** (necessário para ler
temperatura/fan, inclusive com a Integridade de Memória ligada). Nada mais a
instalar à parte.

Para gerar o instalador localmente: `./windows/installer/build.ps1 -Version 0.1.0`
(precisa de Rust, .NET 8 SDK e Inno Setup 6).

### Publicar uma versão

```bash
git tag v0.1.0 && git push origin v0.1.0
```

O GitHub Actions (`.github/workflows/release.yml`) gera o instalador e cria a
Release com ele anexado. Todo push/PR roda os testes em Windows, Linux e macOS
(`ci.yml`).

## Compilar e rodar

```bash
cargo build
cargo run -p kalkan-ui              # abre o dashboard
./target/debug/kalkan list          # lista portas seriais — acha o cooler (★)
./target/debug/kalkan sensors       # leitura de sensores atual
./target/debug/kalkan frame         # monta+embaralha um frame e mostra o hex
./target/debug/kalkan send 42 1234 15  # envia temp/fan fixos ao display por 15 s (teste de hardware)
./target/debug/kalkan hwinfo        # (admin) leituras do motor HWiNFO do app original
cargo test                          # testes do protocolo + sensores (vetores de regressão)
```

Sensores no Windows (em dev): compile o sidecar .NET e tenha o PawnIO instalado
(o instalador cuida disso; o LibreHardwareMonitor também o instala). Se o app
original da Kalkan estiver instalado, o `config.dat` dele serve de fallback.
Ver [`docs/sensors-windows-PT.md`](docs/sensors-windows-PT.md):

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
- [x] **Backend de sensores no Windows**: temperatura e fan, em ordem, do
      **sidecar LibreHardwareMonitor + PawnIO** (`kalkan-sensord`; funciona com
      HVCI), do **motor HWiNFO do app original** (`config.dat`, se instalado) e do
      **HWiNFO Shared Memory**. CPU/RAM load vêm do `sysinfo`. A UI se auto-eleva
      (UAC), como o original. Ver [`docs/sensors-windows-PT.md`](docs/sensors-windows-PT.md).
- [x] **Escolha de fonte**: define qual sensor alimenta os campos de temperatura e RPM do display (Auto, Max CPU/GPU ou qualquer sensor de temp/ventoinha); salva em `%APPDATA%\KalkanMonitor\settings.json`. Veja [`docs/sensors-windows-PT.md`](docs/sensors-windows-PT.md).
- [x] **Instalador** (Inno Setup, PawnIO embutido) e **CI/CD** no GitHub Actions.
- [ ] Backends de sensor para Linux (lm-sensors/hwmon) e macOS (SMC) — temp/fan
      `--` nessas plataformas por ora (CPU load é real).
- [x] **Envio ponta a ponta validado no hardware 1.3**: cooler em `1A86:484A`
      (COM3), `kalkan send 42 1234` mostrou 42 °C / 1234 RPM no display. O
      display não responde nada pela serial. Cadência sempre 1 s.
- [x] **Dashboard envia ao display** com sensores reais: abre a porta (e reabre
      se cair) e manda temp/fan a cada 1 s. Validado num Ryzen 7 9800X3D com HVCI
      ligado (CPU Die average / CPUFANIN0).

## Testando o display

```bash
./target/debug/kalkan list             # a porta do cooler aparece com ★
./target/debug/kalkan send 42 1234 30  # o display deve mostrar 42 °C e 1234 RPM
```

Se nada mudar, feche o app original do Kalkan (ele pode estar segurando a porta).
