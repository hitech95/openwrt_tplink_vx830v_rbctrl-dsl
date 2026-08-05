# Hunt: search kit for missing binaries & artifacts

Practical triage kit for the artifacts listed in [reverse-engineering-plan.md](../plans/reverse-engineering-plan.md) §"Suspected
missing binaries & artifacts". Work cheapest-filter-first.

> **Vendor fact:** the board's DSL SoC is **EcoNet** (confirmed). The
> `DMVS_ADSL`/`DMVS_VDSL` strings seen in the host binaries are EcoNet's DSL
> management API. See [reverse-engineering-plan.md](../plans/reverse-engineering-plan.md) §3 for the architecture note: the board
> SoC is most likely **MIPS**, while `remote_board` (host) is AArch64.

---

## Step 1 — fastest filter: who links `libcmm.so`?

Any host binary talking to `remote_board` goes through libcmm. Filter by
`DT_NEEDED` first — far cheaper than grepping strings blindly.

```bash
find /path/to/rootfs -type f -exec sh -c '
  file "$1" | grep -q ELF || exit
  readelf -d "$1" 2>/dev/null | grep -q "libcmm" && echo "$1"
' _ {} \;
```

This is your **candidate list**. Everything below runs against this set (or the
whole rootfs if you have no candidate filter).

---

## Step 2 — strings to grep

Grouped by what they tell you. Order = signal-to-noise.

### A. Definitive "talks to remote_board" signals

| Pattern | Meaning |
|---------|---------|
| `remote_board` | the server's own name / log tag |
| `msg_connCliAndSend` | the cmm send primitive (symbol or string) |
| `oal_remote_Cfg` | the libcmm → server-`0x3b` bridge |
| `libcmm` | cmm client library reference |
| `cmm_server` / `cmm_init` / `cmm_event` | cmm framework symbols |
| `lan0` | the base interface (lan0.500, lan0.<vlan>) |
| `0x88b5` / `88b5` | board management ethertype (often in debug/log strings) |

### B. Opcode 9 / 14 / 20 senders (the unknowns)

The message IDs are immediates (`0x2968 + op`), so they won't appear in
`strings`. Two angles:

```bash
# Angle 1: among libcmm linkers, find DSL/ATM/PTM-aware ones
strings -a -n 4 <bin> | grep -iE 'dsl|vdsl|adsl|atm|ptm|vpi|vci|annex|modulation'

# Angle 2: disassemble candidates in Ghidra and look for calls to
#          oal_remote_Cfg with literal opcode 9, 14, or 20.
#          Decimal forms (10609 / 10614 / 10620) occasionally appear
#          in debug logging — worth a cheap grep:
strings -a -n 4 <bin> | grep -E '0x297[16c]|1060[9]|1061[40]|10620'
```

Likely senders to inspect first: `tr069`/`cwmpd`, `boardctl`, `diagd`, anything
in `/usr/sbin` or `/usr/bin`.

### C. EcoNet chipset fingerprint

The board SoC is EcoNet. Confirm and narrow the exact part / SDK family:

| Pattern | Meaning |
|---------|---------|
| `econet` / `EcoNet` / `ECONET` | vendor self-identification |
| `EN75` / `EN7512` / `EN7528` / `EN7562` / `EN758` | EcoNet EN75xx DSL SoC family (MIPS) |
| `DMVS_ADSL` / `DMVS_VDSL` | EcoNet DSL management API (already seen in host binaries) |
| `dmvs_` | EcoNet API function prefix |
| `tn_` | EcoNet driver-internal prefix (titanium? — confirm on hit) |
| `EN_` / `en_` | EcoNet generic prefix (noisy — corroborate with DMVS_) |

```bash
strings -a -n 4 <bin> | grep -iE 'econet|en75[0-9]|dmvs|tn_'
```

A hit on `DMVS_` + `EN75xx` + `econet` pins the exact SoC family and tells you
which GPL/driver drop to hunt for.

### D. Board firmware / upgrade-path clues

```bash
strings -a -n 4 <bin> | grep -iE 'remoteflash|remote_upgrade|firmware|flash|upgrade|reboot'
```

Specifically watch for:
- `/var/tmp/remoteflash.bin` — the staged upgrade image (the practical way to
  obtain a board firmware copy; see [reverse-engineering-plan.md](../plans/reverse-engineering-plan.md) §1)
- `remote_upgrade` — the completion marker `firmware_upgrade` writes

If any binary *generates* or *fetches* that staging file, you've found the image
source and an upgrade trigger.

---

## Step 3 — one-shot rootfs sweep

Score every ELF by how many of our identifiers it carries. Sorted by hit count,
the top entries are the highest-priority binaries to pull into Ghidra.

```bash
find /path/to/rootfs -type f -exec sh -c '
  file "$1" | grep -q ELF || exit
  hits=$(strings -a -n 4 "$1" | grep -iEc "remote_board|oal_remote|msg_connCli|dmvs_|econet|en75|remoteflash|0x88b5|lan0\.")
  [ "$hits" -gt 0 ] && printf "%4d  %s\n" "$hits" "$1"
' _ {} \; | sort -rn
```

---

## Step 4 — for the board firmware image (if/when obtained)

If you intercept `remoteflash.bin` (see [reverse-engineering-plan.md](../plans/reverse-engineering-plan.md) §1), the same kit
applies *inside* it:

- **Load into Ghidra as MIPS** (EcoNet EN75xx family), not ARM. Endianness
  depends on the exact part — try LE first, fall back to BE.
- `0x88b5` handling → the `0x88B5` responder state machine (the other half of
  [protocol.md](protocol.md)).
- Checksum / CRC routine → cross-check `proto_compute_checksum` (closes P0).
- Response-struct builders → authoritative RX layouts (closes P2).
- EcoNet strings (`DMVS_`, `EN75xx`) → confirm chipset + match SDK version.
- U-Boot / bootloader strings → flash layout and reboot semantics.

---

## Quick reference: the constants

| Value | Where | Meaning |
|-------|-------|---------|
| `0x88b5` | ethertype | board management (host ↔ board) |
| `0x88b6` | ethertype | cmm control bus (host-internal) |
| `0x3b` | cmm server id | `remote_board` |
| `0x2968` | msg_id base | `msg_id = 0x2968 + opcode` |
| `0x2971` | msg_id | opcode 9 (unknown sender) |
| `0x2976` | msg_id | opcode 14 (unknown sender) |
| `0x297c` | msg_id | opcode 20 (unknown sender) |
| `/var/tmp/remoteflash.bin` | path | staged board firmware upgrade image |
