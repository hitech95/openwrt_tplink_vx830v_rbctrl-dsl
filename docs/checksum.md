# Frame Checksum (P0)

The integrity check covering every `0x88B5` frame between host and board. This
is the **P0 blocker** for the CLI replacement: until the algorithm is matched,
no transmitted frame will be accepted by the board.

> **Status: SOLVED.** Reverse-engineered from `remote_board:proto_compute_checksum`
> (`0x004031b8`), `proto_set_checksum`, `proto_verify_checksum`, and the table at
> `g_awCrcTable` (`0x00404b80`). A Python reference that passes a self-test lives
> at [../examples/checksum.py](../examples/checksum.py).

## TL;DR

The checksum is **CRC-16/ARC** (poly `0x8005`, init `0x0000`, refin = refout =
true, xorout `0x0000`), computed over a **covered region** of the frame with the
checksum field itself **zeroed** during the computation. The result is stored
**big-endian** in the header.

```
crc = CRC-16/ARC( frame[0x0E .. 0x0E + payload_len + 10 - 1],  checksum field = 0 )
frame.wChecksum = big_endian(crc)
```

## Frame header (`proto_frame_hdr`, 24 bytes) + covered payload

Multi-byte fields are **big-endian on the wire** (enforced by `proto_postprocess`
before the checksum is computed — see [§ Byte-order layer](#byte-order-layer)).

| Offset | Field | Size | Covered? |
|--------|-------|------|----------|
| `0x00` | `aDst_mac` | 6 | no |
| `0x06` | `aSrc_mac` | 6 | no |
| `0x0C` | `wEthertype` | 2 | no (`0x88B5`) |
| `0x0E` | `bMagic` | 1 | **yes — region starts here** |
| `0x0F` | `bSubtype` | 1 | yes (= opcode) |
| `0x10` | `dwSeq` | 4 | yes |
| `0x14` | `wPayload_len` | 2 | yes |
| `0x16` | `wChecksum` | 2 | yes (**zeroed during compute**) |
| `0x18` | `bPayload_type` | 1 | yes |
| `0x19` | payload | … | yes |

**Covered length** = `wPayload_len + 10`. The constant 10 is
`magic(1) + subtype(1) + seq(4) + payload_len(2) + checksum(2)`. Note that
`wPayload_len` counts bytes **from `bPayload_type` onward** (type byte + actual
payload), so a zero-payload command still has `wPayload_len ≥ 1`.

The Ethernet header (MACs + ethertype, offsets `0x00`–`0x0D`) is deliberately
**excluded** — it is validated by the BPF filter and socket bind, not by the
checksum. The self-test at [../examples/checksum.py](../examples/checksum.py)
confirms a `dst_mac` change is correctly ignored.

## The algorithm

`proto_compute_checksum(seed, buf, len)` walks `buf` one byte at a time,
processing each byte as two nibbles (low first, then high) via a 16-entry table:

```c
ushort proto_compute_checksum(ushort acc, byte *buf, int len) {
    while (len-- > 0) {
        ushort lo = *buf & 0x0f, hi = *buf >> 4;
        ushort t   = (acc >> 4) ^ T[acc & 0x0f] ^ T[lo];   // low nibble
        acc        = (t   >> 4) ^ T[t   & 0x0f] ^ T[hi];   // high nibble
        buf++;
    }
    return acc;
}
```

`T` is `g_awCrcTable` (`0x00404b80`), 16 × `ushort`:

```
0x0000  0xCC01  0xD801  0x1400   0xF001  0x3C00  0x2800  0xE401
0xA001  0x6C00  0x7800  0xB401   0x5000  0x9C01  0x8801  0x4400
```

### Identification: CRC-16/ARC

The table is **GF(2)-linear** (`T[a^b] == T[a] ^ T[b]`), so each per-nibble step
`(acc>>4) ^ T[acc&0xF] ^ T[n]` collapses to the standard reflected nibble-CRC
update `(acc>>4) ^ T[(acc ^ n) & 0xF]`. The table values are exactly those
produced by the **reflected polynomial `0xA001`** (the reflection of `0x8005`):

- `T[1] = 0xCC01` — confirmed by shifting the seed `1` right four times under
  `0xA001`: `1 → 0xA001 → 0xF001 → 0xD801 → 0xCC01`.
- `T[2] = 0xD801`, `T[3] = 0x1400`, … all match.

That polynomial + `init = 0` + refin/refout + `xorout = 0` is the
**CRC-16/ARC** profile (a.k.a. CRC-16/IBM, CRC-16/ANSI, CRC-16/LHA). The
Python port reproduces the bit-by-bit CRC-16/ARC and asserts equality — the
self-test passes for all tested lengths.

> The board stores the result **big-endian** (`htons` applied in
> `proto_set_checksum` before the store). CRC-16/ARC is often stored
> little-endian elsewhere; here the wire convention is big-endian, matching the
> rest of the header.

## Set / verify semantics

`proto_set_checksum(frame)` (TX):
1. `proto_postprocess(frame)` — byte-swap multi-byte fields to wire order.
2. Zero `wChecksum`.
3. `crc = proto_compute_checksum(0, frame+0x0E, wPayload_len + 10)`.
4. Store `htons(crc)` at `frame.wChecksum`.

`proto_verify_checksum(frame)` (RX):
1. Save the received `wChecksum`, then zero the field in place.
2. Recompute the CRC identically.
3. **On match** → `proto_postprocess(frame)` (byte-swap payload to host order),
   return `0`. **On mismatch** → return `-1`, frame dropped.

The verify path byte-swaps the payload **after** validating — so a failed
checksum leaves the frame untouched and discarded.

## Byte-order layer (`proto_postprocess`)

`proto_postprocess` is not part of the checksum itself, but it determines which
byte order the multi-byte fields are in when checksummed, so it matters for TX.
It branches on `bMagic` (`0x10` vs `0x11`) and `bSubtype` to byte-swap the
appropriate payload fields (mostly `htonl`/`htons` on stat counters and length
fields). For the checksum this just means: **build the frame big-endian**, then
checksum. The full per-subtype swap table belongs to P1/P2 (TX/RX layouts) and is
captured alongside the payload structures there.

`bMagic` ∈ {`0x10`, `0x11`} selects the payload layout family; the exact
command/response assignment is confirmed during P1. Both are covered by the
checksum regardless.

## Python reference

A runnable, self-testing implementation: [../examples/checksum.py](../examples/checksum.py).

```bash
python3 examples/checksum.py
# [ok] nibble-table port == CRC-16/ARC for all tested lengths
# [ok] sample frame checksum set+verify: 0x1ea0
# [ok] corruption detected (covered bit flip rejected)
# [ok] uncovered byte (dst_mac) change correctly ignored
```

It provides:
- `proto_compute_checksum(acc, buf)` — byte-for-byte port of the C nibble loop.
- `crc16_arc(data)` — standard bit-by-bit CRC-16/ARC (cross-validation).
- `set_checksum(frame)` / `verify_checksum(frame)` — frame-level helpers.

## Test vector

The self-test's synthetic `dsl_config_down` frame (opcode 3, no payload,
`seq = 1`, broadcast, magic `0x11`) checksums to **`0x1ea0`**. Reproduce:

```python
from examples.checksum import _build_sample_frame, set_checksum, OFF_CHECKSUM
import struct
f = _build_sample_frame(); set_checksum(f)
print(f"{struct.unpack_from('>H', f, OFF_CHECKSUM)[0]:#06x}")   # 0x1ea0
```
