# Sensores no macOS (temperatura e fan)

[English](sensors-macos.md) | **Português (Brasil)**

Como no Linux, o macOS não precisa de driver nosso nem de root: o IOKit entrega
as leituras dos sensores a qualquer usuário. O `MacSource`
(`crates/kalkan-core/src/macos.rs`, com o FFI em `macos/iokit.rs`) lê isso a
cada segundo; CPU/RAM load vêm do `sysinfo`, como nas outras plataformas. Há
duas fontes, conforme o Mac:

| Mac | Temperaturas | Ventoinhas |
|---|---|---|
| Apple Silicon (M1 em diante) | sensores HID de temperatura do IOKit | SMC |
| Intel | SMC | SMC |

As duas são testadas em todo Mac; o que a máquina não tem é ignorado. Se o
IOKit não puder ser acessado de jeito nenhum, o app volta à estimativa de
temperatura da CPU do `sysinfo` (e fica sem lista de sensores).

## Apple Silicon: sensores HID

Os sensores de temperatura são serviços HID do sistema de eventos privado
(`IOHIDEventSystemClient`, com `PrimaryUsagePage` `0xff00`, `PrimaryUsage` 5 e
evento `kIOHIDEventTypeTemperature`). Cada serviço é nomeado pela propriedade
`Product`, que o app converte num nome amigável e numa categoria:

| `Product` | Nome mostrado | Categoria |
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
| qualquer outro | o texto do `Product` | Other |

`PMU tcal` é um valor de calibração, não uma temperatura, e é descartado.
Leituras fora de 0..150 °C (sensores desconectados informam valores absurdos)
são ignoradas. Quais sensores existem depende da geração do chip; os `tdie` e
`tdev` existem em todos, os `MTR` em chips mais novos.

Os ids são `iohid/<Product>`, por exemplo `iohid/PMU tdie1`; é o que o
`~/Library/Application Support/KalkanMonitor/settings.json` guarda para um
sensor escolhido. (Se dois serviços tiverem o mesmo `Product`, o segundo recebe
`#2`.)

## Intel: chaves do SMC

O SMC é o serviço IOKit `AppleSMC`, aberto como user client. Uma chave é lida
com duas chamadas (informações da chave e depois os bytes), pela struct
`SMCKeyData` de 80 bytes. Na inicialização o app testa uma tabela fixa de
chaves e guarda as que existem:

| Chaves | Nome mostrado | Categoria |
|---|---|---|
| `TC0P`, `TC0D`, `TC0E`, `TC0F`, `TC0H`, `TCXC` | CPU Proximity / Die / Die (virtual) / Die (filtered) / Heatsink / PECI | CPU |
| `TC1C` a `TC8C` | `CPU Core N` | CPU |
| `TG0P`, `TG0D`, `TG0H` | GPU Core (proximity) / Core (diode) / Heatsink | GPU |
| `Tm0P`, `TM0P` | Memory Proximity / Slot Proximity | Memory |
| `TH0P` | Drive Proximity | Storage |
| `TPCD`, `TN0P`, `TN0D` | Platform Controller Hub, Northbridge | Motherboard |
| `TB0T`, `TB1T` | Battery 1 / 2 | Other |
| `TA0P`, `Ts0P`, `TW0P`, `TL0P`, `Th0H` a `Th2H` | Ambient, Palm Rest, Wi-Fi, Display, Heatpipes | Other |

Os ids são `smc/<CHAVE>`, por exemplo `smc/TC0P`. O valor 0 significa "não há
sensor ali" e é ignorado. Os nomes "Core" da GPU são de propósito: o **Max
CPU/GPU** procura uma temperatura de core ou hot spot da GPU.

Os valores vêm nos formatos próprios do SMC, decodificados em `decode_smc`:

| Tipo | Significado |
|---|---|
| `sp78` | ponto fixo com sinal, 7 bits inteiros + 8 fracionários, big-endian (a maioria das temperaturas) |
| `fpe2` | ponto fixo sem sinal, 14 + 2 bits, big-endian (RPM de ventoinha em Macs mais antigos) |
| `flt ` | float de 32 bits little-endian (RPM e temperaturas em Macs mais novos) |
| `ui8 `, `ui16`, `ui32`, `si8 `, `si16` | inteiros big-endian |
| `spXY`, `fpXY` | a mesma família de ponto fixo em outras larguras |

## Ventoinhas

`FNum` informa quantas ventoinhas há; para cada uma, `F<n>Ac` é a velocidade
atual em RPM e `F<n>ID` (quando existe) o nome (`Left side`, `Right side`). Elas
aparecem como `Fan 1 (Left side)`, com o id `smc/F0Ac` para a primeira.
Funciona em Macs Intel e em Macs Apple Silicon que têm ventoinha (MacBook Pro,
Mac Studio, iMac, ...). Um Mac sem ventoinha (MacBook Air) informa zero: o
seletor de RPM fica vazio e o campo de RPM do display permanece em 0. Um Mac
com ventoinha as para quando está frio, então 0 RPM é uma leitura real e é
enviada como tal.

## Categorias

O tipo de hardware decide o grupo nos seletores (o vocabulário que o
`naming.rs` classifica): `Cpu`, `GpuApple`/`Gpu`, `Storage`, `Memory`,
`Motherboard` (ventoinhas, chipset), `Battery` e `Other`. Bateria e os demais
sensores aparecem em **Other**.

## Escolhas Auto

**Temperatura** (vale a primeira que existir):

1. Apple Silicon: a **média dos sensores de die da CPU** (`CPU Die (average)`,
   id `iohid/cpu-die-average`): a média de todos os `PMU tdie*`; em chips sem
   eles, dos sensores de core `pACC`/`eACC`
2. Intel: `TC0P`, senão `TC0D`, `TC0E`, `TC0F`
3. senão, a estimativa do `sysinfo`

**Ventoinha/bomba:** `F0` (`smc/F0Ac`), senão a primeira ventoinha. Um Mac sem
ventoinhas não tem ventoinha Auto.

## Limites

- As funções do sistema de eventos HID são API privada. Estão estáveis entre
  versões do macOS e são as que os monitores de código aberto usam, mas a Apple
  pode mudá-las.
- A tabela de chaves do SMC é uma lista de melhor conhecimento; alguns Macs
  nomeiam sensores de outro jeito, então uma chave pode faltar. Abra uma issue
  com a saída de `kalkan sensors` se um sensor útil não aparecer.
- Escrito sem um Mac à mão: a lógica de decodificação, nomes e escolhas tem
  testes unitários que rodam em qualquer SO, e o código do IOKit é verificado
  pelo compilador para `aarch64-apple-darwin` e `x86_64-apple-darwin`, mas ainda
  não foi executado em hardware real.

## Driver serial

O chip USB-serial do display é da família CH34x da WCH (`1A86:484A`). Versões
recentes do macOS trazem um driver para os chips CH340/CH341 comuns e a porta
aparece como `/dev/cu.usbserial-*`; versões mais antigas e outras variantes
precisam do CH34xVCPDriver da WCH (porta `/dev/cu.wchusbserial*`). O macOS lista
cada porta duas vezes, `/dev/cu.*` e `/dev/tty.*`; o app sempre abre a `cu.`.
Quando nenhuma porta é encontrada, o banner de erro consulta o barramento USB
(`ioreg`) e diz se o display não foi detectado (confira o cabo) ou foi detectado
sem porta serial (instale o driver, libere-o em Ajustes do Sistema →
Privacidade e Segurança e reconecte).

Para as outras plataformas, ver [`sensors-windows-PT.md`](sensors-windows-PT.md)
e [`sensors-linux-PT.md`](sensors-linux-PT.md).
