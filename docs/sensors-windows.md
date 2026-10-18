# Sensores no Windows (temperatura e fan)

O Windows **não tem API** para temperatura de CPU nem RPM de fan. Esses valores
vêm de MSRs do processador e do Super-I/O/Embedded Controller da placa — ambos
só acessíveis em **ring 0 (kernel)**. Rodar "como administrador" não basta
(admin ainda é ring 3); é preciso um **driver de kernel**. Por isso todo monitor
(HWiNFO, LibreHardwareMonitor, Core Temp…) carrega um driver, e por isso o app
original da Kalkan pede elevação: ele embute o motor do HWiNFO (`config.dat` **é**
o `HWiNFO64.dll` do SDK, renomeado, com o driver embutido como recurso).

## Fonte 1 — o motor HWiNFO do app original (`config.dat`)

Mesma fonte do original, e **funciona com a Integridade de Memória (HVCI)
ligada** (o driver do HWiNFO é compatível). O `kalkan-core` carrega a DLL e
chama as funções **por ordinal**, como o `RunHwinfo` (`FUN_140002d80`) do `.exe`:

| ordinal | função | uso |
|---|---|---|
| 590 | `Init(0x40) -> int` | 0 = ok; ≠ 0 sem admin |
| 883 | `NumSensors() -> int` | |
| 726 | `Update()` | antes de cada leitura |
| 749 | `SelectSensor(int)` | |
| 106 | `GetReading(type, sensor, index, *raw) -> bool` | type 1 = temp, 3 = fan; index 0..0x200 |

Struct de saída (0x1D0 bytes): `+0x00` u8 válido, `+0x08` f64 valor, `+0x3C`
char[256] nome do sensor, `+0x148` char[128] rótulo, `+0x1C8` u32 tipo.

Escolha (igual a `FUN_1400023b0`): temperatura = rótulo com `package`/`average`;
fan = rótulo com `cpu` (sem diferenciar maiúsculas); senão a primeira plausível.
Varre tudo uma vez e depois relê só as duas leituras escolhidas a cada 1 s.

A DLL é procurada em: `KALKAN_HWINFO_DLL`; `config.dat`/`HWiNFO64.dll` ao lado do
executável; `C:\Program Files (x86)\Kalkan PC SmartMonitor DZ V1.3\config.dat`.
É binário proprietário do HWiNFO: **não** é versionado nem redistribuído aqui —
usamos a cópia da instalação do app original.

Diagnóstico: `kalkan hwinfo` (elevado) lista toda leitura de temp/fan e a escolha.

## Fonte 2 — sidecar LibreHardwareMonitor (sem o app original)

Usado só se o motor HWiNFO não carregar. Um **sidecar** `kalkan-sensord.exe` (projeto .NET em
`sidecar/kalkan-sensord`, publicado self-contained) que carrega o
**LibreHardwareMonitor** (motor + driver de kernel próprio) e emite leituras como
JSON por linha no stdout, a cada 1 s:

```
{"cpu_temp_c":42.0,"fan_rpm":2400}
```

O `kalkan-core` (só no Windows) spawna o sidecar, lê essa saída numa thread e
combina com CPU/RAM load do `sysinfo`. Fica **lado a lado** com o executável (ou
aponte via a variável `KALKAN_SENSORD`). Se o sidecar não existir ou não entregar
dados, há um **fallback para o HWiNFO Shared Memory** e, por fim, `None` (sem
erro — temp/fan aparecem como `--`).

A app da UI se **auto-eleva** no início (relança via UAC), como o original, para
que o driver do sidecar possa carregar. (`KALKAN_NO_ELEVATE=1` desliga, em dev.)

macOS e Linux **não usam** nada disso: têm fontes nativas sem driver que a gente
distribua (hwmon no Linux, SMC no macOS). O sidecar e o driver são só do Windows.

## Integridade de Memória (HVCI) — limitação importante

O driver que o LibreHardwareMonitor usa (WinRing0) **não carrega** com a
**Integridade de Memória / Isolamento de Núcleo (HVCI)** ligada. Nesse caso o
sidecar roda, mas MSR/Super-I/O voltam zerados (ex.: `Core (Tctl/Tdie) = 0`,
nenhum chip de fan detectado) e temp/fan ficam `--`.

O app original funciona sob HVCI porque o driver do **HWiNFO é compatível com
HVCI**; o do LibreHardwareMonitor (hoje) não é. Opções para ter leitura real sob
HVCI:

1. **Usar o HWiNFO** (o backend já cai nele por fallback): abra o HWiNFO com
   "Shared Memory Support" ligado. Driver compatível com HVCI.
2. **Instalar o PawnIO** — driver de kernel open-source **compatível com HVCI**
   que o LibreHardwareMonitor sabe usar; com ele, o sidecar lê MSR mesmo com a
   Integridade de Memória ligada.
3. **Desligar a Integridade de Memória** (Segurança do Windows → Segurança do
   dispositivo → Isolamento de núcleo). **Não recomendado** — é um downgrade de
   segurança.

Diagnóstico rápido: `kalkan-sensord.exe --dump` (elevado) lista todo o hardware e
sensores que o LibreHardwareMonitor enxerga. Se os valores de temperatura/clock
saírem `0`/`null` mas o CPU Load funcionar, o driver não está ativo (HVCI).
