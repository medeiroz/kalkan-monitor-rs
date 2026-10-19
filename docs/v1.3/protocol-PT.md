# Kalkan SmartMonitor **1.3** — notas de engenharia reversa

[English](protocol.md) | **Português (Brasil)**

Alvo analisado: `Kalkan PC SmartMonitor DZ V1.3.exe` (linha Alpha Revisão 2,
que segundo o fabricante abrange as unidades compradas a partir de jan/2026, e
Delta V2). SHA-256 do binário original: `faf27352654837fa95558c2b5f0e9db3755b763e2c08540fed15a7d37af39897`.
Os instaladores oficiais "Kalkan PC SmartMonitor (Linha Delta V2) V1.3" e
"Kalkan PC SmartMonitor (Linha Alpha Rev.2) V1.3" (SHA-256
`b98d03b730548a089716831f35e44e32a6e861889bde67b6aae96c82c94d1589`) trazem
exatamente este binário (mesmo SHA-256; conferido desempacotando com
`innoextract`, sem executar).
Ferramenta: Ghidra 12.1.4 via REA. Tudo abaixo é **observado estaticamente** no
binário (pseudocódigo + assembly x86-64).

> A engenharia reversa foi feita sobre uma cópia local com um patch de 1 byte
> em `main` (desliga o anti-UAC do app — ver nota no fim); o código do protocolo
> é idêntico ao do binário original e não é tocado por esse patch.

## O que mudou da 1.2 para a 1.3

A 1.3 é **incompatível** com a 1.2 ([notas](../v1.2/protocol-PT.md)) e **bem mais simples**:

| Aspecto | 1.2 | 1.3 |
|---|---|---|
| Transporte | Só HID (`hidapi`; as strings de `QSerialPort` não são referenciadas) | **Só serial** (`QSerialPort`) |
| Seleção do aparelho | VID:PID `0416:1100` fixo no código | VID/PID fixos no código (abaixo) |
| Parâmetros seriais | — | **1 Mbps, 8N1, sem controle de fluxo** |
| Handshake / seed | pacote `01 81 E3`, acha `0xE3` → seed | **nenhum** |
| Frame | 64 bytes `01 86 E0 idx temp fanHi fanLo cpu …` | **5 bytes** `74 00 temp fanHi fanLo` |
| Embaralhamento | bit-a-bit com seed evoluindo | **XOR `0x02`** do byte 2 em diante |
| `cpu_load` no frame | sim | **não** |
| índice de frame | sim | **não** |

## Arquitetura (classes)

- **`Sysctl`** (vftable `0x140036008`): o controlador exposto ao QML como
  `sys_controller`. Lê sensores (via o tipo QML `HwiHandler`), monta o frame,
  embaralha e **emite `emitSendData(QByteArray)`** (sinal, índice 4 →
  `FUN_14002fcc0`). Também cuida de config (`GENERAL`/`Auto Run`/`Show Fah`/
  `Language`/`Default Temp`/`Default Sped`) e idioma.
- **`Transport`** (vftable `0x1400364e0`): encapsula um único `QSerialPort`
  (em `this+0x10`). Slots `slotSendData(QByteArray)` (`FUN_140008640` — escreve
  na serial) e `slotRead()`. O `Sysctl::emitSendData` é ligado ao
  `Transport::slotSendData` no construtor do `Sysctl` (`FUN_140005330`).

## Abrir o dispositivo — `FUN_140008300` → `FUN_1400083d0`

`Sysctl::connect` tenta dois pares (VID, PID), nesta ordem:

1. **`1F3A:0008`**
2. **`1A86:484A`** (família CH340 — ponte USB-serial comum)

`FUN_1400083d0(this, vid, pid)`:

1. `QSerialPortInfo::availablePorts()`; percorre casando `vendorIdentifier()` e
   `productIdentifier()` com (vid, pid); guarda o `portName()`.
2. Se achou: `setPortName`, abre (`open`, ReadWrite), e configura:
   - `setBaudRate(1000000, AllDirections)`
   - `setDataBits(8)`
   - `setParity(NoParity)`
   - `setStopBits(OneStop)`
   - `setFlowControl(NoFlowControl)`
3. Loga `"open ok"` / `"open error"`. Retorna sucesso só se todos os `set*`
   derem certo.

**Não há envio de pacote de handshake.** Depois de abrir, o app começa a mandar
frames direto.

## Frame de dados e embaralhamento — `Sysctl::processTrans`, `FUN_1400062b0`

Chamado periodicamente (timer). Resumo:

```c
if (!opened) {                         // this+0x1c
    opened = Transport::open();        // FUN_140008300 (ver acima)
    if (opened) delay = 6;             // this+0x98: pula 6 ciclos após abrir
    emit openedChanged();
}
if (opened) {
    if (delay != 0) { delay--; return; }
    if (Transport::bytesToWrite() == 0) {      // FUN_1400083a0 == 0
        char  temp = *(u8  *)(this + 0x10);    // temperatura
        u16   fan  = *(u16 *)(this + 0x12);    // rotação (big-endian no frame)

        buf.clear();
        buf.append(0x74);  // 't'
        buf.append(0x00);
        buf.append(temp);
        buf.append(fan >> 8);
        buf.append(fan & 0xff);

        for (i = 2; i < buf.size(); i++)   // cabeçalho (2 bytes) fica literal
            buf[i] ^= 0x02;

        emitSendData(buf);                 // -> Transport escreve na serial
    }
}
```

### Layout do frame (5 bytes)

| offset | conteúdo | no fio (após XOR) |
|--------|----------|-------------------|
| 0 | `0x74` (`'t'`) | `0x74` (literal) |
| 1 | `0x00` | `0x00` (literal) |
| 2 | `temp` (°C) | `temp ^ 0x02` |
| 3 | `fan` byte alto | `(fan>>8) ^ 0x02` |
| 4 | `fan` byte baixo | `(fan&0xff) ^ 0x02` |

Exemplo (`temp=40`, `fan=1200`): cru `74 00 28 04 B0` → fio `74 00 2A 06 B2`.

## Porte em Rust

Implementado e testado em `crates/kalkan-core/src/protocol.rs`:
`build_frame(temp, fan)`, `scramble(frame)` e `KalkanBuilder` (sem estado). Um
vetor de regressão (`74 00 2A 06 B2`) trava o comportamento. A abertura por
VID/PID + baud está em `device.rs` (`find_display_port`, `DISPLAY_IDS`) e
`transport.rs` (`SerialTransport::open_display`), com o atalho `open_display()`
em `lib.rs`. Rode `kalkan frame` (CLI) para o pipeline rodando offline e
`kalkan list` para achar a porta do cooler.

## Dependente de hardware

Nada do **conteúdo** do protocolo depende mais de observação: frame, XOR,
VID/PID e parâmetros seriais estão todos reconstruídos. O único passo que exige
o aparelho é confirmar que ele enumera com um dos VID/PID de [`DISPLAY_IDS`]
(`kalkan list`), já que o cooler de teste só existe na versão 1.3.

## Solução de problemas / qual versão eu tenho

O `LEIA-ME.txt` do fabricante (que acompanha os instaladores da linha Alpha)
liga o software à revisão do hardware:

> "Kalkan PC SmartMonitor (Linha Alpha Rev.2) 1.3: Produtos da linha Alpha
> comprados a partir de Janeiro/2026 são da Revisão 2."
>
> "Kalkan PC SmartMonitor (Linha Alpha) V1.2: Produtos da linha Alpha vendidos
> antes de Janeiro/2026 são da Revisão 1."

(Revisão 2 → 1.3, este protocolo; Revisão 1 → [1.2](../v1.2/protocol-PT.md).)
A data de compra é a regra prática do fabricante; a etiqueta da caixa é mais
precisa: o serial logo acima do código de barras traz o SKU seguido da data de
fabricação no formato ano/mês/dia (exemplo do fabricante:
`KLK00047 - 20251106XXX`).

Se o display **fica travado em 88°**, segundo o fabricante a versão errada está
instalada:

> "Caso execute o software e o mesmo não mostre a temperatura no visor, ficando
> travado em 88°, desinstale a versão atual instalada e tente instalar a outra
> versão disponível nesta pasta."

(O fabricante também pede para instalar e executar o software dele como
Administrador.) Com este app: se o `kalkan list` não mostra nenhuma porta com
`1F3A:0008` / `1A86:484A`, o cooler provavelmente é uma unidade Revisão 1
(1.2), suportada de forma experimental via USB HID (aparece no `kalkan list`
como candidato HID `0416:1100`; veja [1.2](../v1.2/protocol-PT.md)).

## Nota: anti-UAC (fora do protocolo)

A `main` da 1.3 desliga o UAC (`EnableLUA=0` em
`HKLM\…\Policies\System`) a cada início — comportamento removido à parte, por um
patch de 1 byte no `.exe` instalado. Não tem relação com o protocolo do display
e **não** é reproduzido neste porte.
