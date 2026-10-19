# Kalkan SmartMonitor **1.2** — notas de engenharia reversa

[English](protocol.md) | **Português (Brasil)**

> [!WARNING]
> **Status: experimental / ainda não implementado neste app / não testado em
> hardware.** Tudo abaixo vem de análise estática do binário do fabricante.
> Nenhum cooler 1.2 foi testado com isso, e este repositório ainda não fala
> esse protocolo (só implementa a [1.3](../v1.3/protocol-PT.md)).

Alvo analisado: `Kalkan PC SmartMonitor DZ V1.2.exe`, o app usado pelos coolers
da **linha Alpha Revisão 1** (segundo o fabricante: unidades vendidas antes de
jan/2026; veja [Qual versão eu tenho?](#solução-de-problemas--qual-versão-eu-tenho)).
SHA-256 do binário original:
`bacda52c244b23a383d375a91749803ef2b78e64c90f83a11f58e2d28de60f4d`.
É o binário que vem dentro do instalador oficial
`Kalkan PC SmartMonitor (Linha Alpha) V1.2.exe` (Inno Setup, SHA-256
`61a241988da69bb36fd2a75e22a38277454e87d84bf55d8efb573c38b1e314b4`).

Método: instalador desempacotado com `innoextract` (nunca executado), depois
análise estática com Ghidra 12.1.4 via REA (descompilação estilo Ghidra +
assembly x86-64). Os nomes de função abaixo (`FUN_…`) são os nomes padrão do
Ghidra, ou seja, os endereços das funções no binário original.

## O que difere da 1.3

| Aspecto | 1.2 | 1.3 |
|---|---|---|
| Transporte | **USB HID** (`hidapi`) apenas; as strings de `QSerialPort` não são referenciadas | Só serial (`QSerialPort`) |
| Seleção do aparelho | VID:PID **`0416:1100`** fixo no código | VID/PID `1F3A:0008` / `1A86:484A` fixos no código |
| Parâmetros seriais | — | 1 Mbps, 8N1, sem controle de fluxo |
| Handshake / seed | pedido de 64 bytes `01 81 E3 …`, a resposta traz a seed após o `0xE3` | nenhum |
| Frame | **64 bytes** `01 86 E0 idx temp fanHi fanLo cpuLoad 00…` | 5 bytes `74 00 temp fanHi fanLo` |
| Embaralhamento | bit a bit, semeado pelo handshake, do byte 3 em diante | XOR `0x02` do byte 2 em diante |
| `cpu_load` no frame | sim | não |
| Índice de frame | sim (1 byte, volta a 0 após 255) | não |
| Leitura de volta após cada escrita | sim (65 bytes, 2000 ms, só logada) | não |
| Checksum | nenhum | nenhum |

## Arquitetura (classes)

| Camada | Como o app do fabricante faz | Evidência |
|---|---|---|
| UI | Qt6 / QML, embutida (`qrc:/VTF_COM/main.qml`) | strings QML, `QQmlApplicationEngine::load` |
| Sensores | **HWiNFO** via `HWiNFO64.dll` (só Windows) | `config.dat` é uma DLL que importa `HWiNFO64.dll`; `SensorThread` |
| Transporte USB | `hidapi.dll` (classe `HidDeviceController`) | imports `hid_*`, RTTI `HidDeviceController` |
| Transporte serial | **nenhum ativo** | as strings `QSerialPort` / `WCOM` existem, mas nenhum caminho de código as referencia |

Layout do objeto `HidDeviceController` (offsets a partir do `this`):

| Offset | Campo |
|---|---|
| `+0x10` | `hid_device*` (handle aberto; 0 = fechado) |
| `+0x24` | `pid` (int) |
| `+0x28` | `vid` (int) |
| `+0x40` | `mFrameIndex` (byte, contador de frames) |
| `+0x41` | `scramblerSeed` (byte, obtido no handshake) |

## Abrir o dispositivo — `FUN_140002cb0` → `FUN_140002cf0` → `FUN_140001a70`

- `FUN_140002cb0` chama o open com as constantes literais **`0x416`, `0x1100`**
  (VID `0416`, PID `1100`). Esses IDs são **fixos no exe do fabricante**; não
  vêm de arquivo de configuração (isso substitui notas antigas que diziam o
  contrário).
- `FUN_140002cf0` guarda vid/pid no objeto e chama `FUN_140001a70`, que chama
  **`hid_open(vid, pid, NULL)`**: primeiro dispositivo com esses IDs, sem número
  de série, **sem filtro de usage page ou interface**.
- `0416` é um vendor ID genérico da Nuvoton, usado por muitos dispositivos sem
  relação entre si; um porte deve filtrar de forma mais estrita (por exemplo por
  usage page / interface / tamanho de report, quando forem conhecidos numa
  unidade real) em vez de abrir cegamente o primeiro que casar.

### `isOpen()` — `FUN_1400017f0`

Chama `hid_enumerate()` e percorre a lista comparando `vendor_id`/`product_id`
com o vid/pid guardados. Antes de enviar cada frame o app do fabricante refaz
essa checagem e **fecha o handle se o dispositivo não aparece mais na lista**
(detecção de desconexão).

### `writeData(QByteArray)` — `FUN_1400023e0` → `FUN_140002260`

Copia o `QByteArray` num buffer local, loga `[writeData] sent: <hex>` e chama o
envio bruto: `hid_set_nonblocking(dev, 0)` (bloqueante) + `hid_write`.

## Handshake — `FUN_140002cf0`, `FUN_140002740`

1. Monta o pedido de seed (`GenerateScramblerSeed`, `FUN_140002740`): 64 bytes,
   `[0]=0x01 [1]=0x81 [2]=0xE3`, resto `0x00`.
2. Envia via `writeData`.
3. Lê **65 bytes** com `hid_read_timeout(…, 2000)` (bloqueante, 2000 ms).
4. Procura o byte mágico **`0xE3`** no índice 0 (seed = byte 1) **ou** no
   índice 1 (seed = byte 2). O byte logo após o `0xE3` é a seed do scrambler,
   guardada em `+0x41`.
5. Se não achar o `0xE3`: loga `[open] Can't found 0xE3`, **fecha o handle e
   zera a seed, mas o open ainda retorna true**.

## Frame de dados e embaralhamento

### Frame — `GenerateShowDevInfo`, `FUN_140002800`

String de debug de origem: `"[GenerateShowDevInfo] temp: … fan: … cpuload: … mFrameIndex: …"`.

| offset | conteúdo | no fio |
|---|---|---|
| 0 | `0x01` | literal |
| 1 | `0x86` | literal |
| 2 | `0xE0` (comando "mostrar info") | literal |
| 3 | `mFrameIndex` | embaralhado |
| 4 | `temp` (°C, 1 byte) | embaralhado |
| 5 | `fan` byte alto (big-endian) | embaralhado |
| 6 | `fan` byte baixo | embaralhado |
| 7 | `cpu_load` (%, 1 byte) | embaralhado |
| 8..63 | `0x00` | embaralhado |

Sem checksum.

**Índice de frame:** um byte em `+0x40`, escrito no byte 3 e depois
incrementado (volta a 0 após 255). O app do fabricante **não** o zera ao
reabrir. (Um porte que o zera a cada novo handshake é uma escolha razoável; o
valor inicial não foi confirmado, provavelmente 0.)

### Scrambler — `FUN_140002a20`

Strings de debug de origem: `[Scrambler] init/src/out`. Conferido contra o
assembly (aritmética de 8 bits com *wraparound*). Os bytes 0–2 ficam literais;
do byte 3 em diante:

```
seed = <seed do handshake>          // byte, passada POR VALOR
for pos in 3..len:
    acc = 0                         // byte
    for bit in 0..8:
        v = ((src[pos] << bit) & 0x80) + ((seed & 0xf0) << 3) + (seed & 0x80)   // u8
        acc  = acc  + (v >> bit)                                                // u8
        seed = seed*2 + (v >> 6)                                                // u8
    out[pos] = acc
```

O scrambler recebe a seed **por valor** e só altera uma cópia local. Então:

- dentro de um frame, a seed evolui bit a bit e passa de uma posição de byte
  para a seguinte;
- **todo frame recomeça da seed do handshake**; a seed guardada só muda num
  novo handshake.

### Laço de transmissão — `processTrans`, `FUN_140008030`

A cada ciclo do timer:

1. Checagem `isOpen()` (ver acima); fecha o handle se o dispositivo sumiu.
2. Lê temp (1 byte) e fan (2 bytes) das tabelas de sensor, monta o frame
   (`GenerateShowDevInfo`), embaralha, envia (`send`, `FUN_140003180` →
   `writeData`). Loga `[process trans] source:` / `[process trans] scramb:`.
3. **Lê de volta 65 bytes** (bloqueante, 2000 ms). A resposta é **só logada**:
   não é validada, não é usada para trocar a seed, não há nova tentativa. Um
   dispositivo que não responde trava cada ciclo por até 2 s. Se o dispositivo
   realmente precisa dessa leitura, só dá para saber com hardware.

## Vetores de referência

Entrada `temp=45`, `fan=1500`, `cpu_load=20`, seed reiniciada a cada frame.
Frames completos de 64 bytes no fio (hex):

| seed | idx | frame |
|---|---|---|
| `00` | 0 | `0186e0002fa0947b42afa51b8fe1de592044c5d6c19a9cf6855f4a371fc3bcb240898bad833539ed0abe946e3f8779648113175b066a73da157d28dc7f0ef2c9` |
| `00` | 1 | `0186e0013cb7cf7d28dc7f0ef2c902262eb60cd4e7b42afa51b8fe1de592044c5d6c19a9cf6855f4a371fc3bcb240898bad833539ed0abe946e3f87796481131` |
| `FF` | 0 | `0186e00756c4156855f4a371fc3bcb240898bad833539ed0abe946e3f8779648113175b066a73da157d28dc7f0ef2c902262eb60cd4e7b42afa51b8fe1de5920` |
| `FF` | 1 | `0186e00645d34e6e3f8779648113175b066a73da157d28dc7f0ef2c902262eb60cd4e7b42afa51b8fe1de592044c5d6c19a9cf6855f4a371fc3bcb240898bad8` |
| `2A` | 0 | `0186e07d077ceb75b066a73da157d28dc7f0ef2c902262eb60cd4e7b42afa51b8fe1de592044c5d6c19a9cf6855f4a371fc3bcb240898bad833539ed0abe946e` |
| `2A` | 1 | `0186e07c146bb073da157d28dc7f0ef2c902262eb60cd4e7b42afa51b8fe1de592044c5d6c19a9cf6855f4a371fc3bcb240898bad833539ed0abe946e3f87796` |

Vetor de regressão (dos testes do repositório 1.2): seed `0x2A`, idx 1,
`temp=40`, `fan=1200`, `cpu_load=55` → 16 primeiros bytes
`01 86 e0 7c 11 35 83 01 13 17 5b 06 6a 73 da 15`.

## Porte em Rust

**Ainda não implementado neste app.** O repositório 1.2 anterior tem um porte
em Rust do handshake, do frame e do scrambler; trazê-lo para cá (atrás da
seleção `Firmware::V1_2`) está planejado, e os vetores acima devem ser os testes
dele.

## Dependente de hardware (questões em aberto)

Só uma unidade Revisão 1 real responde:

- As unidades reais enumeram como `0416:1100`? Qual usage page / interface, e
  qual o tamanho dos reports de entrada/saída (o app escreve 64 bytes e lê 65)?
- Como é a resposta real do handshake (posição do `0xE3`, valor da seed)?
- O dispositivo precisa da leitura de volta após cada escrita? Existe intervalo
  mínimo entre envios?
- O que exatamente a tela antiga mostra (só temperatura, como na Revisão 2?)
- Linux: permissões de `hidraw` (provavelmente será preciso uma regra udev).
- macOS: se o dispositivo abre sem driver ou permissão especial.

## Solução de problemas / qual versão eu tenho

O `LEIA-ME.txt` do fabricante (que acompanha os instaladores da linha Alpha)
diz qual software corresponde a qual hardware:

> "Kalkan PC SmartMonitor (Linha Alpha Rev.2) 1.3: Produtos da linha Alpha
> comprados a partir de Janeiro/2026 são da Revisão 2."
>
> "Kalkan PC SmartMonitor (Linha Alpha) V1.2: Produtos da linha Alpha vendidos
> antes de Janeiro/2026 são da Revisão 1."

A etiqueta da caixa identifica a unidade: o serial logo acima do código de
barras traz o SKU seguido da data de fabricação no formato ano/mês/dia (exemplo
do fabricante: `KLK00047 - 20251106XXX`).

Se o display **fica travado em 88°**, a versão errada está instalada:

> "Caso execute o software e o mesmo não mostre a temperatura no visor, ficando
> travado em 88°, desinstale a versão atual instalada e tente instalar a outra
> versão disponível nesta pasta."

O fabricante também pede para instalar e executar o software dele como
Administrador. Neste app, o mesmo sintoma num Alpha indica que o cooler
provavelmente usa o outro firmware: se o `kalkan list` não mostra nenhuma porta
1.3 (`1F3A:0008` / `1A86:484A`), provavelmente é uma unidade Revisão 1 / 1.2,
que ainda não é suportada.
