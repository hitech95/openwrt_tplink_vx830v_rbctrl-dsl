# xDSL Modulation & Annex Map

Authoritative map of valid **modulation types**, **annex types**, and **VDSL2
profiles** for the EcoNet xDSL board, extracted directly from the firmware
rootfs (`squashfs-root/lib/libcmm.so`) and cross-checked against the web UI
sources (`squashfs-root/web/main/dslcfg.htm`).

> These three enums populate **opcode 1** (`dsl_config_up`) and are reported
> back in **opcode 2** (`oal_dsl_msgToLineObj`). For the byte-level payload
> layout see [payloads.md](payloads.md); for the response layout see
> [responses.md](responses.md).

---

## Source globals in `libcmm.so`

| Global | Address | Layout | Entries | Role |
|--------|---------|--------|---------|------|
| `modulationTypes` | `0x003edc20` | 32 B/entry | 8 | code + 3 pointers |
| `annexTypes` | `0x003edd20` | 16 B/entry | 9 | code + 1 pointer |
| `profiles` | `0x003eddb0` | 16 B/entry | 9 | bitmask + 1 pointer |

Each `modulationTypes` entry is:

```c
struct modulationType {
    uint64_t code;          // offset 0x00 — wire byte [0] of opcode 1
    char    *canonical;     // offset 0x08 — e.g. "ADSL_G.dmt"   (config / TR-181)
    char    *standard;      // offset 0x10 — e.g. "G.992.1"       (ITU-T designation)
    char    *valid_annexes; // offset 0x18 — e.g. "ABCIJLM"       (letters, or NULL)
};  // sizeof = 0x20
```

The `valid_annexes` string lists the **ITU-T annex letters** that modulation
standard supports. It drives `formatStandsAnnexStr` (`FUN_00323b88`), which
emits the TR-181 `StandardsSupported` string `"<std>_Annex_<L>"` for each
letter of the configured annex that is also in `valid_annexes`. `NULL` means
the standard has no annex concept (T1.413, G.lite).

---

## Modulation types — opcode 1 byte `[0]` / response byte `[5]`

| Code | Canonical name (`X_TP_ModulationType`) | ITU-T standard | Valid annexes | Transport |
|-----:|----------------------------------------|----------------|---------------|-----------|
| `0`  | `ADSL_ANSI_T1.413`                     | `T1.413`       | — (none)      | ATM       |
| `1`  | `ADSL_G.dmt`                           | `G.992.1`      | `ABC`         | ATM       |
| `2`  | `ADSL_G.lite`                          | `G.992.2`      | — (none)      | ATM       |
| `3`  | `ADSL_G.dmt.bis`                       | `G.992.3`      | `ABCIJM`      | ATM       |
| `4`  | `ADSL_2plus`                           | `G.992.5`      | `ABCIJLM`     | ATM       |
| `5`  | `ADSL_Multimode`                       | `G.992.x`      | `ABCIJLM`     | ATM       |
| `6`  | `VDSL2`                                | `G.993.2`      | `ABCIJLM`     | **PTM**   |
| `7`  | `Multimode`                            | `G.99x`        | `ABCIJLM`     | PTM/ATM   |

> **Detection rule** (`oal_dsl_getConfigModulateType`): `code == 6` → VDSL2 /
> PTM path (opcode 15/16); anything else → ADSL / ATM path (opcode 5/6).
> Codes `5` and `7` are **config-only** multimode selectors — they never
> appear as an *active* modulation in opcode-2 responses (only `0,1,2,3,4,6`).

String table evidence in `libcmm.so`:

```
0x003b0e08  ADSL_ANSI_T1.413   0x003b0e20  T1.413     0x003b0e40  ABC
0x003b0e28  ADSL_G.dmt         0x003b0e38  G.992.1    0x003b0e78  ABCIJM
0x003b0e48  ADSL_G.lite        0x003b0e58  G.992.2    0x003b0e98  ABCIJLM
0x003b0e60  ADSL_G.dmt.bis     0x003b0e70  G.992.3
0x003b0e80  ADSL_2plus         0x003b0e90  G.992.5
0x003b0ea0  ADSL_Multimode     0x003b0eb0  G.992.x
0x003b0eb8  VDSL2              0x003b0ec0  G.993.2
0x003b0ec8  Multimode          0x003b0ed8  G.99x
```

---

## Annex types — opcode 1 byte `[1]` / response byte `[7]`

| Code | Name (`X_TP_AnnexType`) | Letters | Region / use |
|-----:|-------------------------|---------|--------------|
| `0`  | `Annex A`               | A       | POTS (worldwide, common) |
| `1`  | `Annex B`               | B       | ISDN (Europe) |
| `2`  | `Annex I`               | I       | POTS, all-digital (spectrum-optimized) |
| `3`  | `Annex M`               | M       | POTS, extended upstream |
| `4`  | `Annex A/L`             | A, L    | Annex A with L band |
| `5`  | `Annex A/L/M`           | A, L, M | Annex A with L and M bands |
| `6`  | `Annex J`               | J       | All-digital ISDN, extended upstream |
| `7`  | `Annex B/J`             | B, J    | Annex B with J band |
| `8`  | `Annex auto`            | —       | Board selects automatically |

String table at `0x003b0ee0`:

```
0x003b0ee0  Annex A     0x003b0ef0  Annex I     0x003b0f00  Annex A/L
0x003b0ee8  Annex B     0x003b0ef8  Annex M     0x003b0f10  Annex A/L/M
0x003b0f20  Annex J     0x003b0f28  Annex B/J   0x003b0f38  Annex auto
```

> A second short list at `0x00349370` (`Annex A/L`, `Annex A/L/M`, `Annex M`,
> `Annex A`, `Annex B`) is used only by the WAN-add code path.

---

## VDSL2 profile bitmask — opcode 1 bytes `[4..7]` (BE u32)

Only populated when modulation ∈ {`6` VDSL2, `7` Multimode}; zero for all ADSL
modes. Multiple profiles OR together. Sent big-endian; bytes `[8..11]` are
always zero.

| Bit | Profile | Bit | Profile |
|-----|---------|-----|---------|
| `0x001` | `8a`  | `0x010` | `12a` |
| `0x002` | `8b`  | `0x020` | `12b` |
| `0x004` | `8c`  | `0x040` | `17a` |
| `0x008` | `8d`  | `0x080` | `30a` |
|         |       | `0x100` | `35b` |

Built by `oal_dsl_lineObjToMsg` from the TR-181 `allowedProfile` string
(`strtok_r` on `;`,`, case-insensitive match against the `profiles` table).

---

## Valid modulation × annex combinations

The `valid_annexes` field of each modulation defines the **ITU-T-compliant**
annex letters. The firmware does not hard-reject non-compliant pairs (it only
filters the `StandardsSupported` display string), but the combinations below
are the ones the web UI offers and that the board trains against.

**Legend:** ✓ = all annex letters valid · ~ = partial (some letters outside the
standard) · ✗ = not standard-compliant · — = modulation has no annex concept ·
the `auto` annex (code `8`) is valid for every modulation.

| Modulation (code)            | Standard   | `A` | `B` | `I` | `M` | `A/L` | `A/L/M` | `J` | `B/J` | `auto` |
|------------------------------|------------|:---:|:---:|:---:|:---:|:-----:|:-------:|:---:|:-----:|:-----:|
| `ADSL_ANSI_T1.413` (0)       | T1.413     |  —  |  —  |  —  |  —  |   —   |    —    |  —  |   —   |   ✓   |
| `ADSL_G.dmt` (1)             | G.992.1    |  ✓  |  ✓  |  ✗  |  ✗  |   ~   |    ~    |  ✗  |   ~   |   ✓   |
| `ADSL_G.lite` (2)            | G.992.2    |  —  |  —  |  —  |  —  |   —   |    —    |  —  |   —   |   ✓   |
| `ADSL_G.dmt.bis` (3)         | G.992.3    |  ✓  |  ✓  |  ✓  |  ✓  |   ~   |    ~    |  ✓  |   ✓   |   ✓   |
| `ADSL_2plus` (4)             | G.992.5    |  ✓  |  ✓  |  ✓  |  ✓  |   ✓   |    ✓    |  ✓  |   ✓   |   ✓   |
| `ADSL_Multimode` (5)         | G.992.x    |  ✓  |  ✓  |  ✓  |  ✓  |   ✓   |    ✓    |  ✓  |   ✓   |   ✓   |
| `VDSL2` (6)                  | G.993.2    |  ✓  |  ✓  |  ✓  |  ✓  |   ✓   |    ✓    |  ✓  |   ✓   |   ✓   |
| `Multimode` (7)              | G.99x      |  ✓  |  ✓  |  ✓  |  ✓  |   ✓   |    ✓    |  ✓  |   ✓   |   ✓   |

> **Note on `C`:** the `valid_annexes` strings contain `C` (Annex C,
> TCM-ISDN, Japan) but `C` has no entry in the selectable `annexTypes` table —
> it surfaces only in the formatted `StandardsSupported` string. This is why
> `G.992.1` lists `ABC` but only `A`/`B` are selectable as single-letter
> annexes.

### How the matrix is computed

For a given (modulation `M`, annex `X`):

1. Take `M.valid_annexes` (e.g. `ABCIJM` for G.992.3).
2. Take the letter set of `X` (e.g. `A/L` → {A, L}).
3. **✓** if every letter of `X` is in `M.valid_annexes`.
4. **~** if some letters match and some don't (firmware accepts; the
   `StandardsSupported` string lists only the matching letters).
5. **✗** if no letter matches.
6. **—** if `M.valid_annexes` is `NULL` (T1.413, G.lite — annex is irrelevant).
7. `auto` is always valid — the board performs its own selection.

---

## Web UI mapping (`X_TP_SupportedDslMode`)

The web UI (`dslcfg.htm`) drives the two dropdowns from a single semicolon-
separated string served by the data model:

```
<UI-modulation>:<annex-letter>,<annex-letter>,...;<UI-modulation>:...;...
```

The UI modulation label maps to the canonical name via this table
(`moduValArray` in `dslcfg.htm`):

| UI label             | Canonical name        | Code |
|----------------------|-----------------------|-----:|
| `Auto Sync-up`       | `Multimode`           | `7`  |
| `ADSL Auto Sync-up`  | `ADSL_Multimode`      | `5`  |
| `T1.413`             | `ADSL_ANSI_T1.413`    | `0`  |
| `G.lite`             | `ADSL_G.lite`         | `2`  |
| `G.dmt`              | `ADSL_G.dmt`          | `1`  |
| `ADSL2`              | `ADSL_G.dmt.bis`      | `3`  |
| `ADSL2+`             | `ADSL_2plus`          | `4`  |
| `VDSL2`              | `VDSL2`               | `6`  |

The annex-letter list per modulation is exactly the `valid_annexes` string
from `modulationTypes` (with `C` filtered out of the selectable set). The
build-time flags `INCLUDE_VDSLWAN` and `INCLUDE_ADSL_ECN` further hide
`VDSL2` / `ADSL Auto Sync-up` on ADSL-only builds.

The configuration is written back as TR-181 vendor extensions on
`Device.DSL.Line.{i}.`:

```js
{
    X_TP_ModulationType: "<canonical>",   // e.g. "ADSL_G.dmt.bis"
    X_TP_AnnexType:       "Annex <L>",    // e.g. "Annex A"
    X_TP_SRAEnable:       0|1,
    X_TP_BitswapEnable:   0|1,
    ...
}
```

`oal_dsl_lineObjToMsg` then resolves these strings to the numeric codes above
via linear scan of `modulationTypes` / `annexTypes`, producing opcode 1's
12-byte payload.

---

## Wire encoding recap (opcode 1, `dsl_config_up`)

| Offset | Size | Field | Source |
|--------|------|-------|--------|
| `0x00` | 1 | modulation code | this doc, table 1 |
| `0x01` | 1 | annex code      | this doc, table 2 |
| `0x02` | 1 | line-config byte | TR-181 obj `+0x2c9` |
| `0x03` | 1 | line-config byte | TR-181 obj `+0x2ca` |
| `0x04` | 4 | VDSL2 profile bitmask (BE) | this doc, table 3 |
| `0x08` | 4 | reserved (`0x00000000`) | — |

See [payloads.md](payloads.md) for the full per-opcode byte maps.

---

## Cross-references

- [payloads.md](payloads.md) — TX payload layouts, original enum tables
- [responses.md](responses.md) — opcode 2/4 reply parsing (active modulation/annex)
- [layers.md](layers.md) — ATM vs PTM transport selection, annex/modulation narrative
- [../map.md](../map.md) — `modulationTypes` / `annexTypes` / `profiles` symbol addresses
