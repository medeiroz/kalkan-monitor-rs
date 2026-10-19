# Kalkan Monitor RS: monitor para watercoolers Kalkan Alpha 240/360/360-W e Delta V2 240/360

[English](README.md) | **Português (Brasil)**

Monitor gratuito, open source e multiplataforma (Windows / macOS / Linux) para
os watercoolers (AIO, refrigeração líquida) **Kalkan Alpha** e **Delta V2**. Ele
lê a **temperatura da CPU** e a mostra no display do cooler, como alternativa
ao software oficial Kalkan PC SmartMonitor.

<table>
  <tr>
    <td align="center"><img src="docs/img/app-display.png" alt="Kalkan Monitor, página Display com a prévia do cooler Alpha: tela redonda mostrando a temperatura da CPU e os seletores de fonte de temperatura, RPM e uso" width="420"><br><sub>Alpha</sub></td>
    <td align="center"><img src="docs/img/app-display-delta-v2.png" alt="Kalkan Monitor, página Display com a prévia do cooler Delta V2: temperatura e velocidade da ventoinha" width="420"><br><sub>Delta V2</sub></td>
  </tr>
</table>

## Download

Baixe a versão mais recente na
**[página de Releases](https://github.com/medeiroz/kalkan-monitor-rs-v1.3/releases/latest)**
(abra **Assets** e escolha o arquivo do seu sistema):

| Sistema | Arquivo para baixar | Observações |
| --- | --- | --- |
| Windows (64 bits) | `KalkanMonitor-Setup-<versão>.exe` (instalador) | Inclui tudo, inclusive o driver de sensores |
| Linux (Debian, Ubuntu e derivadas, 64 bits) | `kalkan-monitor_<versão>_amd64.deb` | Inclui a regra de permissão USB |
| macOS 11 ou posterior (Apple Silicon e Intel) | `KalkanMonitor-<versão>.dmg` | App universal |

**Windows.** Execute o instalador e siga os passos. Ao abrir o app, ele pede
**permissão de administrador** (o aviso do UAC do Windows): é necessária para ler
a temperatura da CPU e a velocidade da ventoinha. Aceite. Se você recusar, o app
abre mesmo assim, mas mostra um aviso e não consegue ler os sensores.

**Linux.** Dê dois cliques no `.deb` (ou rode `sudo apt install ./kalkan-monitor_<versão>_amd64.deb`).
O pacote já configura a permissão para falar com o display: **desconecte e
reconecte o cabo USB do cooler** depois de instalar. Outras distribuições:
veja [Instalar (Linux)](#instalar-linux).

**macOS.** Abra o `.dmg` e arraste o **Kalkan Monitor** para **Aplicativos**;
abra-o a partir de lá. Na primeira vez, o macOS pode dizer que o app é de um
"desenvolvedor não identificado": clique com o botão direito no app, escolha
**Abrir** e depois **Abrir** de novo (detalhes em [Instalar (macOS)](#instalar-macos)).

## Coolers suportados

Lista oficial da Kalkan (veja [kalkan.com.br/cooler](https://www.kalkan.com.br/cooler)):

| Cooler | Código do modelo | Observações |
| --- | --- | --- |
| [Kalkan Alpha 240](https://www.kalkan.com.br/produto/alpha-240) | KLK047 | Watercooler AIO de 240 mm, LCD redondo que mostra apenas a temperatura da CPU. Apenas Alpha Rev.2 (firmware 1.3) |
| [Kalkan Alpha 360](https://www.kalkan.com.br/produto/alpha-360) | KLK048 | Watercooler AIO de 360 mm, LCD redondo que mostra apenas a temperatura da CPU. Apenas Alpha Rev.2 (firmware 1.3) |
| [Kalkan Alpha 360-W](https://www.kalkan.com.br/produto/alpha-360w) | KLK049 | Versão branca do Alpha 360. Apenas Alpha Rev.2 (firmware 1.3) |
| [Kalkan Delta V2 240](https://www.kalkan.com.br/produto/delta-v2-240) | KLK066 | Watercooler AIO de 240 mm, compatível por protocolo; ainda não testado em hardware |
| [Kalkan Delta V2 360](https://www.kalkan.com.br/produto/delta-v2-360) | KLK067 | Watercooler AIO de 360 mm, compatível por protocolo; ainda não testado em hardware |

**Alpha 240 / 360 / 360-W:** o **display redondo mostra apenas a temperatura da
CPU**. RPM da ventoinha e uso (%) não aparecem na tela (o app ainda os envia no
frame do protocolo, mas esses modelos não os exibem).

**Delta V2 240 / 360:** compatível por protocolo (mesmo software oficial);
**ainda não testado em hardware**. Verificamos de forma estática que o software
oficial para Windows "Kalkan PC SmartMonitor (Linha Delta V2) V1.3" é exatamente
o mesmo binário (mesmo SHA-256) do qual este protocolo foi derivado, então os
IDs USB (`1F3A:0008` / `1A86:484A`), as configurações da serial e o formato do
frame são idênticos. O que o display do Delta V2 exibe (formato e campos) não
foi verificado; o software oficial envia temperatura e velocidade da ventoinha.
Se você tem um Delta V2, relatos são bem-vindos: abra uma issue com a saída de
`kalkan list`.

## Recursos

- Mostra a **temperatura da CPU** no display do cooler, atualizada a cada segundo.
- Dashboard com temperatura da CPU, velocidade da ventoinha e uso da CPU ao vivo.
- **Escolha das fontes**: defina qual sensor alimenta os campos de temperatura e
  RPM (Auto, Max CPU/GPU ou qualquer sensor de temperatura/ventoinha do seu PC).
- **Prévia da tela** do cooler, para Alpha e Delta V2.
- **Ícone na bandeja** (barra de menus no macOS): fechar a janela mantém o app
  rodando e atualizando o display.
- **Iniciar com o sistema**: vem ativado na primeira execução, e você pode
  desligar em **Settings**.

## Como usar

1. Conecte o cabo USB do cooler ao PC (a conexão USB do próprio cooler, a mesma
   usada pelo software oficial) e feche o software oficial da Kalkan, se estiver
   instalado.
2. Abra o **Kalkan Monitor**. O status no canto inferior esquerdo deve dizer
   **Connected**.
3. Na página **Display**, confira as fontes de temperatura e RPM (**Auto**
   serve para a maioria dos PCs) e escolha o modelo do seu cooler (Alpha ou
   Delta V2) para a prévia.
4. Pronto: pode fechar a janela, o app continua rodando na bandeja. Ajuste
   **Settings** se não quiser que ele inicie com o sistema.

## Solução de problemas / FAQ

**O display do cooler fica travado em "88°".** Você provavelmente tem a outra
revisão do hardware. Este app suporta o Alpha **Rev.2** (firmware 1.3, vendido a
partir de janeiro de 2026) e o Delta V2. O Alpha **Rev.1** (vendido antes de
janeiro de 2026, firmware 1.2) usa outro protocolo, ainda não suportado.

**O app diz que o cooler não foi encontrado.** Confira se o cabo USB está
conectado, tente outra porta USB e **feche o software oficial da Kalkan** ou
qualquer outro programa que possa estar usando o display. Depois reconecte o cabo.

**O Windows pede permissão de administrador / mostra um aviso.** O app precisa
dela para ler os sensores de temperatura. Reinicie o app e aceite o aviso. Ele
inicia com o sistema sem pedir de novo.

**Linux: "Permission denied" ao abrir o display.** Instale o `.deb` (ele
adiciona a regra de permissão) e reconecte o cooler. Sem a regra, rode
`sudo usermod -aG dialout $USER` (`uucp` no Arch) e faça login de novo. Detalhes
em [Instalar (Linux)](#instalar-linux).

**macOS: o display está conectado mas nenhuma porta é encontrada.** O chip USB
do display (CH34x) pode precisar de um driver em versões mais antigas do macOS:
instale o CH34xVCPDriver da WCH em [wch-ic.com](https://www.wch-ic.com/), libere-o
em **Ajustes do Sistema → Privacidade e Segurança** e reconecte o cooler.
Detalhes em [Instalar (macOS)](#instalar-macos).

**Meu Delta V2 é suportado?** É compatível por protocolo, mas ainda não foi
testado em hardware real. Relatos são bem-vindos (abra uma issue com a saída de
`kalkan list`).

---

# Detalhes técnicos

As seções abaixo são para os curiosos e para quem quer contribuir.

É uma reimplementação em Rust do driver + interface do protocolo **Kalkan
SmartMonitor 1.3** usado por esses coolers. Projeto novo — não é o
`.exe` original traduzido; é uma reconstrução baseada em engenharia reversa do
comportamento dele (ver [`docs/v1.3/protocol-PT.md`](docs/v1.3/protocol-PT.md)), com uma UI
moderna nova por cima.

## Por que existe

O software original é Windows-only: usa HWiNFO para sensores e um app Qt/QML
para a UI (além de, na versão instalada, desligar o UAC — removido à parte).
Aqui a lógica útil — ler sensores e falar com o display do cooler por serial —
vira um núcleo portátil, e a interface é refeita do zero.

## O que é diferente na 1.3

Comparada à 1.2, a 1.3 é **mais simples** (detalhes em `docs/v1.3/protocol-PT.md`):

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
linux/          # arquivos do pacote .deb: regra udev, .desktop, ícones, scripts
macos/          # empacotamento .app/.dmg: modelo do Info.plist + build.sh
docs/
  README.md      # índice da documentação
  v1.3/
    protocol-PT.md # achados da engenharia reversa (protocolo 1.3 reconstruído)
  v1.2/
    protocol-PT.md # notas do protocolo 1.2 (HID): experimental, ainda não implementado
```

## Instalar (Windows)

Baixe o `KalkanMonitor-Setup-<versão>.exe` em
[Releases](https://github.com/medeiroz/kalkan-monitor-rs-v1.3/releases). Ele instala o
dashboard, o sidecar de sensores e o driver **PawnIO** (necessário para ler
temperatura/fan, inclusive com a Integridade de Memória ligada). Nada mais a
instalar à parte.

Para gerar o instalador localmente: `./windows/installer/build.ps1 -Version 0.1.0`
(precisa de Rust, .NET 8 SDK e Inno Setup 6).

### Bandeja e iniciar com o sistema

- Fechar a janela mantém o app rodando na **bandeja** (o display continua sendo
  atualizado). Clique com o botão esquerdo no ícone para reabrir; botão direito
  → **Close monitor** encerra. Abrir o app de novo enquanto ele roda só mostra
  a janela.
- **Settings → Start with the operating system** registra o app para iniciar
  escondido na bandeja no login. Vem ativado na primeira execução (uma vez;
  desligue em Settings e ele continua desligado):
  - Windows: uma tarefa agendada (`Kalkan Monitor`, no logon, privilégios mais
    altos), então ele inicia elevado sem prompt do UAC. O desinstalador a remove.
  - macOS: um LaunchAgent em `~/Library/LaunchAgents` que executa o binário
    dentro do `Kalkan Monitor.app` instalado, então ative-o a partir da cópia em
    Aplicativos (ver [Instalar (macOS)](#instalar-macos)).
  - Linux: uma entrada em `~/.config/autostart`. A bandeja precisa de um host
    StatusNotifier (ver [Instalar (Linux)](#instalar-linux)); sem um, fechar a
    janela encerra o app e a janela abre no login em vez de ficar escondida.

### Publicar uma versão

As Releases são automáticas: todo push na `main` cujos builds de Windows, Linux e macOS passem
faz o GitHub Actions criar a tag `v<MAJOR.MINOR>.<número da execução>` (MAJOR.MINOR vem da versão
no `Cargo.toml`) e a Release. Nunca crie tags manualmente. Para mudar MAJOR.MINOR, altere `version`
em `[workspace.package]`.

O GitHub Actions (`.github/workflows/release.yml`) gera o instalador do Windows,
o `.deb` do Linux e o `.dmg` do macOS e cria a Release com eles anexados. Todo push/PR roda os testes em Windows, Linux e macOS
(`ci.yml`).

## Instalar (Linux)

Baixe o `kalkan-monitor_<versão>_amd64.deb` em
[Releases](https://github.com/medeiroz/kalkan-monitor-rs-v1.3/releases) e instale:

```bash
sudo apt install ./kalkan-monitor_<versão>_amd64.deb
```

O pacote instala o app `kalkan-monitor`, uma entrada de menu, um ícone e a
**regra udev** que permite abrir a porta serial do display. Outras distros:
compile do código-fonte (abaixo) e copie
[`linux/udev/60-kalkan-monitor.rules`](linux/udev/60-kalkan-monitor.rules) para
`/etc/udev/rules.d/`.

**Acesso à porta serial.** O display aparece como `/dev/ttyUSB*` ou
`/dev/ttyACM*`, que usuários comuns não conseguem abrir por padrão. A regra udev
dá acesso ao usuário logado na máquina (`TAG+="uaccess"`), com o grupo `dialout`
como alternativa. Depois de instalá-la, **reconecte o display** (o pacote também
recarrega as regras). Sem a regra, entre no grupo e faça login de novo:

```bash
sudo usermod -aG dialout $USER   # "uucp" no Arch
```

Se a porta não puder ser aberta, o app avisa no banner de erro e repete essa
solução.

Os **sensores** vêm da interface hwmon do kernel (`/sys/class/hwmon`): CPU, GPU,
ventoinhas da placa, NVMe, memória e coolers líquidos, agrupados por categoria
nos seletores de fonte, com escolhas Auto para a temperatura da CPU e para a
ventoinha/bomba. Não precisa de driver nem de root, mas o módulo de kernel
correspondente precisa estar carregado (`k10temp`/`coretemp`,
`nct6775`/`it87` para a placa-mãe, ...); o `sensors-detect` do lm-sensors ajuda
a achá-lo. O driver proprietário da NVIDIA não tem hwmon: a temperatura da GPU
vem do `nvidia-smi`, quando instalado. Ver [`docs/sensors-linux-PT.md`](docs/sensors-linux-PT.md).

**Bandeja.** O ícone é um StatusNotifierItem (sem precisar de GTK nem de
bibliotecas appindicator). Funciona direto no KDE Plasma, XFCE, Cinnamon, MATE e
similares. O **GNOME** precisa da extensão *AppIndicator and KStatusNotifierItem
Support* (o Ubuntu já a traz ativada). Sem um host StatusNotifier o app não
mostra ícone de bandeja: fechar a janela encerra o app, e iniciar com `--tray`
(autostart) abre a janela em vez de rodar invisível.

Abrir o app enquanto ele já roda só mostra a janela (um socket em
`$XDG_RUNTIME_DIR` mantém uma única instância).

## Instalar (macOS)

Baixe o `KalkanMonitor-<versão>.dmg` em
[Releases](https://github.com/medeiroz/kalkan-monitor-rs-v1.3/releases), abra-o
e arraste o **Kalkan Monitor** para **Aplicativos**. É um app universal (Apple
Silicon e Intel) para macOS 11 ou posterior. Execute-o a partir de Aplicativos,
não da imagem de disco, para que "iniciar com o sistema" o encontre.

**Primeira execução (Gatekeeper).** O app tem assinatura ad hoc e não é
notarizado (isso exige uma conta paga de desenvolvedor Apple), então o macOS diz
que ele é de um "desenvolvedor não identificado" ou que não pode ser aberto.
Aprove uma vez:

- clique com o botão direito (ou Control-clique) em **Kalkan Monitor** em
  Aplicativos → **Abrir** → **Abrir**; ou
- no macOS 15 Sequoia e posteriores, onde esse atalho não existe mais: tente
  abrir o app, vá em **Ajustes do Sistema → Privacidade e Segurança**, role até o
  fim e clique em **Abrir Mesmo Assim** ao lado do aviso do Kalkan Monitor; ou
- no terminal: `xattr -dr com.apple.quarantine "/Applications/Kalkan Monitor.app"`.

**Barra de menus.** O app coloca um ícone de termômetro na barra de menus (uma
imagem monocromática de template, clara ou escura conforme a barra). Clique nele
para **Open Kalkan Monitor** e **Close monitor**. Fechar a janela (botão
vermelho ou ⌘W) só a esconde: o app continua enviando ao display pela barra de
menus, e clicar no ícone dele no Dock traz a janela de volta (melhor esforço: se
sua versão do macOS ignorar esse gancho, use o ícone da barra de menus). **Close
monitor** ou ⌘Q encerra de verdade. Abrir o app de novo enquanto ele roda só
mostra a janela (um socket em `$TMPDIR` mantém uma única instância).
**Settings → Start with the operating system** cria um LaunchAgent que o inicia
escondido na barra de menus no login (`--tray`).

**Sensores.** Sem root e sem driver. No Apple Silicon as temperaturas vêm dos
sensores HID do IOKit (die da CPU, GPU, SSD, bateria), com a média do die da CPU
como Auto; nos Macs Intel, do SMC (`TC0P`, `TG0D`, ...). As ventoinhas vêm do
SMC; um Mac sem ventoinha (MacBook Air) não tem nenhuma, então o campo de RPM
mostra `--`. Ver [`docs/sensors-macos-PT.md`](docs/sensors-macos-PT.md).

**Driver serial (CH34x).** O chip USB-serial do display é da família CH34x da
WCH. Versões recentes do macOS trazem um driver para os mais comuns, e o display
aparece como `/dev/cu.usbserial-*` (ou `/dev/cu.wchusbserial*`). Versões mais
antigas, e algumas variantes do chip, precisam do CH34xVCPDriver da WCH
([wch-ic.com](https://www.wch-ic.com/)); depois de instalá-lo, libere-o em
**Ajustes do Sistema → Privacidade e Segurança** e reconecte o display. Se o
display está conectado mas o app não acha uma porta, o banner de erro diz se o
display não foi detectado (cabo) ou foi detectado sem porta (driver).

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

Build no Linux (precisa de `libudev-dev`, `libfontconfig1-dev` e
`libxkbcommon-dev` no Debian/Ubuntu) e o pacote (`cargo install cargo-deb`):

```bash
cargo build --release -p kalkan-ui
cargo deb -p kalkan-ui              # -> target/debian/kalkan-monitor_*.deb
```

Build no macOS (precisa das Xcode command line tools) e do app/imagem de disco:

```bash
./macos/build.sh 0.1.0              # -> target/macos/KalkanMonitor-0.1.0.dmg (universal)
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
- [x] **Direitos de administrador (Windows)**: ao iniciar, o app pede
      elevação (UAC), pois o driver de sensores e a tarefa agendada de
      inicialização automática precisam de um processo elevado. Se você
      recusar, ele continua sem isso e mostra um aviso em Dispositivo e
      Configurações; reinicie o app e aceite o prompt.
- [x] **Escolha de fonte**: define qual sensor alimenta os campos de temperatura e RPM do display (Auto, Max CPU/GPU ou qualquer sensor de temp/ventoinha); salva em `%APPDATA%\KalkanMonitor\settings.json`. Veja [`docs/sensors-windows-PT.md`](docs/sensors-windows-PT.md).
- [x] **Instalador** (Inno Setup, PawnIO embutido) e **CI/CD** no GitHub Actions.
- [x] **Linux**: sensores via hwmon (+ `nvidia-smi`), bandeja por StatusNotifier
      com alternativa quando não há host, instância única, regra udev e `.deb`.
      Ver [`docs/sensors-linux-PT.md`](docs/sensors-linux-PT.md). Escrito sem uma
      máquina Linux à mão: testado por testes unitários sobre uma árvore hwmon
      falsa, então relatos de hardware real são bem-vindos.
- [x] **macOS**: sensores via IOKit (sensores HID de temperatura no Apple
      Silicon, SMC no Intel, ventoinhas pelo SMC), ícone na barra de menus,
      instância única, dicas sobre o driver serial e um `.app` universal num
      `.dmg`. Ver [`docs/sensors-macos-PT.md`](docs/sensors-macos-PT.md). Escrito
      sem um Mac à mão: a lógica pura tem testes unitários e o código do macOS é
      verificado pelo compilador para as duas arquiteturas, mas só rodar em
      hardware real (ou o job de CI do macOS) prova que funciona, então relatos
      são bem-vindos.
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
