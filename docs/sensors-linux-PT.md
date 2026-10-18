# Sensores no Linux (temperatura e fan)

[English](sensors-linux.md) | **Português (Brasil)**

Diferente do Windows, o Linux não precisa de driver nosso: o kernel já expõe
todo chip de monitoramento pelo **hwmon** (`/sys/class/hwmon`), legível por
qualquer usuário. O `LinuxSource` (`crates/kalkan-core/src/hwmon.rs`) lê isso a
cada segundo; CPU/RAM load vêm do `sysinfo`, como nas outras plataformas.

O módulo de kernel correspondente precisa estar carregado para o chip aparecer:
`k10temp` (CPUs AMD), `coretemp` (Intel), `nct6775`/`nct6799`, `it87` ou
`asus-ec-sensors` (placa-mãe), `amdgpu`, `nvme`, `drivetemp` (discos SATA),
`spd5118` (módulos DDR5), `nzxt-kraken3`, `corsair-cpro`, `aquacomputer_d5next`
(coolers líquidos e controladores de ventoinha). O `sensors-detect` do
lm-sensors ajuda a achar o da placa-mãe. Veja o que o kernel enxerga com
`ls /sys/class/hwmon/*/name`.

## O que é lido

Em cada diretório `hwmonN`, o nome do chip (`name`) mais:

| Arquivo | Significado |
|---|---|
| `tempN_input` | temperatura em milésimos de grau Celsius |
| `tempN_label` | nome do sensor (opcional) |
| `fanN_input` | velocidade da ventoinha em RPM |
| `fanN_label` | nome da ventoinha (opcional) |

Só os arquivos `*_input` são leituras. `*_max`, `*_crit`, `*_min`, `*_alarm`,
`pwm*` e afins são limites ou controles e nunca são lidos. Arquivos ilegíveis e
valores implausíveis (temperatura fora de -50..200 °C, fan acima de 100000 RPM)
são ignorados. Um sensor sem label se chama `Temperature #N` / `Fan #N`.

O nome do hardware é o `model` do dispositivo (modelo do NVMe/SATA, nome do
produto da GPU) quando existe; senão, um nome amigável do chip (`AMD CPU`,
`NVMe drive`, ...). Na CPU, é trocado pelo modelo do `/proc/cpuinfo`.

Os ids dos sensores são estáveis entre reinicializações:
`hwmon/<chip>/<dispositivo>/<temp|fan><N>`, onde `<dispositivo>` é o último
componente do link simbólico `device` (endereço PCI, `nvme0`, `coretemp.0`, ...)
e não o índice `hwmonN`, que pode mudar. É isso que o
`~/.config/KalkanMonitor/settings.json` guarda para um sensor escolhido, por
exemplo `hwmon/k10temp/0000:00:18.3/temp1`.

## Categorias

O nome do chip decide o grupo nos seletores (o tipo de hardware que o
`naming.rs` classifica):

| Nome do chip | Categoria |
|---|---|
| `k10temp`, `zenpower`, `coretemp`, `cpu_thermal` | CPU |
| `amdgpu`, `radeon`, `nouveau`, `i915`, `xe` | GPU |
| `nvme`, `drivetemp` | Storage |
| `spd5118`, `jc42` | Memory |
| `nct6*`, `nct7*`, `it87*`/`it86*`/`it8*`, `asus*`, `gigabyte_wmi`, `w83*`, `f71*`, `acpitz`, `pch_*`, ... | Motherboard |
| `nzxt*`, `corsair*` (exceto `corsairpsu`), `aquacomputer*`, `gigabyte_waterforce`, `asus_rog_ryujin` | Liquid / Pump |

Todo sensor cujo label contenha `pump`, `coolant`, `liquid`, `water` ou `aio`
aparece como Liquid / Pump, seja qual for o chip. Chips desconhecidos caem nas
heurísticas por nome e, se nada casar, ficam em Other.

Os labels da GPU AMD são renomeados para a fonte **Max CPU/GPU** enxergá-los:
`edge` vira `GPU Core`, `junction` vira `GPU Hot Spot`, `mem` vira `GPU Memory`
(a memória não entra no Max CPU/GPU).

## Escolhas Auto

**Temperatura** (vale a primeira que casar):

1. `k10temp` `Tctl`, senão `Tdie`
2. `coretemp` `Package id 0`
3. `zenpower` `Tdie`
4. o primeiro sensor de CPU (de qualquer chip de CPU)

Se não houver nenhum sensor de CPU no hwmon, usa-se a estimativa do `sysinfo`.

**Fan/bomba** (ventoinha lendo 0 RPM é um header vazio e nunca é escolhida):

1. uma ventoinha cujo label contenha `pump`
2. uma ventoinha cujo label contenha `cpu` (`cpu_fan`, `CPU Fan`)
3. a primeira ventoinha em funcionamento de um chip de placa-mãe

A maioria dos drivers de Super-I/O **não publica labels de ventoinha** (os
nomes que aparecem no `sensors` vêm dos arquivos de configuração do
lm-sensors), então em muitas placas vale a terceira regra. Se ela escolher o
header errado, escolha a ventoinha certa em **RPM source**; a escolha é salva
pelo id.

## NVIDIA

O driver proprietário da NVIDIA não tem hwmon. Quando o `nvidia-smi` está
instalado, uma thread em segundo plano roda

```
nvidia-smi --query-gpu=temperature.gpu,name --format=csv,noheader
```

a cada 5 segundos (timeout de 3 s, nunca no laço de 1 s), e cada GPU aparece
como `nvidia-smi/<índice>/temp1` chamada `GPU Core`, então vale para o **Max
CPU/GPU**. A consulta é pulada enquanto a GPU está desligada (o
`power/runtime_status` do dispositivo PCI é `suspended`), para que a dGPU de um
notebook com gráficos híbridos não seja acordada só para ler a temperatura. O
driver livre `nouveau` expõe hwmon.

## Permissões da porta serial

O display é um dispositivo serial USB (`/dev/ttyUSB*` ou `/dev/ttyACM*`).
Instale `linux/udev/60-kalkan-monitor.rules` (o `.deb` já faz isso) ou entre no
grupo `dialout` (`uucp` no Arch) e faça login de novo. O app mostra essa
solução quando abrir a porta falha com "permission denied".

## Limites

- Sensores que dependem de módulo de kernel fora da árvore (algumas placas
  NZXT/ASUS/Gigabyte) só aparecem quando esse módulo está carregado.
- O hwmon expõe o que o chip informa; algumas placas têm headers com label
  errado ou ligação trocada.
- Testado com testes unitários sobre uma árvore hwmon falsa (rodam em qualquer
  SO); ainda não validado em uma variedade grande de hardware real.

Para as outras plataformas, ver [`sensors-windows-PT.md`](sensors-windows-PT.md) e
[`sensors-macos-PT.md`](sensors-macos-PT.md).
