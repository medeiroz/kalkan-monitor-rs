# Sensores no Windows (temperatura e fan)

O Windows **não tem API** para temperatura de CPU nem RPM de fan. Esses valores
vêm de MSRs do processador e do Super-I/O/Embedded Controller da placa — ambos
só acessíveis em **ring 0 (kernel)**. Rodar "como administrador" não basta
(admin ainda é ring 3); é preciso um **driver de kernel**. Por isso todo monitor
(HWiNFO, LibreHardwareMonitor, Core Temp…) carrega um driver, e por isso o app
original da Kalkan pede elevação: ele embute o motor do HWiNFO (`config.dat` **é**
o `HWiNFO64.dll` do SDK, renomeado, com o driver embutido como recurso).

A UI se **auto-eleva** no início (relança via UAC), como o original, para que os
drivers possam ser usados. (`KALKAN_NO_ELEVATE=1` desliga, em dev.)

Ordem das fontes (`WindowsSource`): **sidecar LibreHardwareMonitor + PawnIO** →
**motor HWiNFO do app original** → **HWiNFO Shared Memory** → `None` (sem erro —
temp/fan aparecem como `--`). CPU/RAM load vêm sempre do `sysinfo`.

macOS e Linux **não usam** nada disso: têm fontes nativas sem driver que a gente
distribua (hwmon no Linux, SMC no macOS).

## Fonte 1 — sidecar LibreHardwareMonitor + PawnIO (padrão)

Um **sidecar** `kalkan-sensord.exe` (projeto .NET em `sidecar/kalkan-sensord`,
publicado self-contained — não exige .NET instalado) carrega o
**LibreHardwareMonitor** (MPL-2.0) e emite leituras como JSON por linha no
stdout, a cada 1 s:

```
{"cpu_temp_c":42.0,"fan_rpm":2400}
```

O `kalkan-core` spawna o sidecar (sem janela de console), lê essa saída numa
thread e combina com o `sysinfo`. Fica **lado a lado** com o executável (ou
aponte via `KALKAN_SENSORD`).

O LibreHardwareMonitor (0.9.5+) acessa o hardware pelo driver **PawnIO**: open
source, assinado pela Microsoft e **compatível com a Integridade de Memória
(HVCI)**. O **instalador** (`installer/`) instala o PawnIO 2.2.0 em modo
silencioso (`PawnIO_setup.exe -install -silent`) quando ele falta ou é < 2.0 —
mesma regra do LibreHardwareMonitor — e **não** o remove na desinstalação (outros
programas usam o mesmo driver). Validado num Ryzen 7 9800X3D + Nuvoton NCT6799D
com HVCI ligado: Tctl/Tdie e "Fan #2" (= CPUFANIN0).

Sem o PawnIO, o sidecar roda mas MSR/Super-I/O voltam zerados e temp/fan ficam
`None`; após 10 s sem dados, o `WindowsSource` tenta a Fonte 2.

Diagnóstico: `kalkan-sensord.exe --dump` (elevado) lista todo o hardware e
sensores que o LibreHardwareMonitor enxerga. Temperatura/clock `0`/`null` com CPU
Load funcionando = driver não ativo.

## Fonte 2 — o motor HWiNFO do app original (`config.dat`)

Fallback para quando o app original da Kalkan está instalado. Também **funciona
com HVCI** (o driver do HWiNFO é compatível). O `kalkan-core` carrega a DLL e
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
usamos a cópia da instalação do app original. Ela grava um `-debug.log` no
diretório atual (ignorado pelo git).

Diagnóstico: `kalkan hwinfo` (elevado) lista toda leitura de temp/fan e a escolha.

## Fonte 3 — HWiNFO Shared Memory

Se o HWiNFO estiver aberto com "Shared Memory Support" ligado, lemos
`Global\HWiNFO_SENS_SM2` (não exige admin).

## Desligar a Integridade de Memória?

**Não é necessário** (as Fontes 1 e 2 funcionam com HVCI) e **não é
recomendado** — é um downgrade de segurança.
