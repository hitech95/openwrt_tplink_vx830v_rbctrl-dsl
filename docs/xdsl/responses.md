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

## Opcode 2 — `dsl_get_line_obj` reply (63 bytes)

Parsed by `oal_dsl_msgToLineObj` (`FUN_00323d98`) on the host; filled by
`dslCfgGet` (`FUN_004048c8`) on the board. The board-side code is now
available (EcoNet MIPS `remote_board`, see [../server.md](../server.md))
and provides **ground truth** for every field.

### Confirmed layout (board-side ground truth)

| Offset | Size | Board writer | Field | Encoding |
|--------|------|-------------|-------|----------|
| `0x00` | 4 | `dslCfgGet` return | status | `0` = success |
| `0x04` | 1 | `apiGetXdslStatus` | link status | `0`,`2`=NoSignal · `1`=Up · `3`=Init · `4`=Establishing |
| `0x05` | 1 | `apiGetAdslType` | modulation | `0`=T1.413 · `1`=G.dmt · `2`=G.lite · `3`=G.dmt.bis · `4`=ADSL2+ · `6`=VDSL2 |
| `0x06` | 1 | `apiGetAdslDataPath` | **data path** | **`0`=ATM · `1`=PTM** ← was "reserved" |
| `0x07` | 1 | `apiGetAdslType` | annex | `0`=A · `1`=B · `2`=I · `3`=M |
| `0x08` | 4 | `FUN_00402818` | downstream rate | uint32 BE, kbps |
| `0x0c` | 4 | `FUN_00402818` | upstream rate | uint32 BE, kbps |
| `0x10` | 4 | `FUN_00402bf8` | **output power** pair ¹ | uint32 BE |
| `0x14` | 4 | `FUN_00402bf8` | **output power** pair ¹ | uint32 BE |
| `0x18` | 4 | `FUN_00402988` | **noise margin** pair ¹ | uint32 BE, dB×10 (e.g. 63 = 6.3 dB) |
| `0x1c` | 4 | `FUN_00402988` | **noise margin** pair ¹ | uint32 BE, dB×10 |
| `0x20` | 4 | `FUN_00402ac0` | **attenuation** pair ¹ | uint32 BE, dB×10 |
| `0x24` | 4 | `FUN_00402ac0` | **attenuation** pair ¹ | uint32 BE, dB×10 |
| `0x28` | 4 | `FUN_00402cec` | **attainable rate** pair ¹ | uint32 BE, kbps |
| `0x2c` | 4 | `FUN_00402cec` | **attainable rate** pair ¹ | uint32 BE, kbps |
| `0x30` | 4 | `FUN_00402e24` | **CRC errors** pair ¹ | uint32 BE, count |
| `0x34` | 4 | `FUN_00402e24` | **CRC errors** pair ¹ | uint32 BE, count |
| `0x38` | 1 | `dslCfgGet` | **ATM connection flag** | `1`=ATM, `0`=PTM ← new field |
| `0x39` | 2 | `FUN_00402284` | VDSL2 profile bitmask | uint16 BE, only when modulation=6 |
| `0x3b` | 4 | `dslCfgGet` | **uptime seconds** | uint32 BE, only when link=1 ← was "unknown" |

> ¹ The board reads DS and US values from `/proc/tc3162/adsl_stats` and
> writes them to consecutive offsets. The DS/US order within each pair
> follows the order in the stats file. The noise margin, attenuation, and
> output power values are in **dB × 10** (parsed via `sscanf("%d.%d")`
> then multiplied by 10).

### Corrections from board-side ground truth

The previous (host-side-only) analysis inferred field names from TR-181
ordering and destination struct offsets. Several were **wrong**:

| Offset | Previous (inferred) | Corrected (board ground truth) |
|--------|---------------------|-------------------------------|
| `0x06` | reserved | **data path** (0=ATM, 1=PTM) |
| `0x10` | US curr rate | **output power** (pair) |
| `0x14` | DS curr rate | **output power** (pair) |
| `0x18` | US max rate | **noise margin** (pair, dB×10) |
| `0x1c` | DS max rate | **noise margin** (pair, dB×10) |
| `0x20` | US SNR margin | **attenuation** (pair, dB×10) |
| `0x24` | DS SNR margin | **attenuation** (pair, dB×10) |
| `0x28` | US attenuation | **attainable rate** (pair) |
| `0x2c` | DS attenuation | **attainable rate** (pair) |
| `0x30` | US errors (ES) | **CRC errors** (pair) |
| `0x34` | DS errors (SES) | **CRC errors** (pair) |
| `0x38` | (not read) | **ATM connection flag** |
| `0x3b` | unknown u32 | **uptime seconds** |

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

- ~~Confirm the 12 + 6 *inferred* metric names~~ — **RESOLVED** via board-side
  ground truth (see corrections table above).
- ~~Undocumented u32 at payload[0x3B]~~ — **RESOLVED**: uptime seconds, only
  filled when link status == 1.
- ~~Offset 0x06 "reserved"~~ — **RESOLVED**: data path flag (0=ATM, 1=PTM).
- The DS/US ordering within each metric pair (offsets 0x10–0x34) follows the
  order values appear in `/proc/tc3162/adsl_stats`; confirm exact DS/US
  assignment per pair with a live capture.

---

## Cross-reference confirmation

All offsets below were cross-referenced against the named functions in
`libcmm.so`:

| Function | Reads (reply offset → field) | Confirmed |
|----------|------------------------------|-----------|
| `oal_dsl_msgToChannelObj` | u32 @ 0x08, 0x0C | ✓ |
| `oal_dsl_msgToLineObj` | u32 @ 0x10..0x2C (8 fields) | ✓ |
| `oal_dsl_msgToLineObj` | u8 @ 0x04 (link_status) | ✓ |
| `oal_dsl_msgToLineObj` | u8 @ 0x05 (modulation) | ✓ |
| `oal_dsl_msgToLineObj` | u8 @ 0x07 (annex) | ✓ |
| `oal_dsl_msgToLineObj` | u16 @ 0x39 (VDSL2 profile) | ✓ |
| `oal_dsl_msgToLineStatsObj` | u32 @ 0x30, 0x34 | ✓ |
| `oal_dsl_msgToChannelStatsTotObj` | u32 @ 0x00..0x18 (7 fields) | ✓ |

`proto_postprocess` confirms: **no opcode echo byte** — byte-swapping starts
directly at payload[0x00]. Default case (op1/op3/op5/etc.) swaps only the
status u32 at payload[0x00].
