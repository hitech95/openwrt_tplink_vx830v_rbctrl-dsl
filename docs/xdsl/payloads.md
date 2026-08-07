# TX Payload Layouts (P1)

Exact byte layouts for every write/config command. Reverse-engineered from the
`libcmm.so` serializers and cross-checked against `remote_board:proto_postprocess`
(the byte-swap layer that runs before TX). All multi-byte fields are
**big-endian on the wire**.

> **Frame/payload boundary correction.** The byte the `proto_frame_hdr` struct
> calls `bPayload_type` (offset `0x18`) is **functionally payload byte 0** — the
> first byte of the command descriptor. Both the serializers and
> `proto_postprocess` treat offset `0x18` as the descriptor start. The header
> proper is 24 bytes (`0x00`–`0x17`); `wPayload_len` = descriptor length, and the
> checksum covers `wPayload_len + 10` bytes from `0x0E` (see
> [../checksum.md](../checksum.md)).

> The payloads are serialized TR-181 objects — see [../tr-181.md](../tr-181.md)
> for the data-model context (e.g. opcode 5 packs a `Device.ATM.Link.` row +
> its `QoS.` child).

Runnable packers: [../../examples/pack.py](../../examples/pack.py).

---

## Enum tables

Extracted from `libcmm.so` globals (`modulationTypes` `0x3edc20`,
`annexTypes` `0x3edd20`, `profiles` `0x3eddb0`); ATM tables inline in the
serializers.

### Modulation (`modulationTypes`) — opcode 1 byte `[0]`

| code | name | | code | name |
|------|------|---|------|------|
| 0 | ADSL_ANSI_T1.413 | | 4 | ADSL_2plus |
| 1 | ADSL_G.dmt (G.992.1) | | 5 | ADSL_Multimode |
| 2 | ADSL_G.lite (G.992.2) | | **6** | **VDSL2** |
| 3 | ADSL_G.dmt.bis (G.992.3/4) | | 7 | Multimode |

### Annex (`annexTypes`) — opcode 1 byte `[1]`

| code | name | | code | name |
|------|------|---|------|------|
| 0 | Annex A | | 5 | Annex A/L/M |
| 1 | Annex B | | 6 | Annex J |
| 2 | Annex I | | 7 | Annex B/J |
| 3 | Annex M | | 8 | Annex auto |
| 4 | Annex A/L | | | |

### VDSL2 profile bitmask (`profiles`) — opcode 1 bytes `[4..7]`

Only populated when modulation ∈ {6 VDSL2, 7 Multimode}; zero for all ADSL modes.
Multiple profiles OR together.

| bit | profile | | bit | profile |
|-----|---------|---|-----|---------|
| 0x001 | 8a | | 0x010 | 12a |
| 0x002 | 8b | | 0x020 | 12b |
| 0x004 | 8c | | 0x040 | 17a |
| 0x008 | 8d | | 0x080 | 30a |
| | | | 0x100 | 35b |

### ATM enums — opcode 5

| field | values |
|-------|--------|
| encapsulation `[0x10]` | `0`=LLC, `1`=VCMUX |
| link type `[0x11]` | `0`=EoA, `6`=PPPoA, `7`=IPoA |
| QoS category `[0]` | `1`=UBR, `2`=CBR, `3`=VBR-nrt, `4`=VBR-rt |

---

## Opcode 1 — `dsl_config_up` (12 bytes)

Serializer: `oal_dsl_lineObjToMsg` (`FUN_00323584`).

| Offset | Size | Field | Encoding |
|--------|------|-------|----------|
| `0x00` | 1 | modulation | `MODULATION` code |
| `0x01` | 1 | annex | `ANNEX` code |
| `0x02` | 1 | bitswap enable | `X_TP_BitswapEnable`: 0=disabled, 1=enabled (TR-181 obj `+0x2c9`) |
| `0x03` | 1 | SRA enable | `X_TP_SRAEnable`: 0=disabled, 1=enabled (TR-181 obj `+0x2ca`) |
| `0x04` | 4 | VDSL2 profile bitmask | big-endian uint32 (OR of `VDSL2_PROFILE` bits) |
| `0x08` | 4 | reserved | `0x00000000` |

The bitmask is built by tokenizing the source object's `allowedProfile` string
(`strtok_r` on `;`,`) and OR-ing each token's bit. Sent as a 4-byte BE value at
`[4..7]` with `[8..11]` zero (`proto_postprocess` swaps only the low 4 bytes).

> **Bytes [0x02]/[0x03] resolved** (previously TBD): they are `X_TP_BitswapEnable`
> and `X_TP_SRAEnable` — confirmed via `oal_dsl_lineObjToMsg` decompilation +
> `rsl_dslLine_checkParamValid` validation + web UI `dslcfg.htm`. Both are
> **TX-only**: the opcode-2 response does not report them back.
>
> **Additional DSL features in the web UI** (`dslcfg.htm`) that are NOT in this
> payload: G.INP (`X_TP_GINPEnable`), 35B compat (`X_TP_35bCompat`), UPBO
> (`X_TP_UPBO`), ROC/SOS (`X_TP_ROCSOSEnable`). Bytes `[0x08-0x0B]` are zeroed
> by the serializer — these features are either board-autonomous or travel a
> path not yet traced.

---

## Opcode 5 — `atm_link_add` (24 bytes)

Serializers: `oal_atm_linkObjToMsg` (`FUN_00324938`) + `oal_atm_qosObjToMsg`
(`FUN_00324b84`), combined into one 24-byte descriptor by `oal_atm_setVlanTag`.

| Offset | Size | Field | Encoding |
|--------|------|-------|----------|
| `0x00` | 1 | QoS category | `ATM_QOS` code (1=UBR, 2=CBR, 3=VBR-nrt, 4=VBR-rt) |
| `0x01` | 1 | VPI | byte |
| `0x02` | 2 | VCI | big-endian uint16 |
| `0x04` | 4 | peak cell rate (PCR) | big-endian uint32 |
| `0x08` | 4 | sustainable cell rate (SCR) | big-endian uint32 (0 unless VBR) |
| `0x0c` | 4 | max burst size (MBS) | big-endian uint32 (0 unless VBR) |
| `0x10` | 1 | encapsulation | 0=LLC, 1=VCMUX |
| `0x11` | 1 | link type | 0=EoA, 6=PPPoA, 7=IPoA |
| `0x12` | 2 | local VLAN id | big-endian uint16 (= `dslVlan + 2000`) |
| `0x14` | 1 | tag enable | 0/1 |
| `0x15` | 2 | tag VID | big-endian uint16 |
| `0x17` | 1 | tag priority | byte |

> Note: `oal_atm_qosObjToMsg` writes the category as a 4-byte int at `[0]`, then
> `oal_atm_linkObjToMsg` overwrites bytes `[1..3]` with VPI/VCI. Since the
> category fits in one byte (≤4), this composes correctly — category occupies
> only byte `[0]`, and VPI/VCI fill `[1..3]`.

---

## Opcode 15 — `ptm_link_add` (8 bytes)

Built inline in `oal_ptm_setVlanTag`.

| Offset | Size | Field | Encoding |
|--------|------|-------|----------|
| `0x00` | 1 | tag enable | 0/1 |
| `0x01` | 2 | tag VID | big-endian uint16 |
| `0x03` | 2 | tag priority | big-endian uint16 |
| `0x05` | 1 | reserved | `0x00` |
| `0x06` | 2 | local VLAN id | big-endian uint16 |

---

## Opcodes 6 / 16 — `atm_link_del` / `ptm_link_del` (3 bytes)

Built inline in `oal_atm_delVlanTag` / `oal_ptm_delVlanTag` /
`oal_atm_setAtmIfStatus`. Identical 3-byte layout for both ATM (opcode 6) and
PTM (opcode 16).

| Offset | Size | Field | Encoding |
|--------|------|-------|----------|
| `0x00` | 2 | local VLAN id | big-endian uint16 |
| `0x02` | 1 | type / status | `3` = delete |

The same opcode-6 descriptor with `type != 3` is a status-set rather than a
delete (used by `oal_atm_setAtmIfStatus`).

---

## Opcodes 7 / 8 — firmware (no new layout)

Opcode 7 (`main_image_check`) carries **no payload**. Opcode 8
(`firmware_upgrade`) carries the 128-byte path string `/var/tmp/remoteflash.bin`.
Both are documented in [../commands/firmware.md](../commands/firmware.md).
