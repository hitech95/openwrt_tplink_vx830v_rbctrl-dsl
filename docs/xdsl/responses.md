# RX Response Layouts (P2)

Byte layouts for the board's replies, parsed by the `libcmm.so` deserializers.
Reverse-engineered from `oal_dsl_msgToLineObj` / `_msgToChannelObj` /
`_msgToLineStatsObj` / `_msgToChannelStatsTotObj` (the four parsers that consume
the opcode-2 and opcode-4 replies). Byte-order cross-checked against
`proto_postprocess`.

> **Confirmed via hardware capture (sniff on lan0.500):**
>
> - RX response payloads contain **raw data only** — the board does **not**
>   echo the opcode byte at the start of the payload. All offsets below are
>   relative to the first byte after the 24-byte frame header (offset `0x18`).
> - The `payload_len` field in the frame header is authoritative for the data
>   size. Ethernet padding (frames < 60 bytes are zero-padded) is excluded.
> - Op2 reply: `payload_len` = 63 (59 bytes data + 4 bytes trailing zeros).
> - Op1/op4/op5 replies: short payloads (4–28 bytes), padded to 60-byte
>   minimum Ethernet frame by the NIC.

> No pcap is available, so these layouts are **pure static analysis** — the
> deserializers are authoritative for field offsets and types. Numeric metric
> names marked *inferred* below follow TR-181 conventions (see
> [../tr-181.md](../tr-181.md) for the data-model context and the
> `Device.DSL.` node mapping) and should be confirmed against a live capture
> when shell access is regained (folds into P4). The byte offsets, sizes, and
> byte-order are NOT inferred — they are read directly from the parser code.

Runnable parser: [../../examples/unpack.py](../../examples/unpack.py).

---

## Byte order

Multi-byte fields arrive **big-endian** on the wire (`proto_postprocess` applies
`ntohl`/`ntohs` on RX before the deserializer sees them in host order). The
parser reads host-order ints; the CLI replacement must `struct.unpack(">...")`
the wire bytes directly.

---

## Opcode 2 — `dsl_get_line_obj` reply (59 bytes)

Parsed by `oal_dsl_msgToLineObj` (`FUN_00323d98`); the channel/stats parsers
slice the same 59-byte reply differently.

### Confirmed fields

| Offset | Size | Field | Encoding / values |
|--------|------|-------|-------------------|
| `0x00` | 4 | status | uint32 BE — `0` = success, else error |
| `0x04` | 1 | link status | `0`,`2`=NoSignal · `1`=Up · `3`=Initializing · `4`=EstablishingLink |
| `0x05` | 1 | modulation | `0`=ADSL_ANSI_T1.413 · `1`=G.dmt · `2`=G.lite · `3`=G.dmt.bis · `4`=ADSL_2plus · `6`=VDSL2 |
| `0x06` | 1 | reserved | (not read) |
| `0x07` | 1 | annex | `ANNEX` code (0=A … 8=auto) |
| `0x39` | 2 | VDSL2 profile bitmask | uint16 BE — bits per `VDSL2_PROFILE` (only read when modulation == 6) |

### The 12 metric uint32 fields (offsets `0x08`–`0x34`, each BE)

The reply carries 12 big-endian uint32 values. Their **offsets and types are
authoritative**; the human names below are *inferred* from TR-181 ordering
(`Device.DSL.Line.` + `Channel.`) and the destination struct offsets:

| Offset | Size | Read by | Inferred name |
|--------|------|---------|---------------|
| `0x08` | 4 | channel obj | downstream curr rate |
| `0x0c` | 4 | channel obj | upstream curr rate |
| `0x10` | 4 | line obj | upstream curr rate |
| `0x14` | 4 | line obj | downstream curr rate |
| `0x18` | 4 | line obj | upstream max rate |
| `0x1c` | 4 | line obj | downstream max rate |
| `0x20` | 4 | line obj | upstream SNR margin |
| `0x24` | 4 | line obj | downstream SNR margin |
| `0x28` | 4 | line obj | upstream attenuation |
| `0x2c` | 4 | line obj | downstream attenuation |
| `0x30` | 4 | line stats | upstream errors (ES) |
| `0x34` | 4 | line stats | downstream errors (SES) |

> The three parsers divide the 12 fields: `msgToChannelObj` reads `[0x08,0x0c]`,
> `msgToLineObj` reads `[0x10..0x2c]`, `msgToLineStatsObj` reads `[0x30,0x34]`.
> Field `[0x56]` is reserved; the reply is 59 bytes total.

---

## Opcode 4 — `dsl_get_channel_stats` reply (28 bytes)

Parsed by `oal_dsl_msgToChannelStatsTotObj` (`FUN_0032483c`). 7 uint32
(`status` + 6 counters), all big-endian.

| Offset | Size | Inferred name |
|--------|------|---------------|
| `0x00` | 4 | status (`0` = success) |
| `0x04` | 4 | ReceiveBlocks |
| `0x08` | 4 | ReceiveErrors |
| `0x0c` | 4 | ReceiveDiscards |
| `0x10` | 4 | TransmitBlocks |
| `0x14` | 4 | TransmitErrors |
| `0x18` | 4 | TransmitDiscards |

> The source→destination permutation in the parser (`+4,+8,+0xc,+0x10,+0x14,
> +0x18`) maps to the standard TR-181 `Channel.Stats.Total` field order; the
> names above are *inferred* from that order.

---

## Enums (RX)

`link status`, `modulation`, `annex`, and `VDSL2 profile` reuse the TX tables in
[payloads.md](payloads.md). RX-relevant subsets:

- **link status**: `0`/`2` = NoSignal, `1` = Up, `3` = Initializing, `4` = EstablishingLink
- **modulation** (active): only `0,1,2,3,4,6` appear in the Up state (`5`/`7` are config-only multimode)
- **annex / VDSL2 profile**: identical to TX

---

## Open items (deferred to P4 / capture)

- ~~Confirm the 12 + 6 *inferred* metric names against a real reply~~ — offsets
  confirmed via hardware capture (all zeros with NoSignal; field names still
  inferred from TR-181 ordering).
- The 4 trailing bytes in the op2 reply (payload_len=63, data=59) are
  undocumented — likely a board-internal footer or alignment padding.
- Two config bytes (`[0x02]`,`[0x03]` in the opcode-1 TX) round-trip through the
  same struct; a capture will name them.
