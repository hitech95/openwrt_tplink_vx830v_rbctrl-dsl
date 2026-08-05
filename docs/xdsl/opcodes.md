# Opcode Protocol (oal_remote_Cfg)

Detailed breakdown of every DSL opcode sent by `libcmm.so` through
`oal_remote_Cfg` (`0x00325300`, `./src/oal_dsl_remote.c`) to `remote_board`
(server `0x3b`).

## The sender contract

```c
int oal_remote_Cfg(int opcode, void *buf, ushort len, char expect_reply, ushort timeout);
```

- `opcode` → becomes cmm `msg_id = opcode + 0x2968`, and `remote_board` dispatch
  key `= opcode`, and `0x88B5` wire subtype `= opcode`.
- `buf` / `len` → payload copied into the cmm message body (max `0x1000`).
- `expect_reply == 0` → fire-and-forget via `msg_connCliAndSend(0x3b, …)`.
- `expect_reply != 0` → request/reply: `msg_init` + `msg_connSrv(0x3b)` +
  `msg_sendAndGetReplyWithTimeout(timeout)`. The reply body is copied back into
  `buf`; a non-zero reply status word prints `"remote cfg error:%d"`.
- `timeout` must be `≥ 0x15` (21) seconds, else error `0x2651`.

On any failure to reach server `0x3b`, prints
`"connect remote server failed!"`.

> **Exhaustive sender verification.** `oal_remote_Cfg` is the **sole** path to
> server `0x3b`: among 85+ `msg_connCliAndSend` call sites in `libcmm.so`, it is
> the only one that targets the remote-board server (every other caller belongs
> to DHCP/HTTP/VoIP/WiFi/IGMP/diag and targets a different server id). Its 16
> call sites (across 15 wrappers) pass opcodes **{1, 2, 3, 4, 5, 6, 7, 8, 15,
> 16}** only. Opcodes **9, 14, 20 have no host sender** — their handlers in
> `remote_board` are vestigial/dead code in this build. Confirmed 2026-08 via
> full `oal_remote_Cfg` caller enumeration + `msg_connCliAndSend` cross-check.

## Opcode 1 — DSL line config UP

| Field | Value |
|-------|-------|
| Caller | `oal_dsl_configUp(lineObj)` |
| Payload | 12 bytes — serialized line object (`dslLineObjToMsg`) |
| Reply | no (fire-and-forget) |
| `remote_board` | `dsl_config_up` |

Pushes the **DSL line configuration** to the board: modulation type, annex type,
and line parameters. This is how the host tells the remote DSL line which
standard to train with (see [layers.md](layers.md)).

## Opcode 2 — GET DSL line/channel/status

| Field | Value |
|-------|-------|
| Callers | `oal_getDev2DslLineObj`, `oal_getDev2DslLineStatsObj`, `oal_getDev2DslChannelObj`, `oal_dsl_getConfigModulateType` |
| Payload | 59 bytes (`0x3b`) returned from board |
| Reply | **yes**, timeout 3 s |
| `remote_board` | `dsl_get_line_obj` |

Reads back the current DSL line/channel object (TR-181 `Device.DSL.Line.{i}.`).
All four callers use the same 59-byte buffer and the same reply path; they differ
only in how they deserialize the response:

- `oal_getDev2DslLineObj` → fills the TR-181 line object; on failure sets
  line status to `"NoSignal"`.
- `oal_dsl_getConfigModulateType` → inspects the **modulation byte** at offset
  `0x05` of the response: `6` ⇒ VDSL2 (returns 2), otherwise ADSL (returns 1).
  (Field map fully resolved in [responses.md](responses.md).)

## Opcode 3 — DSL config DOWN

| Field | Value |
|-------|-------|
| Caller | `oal_dsl_configDown(void)` |
| Payload | none |
| Reply | no |
| `remote_board` | `dsl_config_down` |

Tears down the active DSL line configuration on the board.

## Opcode 4 — GET channel total stats

| Field | Value |
|-------|-------|
| Caller | `oal_getDev2DslChannelStatsTotObj` |
| Payload | 28 bytes (`0x1c`) returned |
| Reply | **yes**, timeout 3 s |
| `remote_board` | `dsl_get_channel_stats` |

Reads aggregate DSL channel statistics (TR-181
`Device.DSL.Channel.Stats.Total.`).

## Opcode 5 — ATM link add

| Field | Value |
|-------|-------|
| Callers | `oal_atm_setVlanTag`, `oal_atm_addTestIntf` |
| Payload | 24 bytes (`0x18`) — ATM link + QoS + VLAN-tag descriptor |
| Reply | no |
| `remote_board` | `atm_link_add` |

Adds an ATM link on the board. `remote_board` then discovers the VLAN id from the
board's reply and runs `ifconfig lan0.<vlan> up` locally — i.e. **ATM link
provisioning**.

`oal_atm_addTestIntf` uses a hardcoded VLAN of **2000** (`lan0.2000`);
`oal_atm_setVlanTag` computes `vlanid = linkObj.vlan + 2000`.

## Opcode 6 — ATM link del

| Field | Value |
|-------|-------|
| Callers | `oal_atm_setAtmIfStatus` (`FUN_00324d38`), `oal_atm_delVlanTag` |
| Payload | 3 bytes (vlan id + status/type byte) |
| Reply | no |
| `remote_board` | `atm_link_del` |

Sets ATM interface status or deletes an ATM VLAN tag. When the type byte is `3`
(delete), `remote_board` first runs `iface_vlan_down(vlan_id)` locally to tear
down `lan0.<vlan>`, then forwards the 3-byte delete to the board. Symmetric
counterpart of opcode 5.

## Opcode 7 — main-board image check

| Field | Value |
|-------|-------|
| Caller | `oal_remote_upgradeImage` (when `param_2 == 0`) |
| Payload | none |
| Reply | no |
| `remote_board` | `main_image_check` — 100 s timeout |

`oal_remote_upgradeImage` logs `"upgradeImage only contains Image of Main board"`
and sends opcode 7 with no payload. This is the long operation (100-second
timeout) — a main-board firmware validation/commit step.

## Opcode 8 — firmware upgrade

| Field | Value |
|-------|-------|
| Caller | `oal_remote_upgradeImage` (when `param_2 != 0`) |
| Payload | 128 bytes — `/var/tmp/remoteflash.bin` path |
| Reply | **yes**, timeout 15 (`0xf`) |
| `remote_board` | `firmware_upgrade` |

`oal_remote_upgradeImage` writes the image to `/var/tmp/remoteflash.bin`, sends
the path via opcode 8, waits for the result, then `unlink`s the temp file and
`sleep(30)`. Full wire protocol: see [../commands/firmware.md](../commands/firmware.md).

## Opcode 9 — 2-B forward

| Field | Value |
|-------|-------|
| Caller | **none in `libcmm.so`** (**DEAD** — no sender) |
| Payload | 2 bytes (a single `ushort`) |
| Reply | no |
| `remote_board` | `cmd9_forward` |

Forwards a 2-byte value to the board on `0x88B5` subtype 9. No local side-effect.
**Confirmed dead** — no host sender exists. `oal_remote_Cfg` (the sole
server-`0x3b` bridge) is never called with opcode 9, and no other path to server
`0x3b` exists in `libcmm.so` (see the verification note at the top). The board
implements the handler but this host build never invokes it; semantics would only
surface from a board-firmware analysis.

## Opcode 14 — 7-B forward

| Field | Value |
|-------|-------|
| Caller | **none in `libcmm.so`** (**DEAD** — no sender) |
| Payload | 7 bytes |
| Reply | no |
| `remote_board` | `cmd14_forward` |

Forwards a 7-byte payload to the board on `0x88B5` subtype 14. No local
side-effect. **Confirmed dead** — same basis as opcode 9 (no `oal_remote_Cfg`
call passes opcode 14; sole-bridge verification at top of page).

## Opcode 15 (0xF) — PTM link add

| Field | Value |
|-------|-------|
| Caller | `oal_ptm_setVlanTag` |
| Payload | 8 bytes (VLAN-tag descriptor) |
| Reply | no |
| `remote_board` | `ptm_link_add` |

VDSL/PTM equivalent of opcode 5. Sends the 8-byte VLAN descriptor to the board;
on the board's OK reply it runs `iface_vlan_up(vlan_id, priority)` locally to
create `lan0.<vlan>`. If the board rejects the config, the local interface is
**not** created and the handler returns an error — failures are observable, not
silent.

## Opcode 16 (0x10) — PTM link del

| Field | Value |
|-------|-------|
| Caller | `oal_ptm_delVlanTag` |
| Payload | 3 bytes (vlan id + type=3) |
| Reply | no |
| `remote_board` | `ptm_link_del` |

VDSL/PTM equivalent of opcode 6. When the type byte is `3`, runs
`iface_vlan_down(vlan_id)` locally, then forwards the delete to the board.

## Opcode 20 (0x14) — board identity

| Field | Value |
|-------|-------|
| Caller | **none — confirmed dead** |
| `remote_board` | `board_identity_check` — MAC `memcmp` verify |

Handled by `remote_board` but, like opcodes 9 and 14, **never invoked from the
host**: the sole-bridge verification (top of page) shows no `oal_remote_Cfg` call
passes opcode 20, and no bypass path to server `0x3b` exists. Performs a 6-byte
MAC identity check on the board — likely a factory/test capability reserved for
vendor tooling not present in this rootfs.
