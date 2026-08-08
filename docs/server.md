# Board-side `remote_board` — the 0x88B5 server

The EcoNet EN7516 board runs its own `/userfs/bin/remote_board` daemon (MIPS32
BE, 110 KB, stripped). It is the **server endpoint** of the EtherType `0x88B5`
protocol — the counterpart to the host-side `remote_board` client we documented
throughout the rest of this documentation.

## Startup

Started by `/usr/etc/init.d/rcS` as the last step, after all kernel modules
are loaded:

```sh
/userfs/bin/remote_board &
```

`main()` calls `msg_init()` then retries `msg_initServer()` every 3 seconds
until the raw socket is successfully bound.

### `msg_initServer`

```
1. msg_createRawSocket("eth0.1", &mac, 0x88B5)
     socket(AF_PACKET, SOCK_RAW, htons(0x88B5))
     setsockopt(SO_BROADCAST)
     ioctl(SIOCGIFHWADDR) → store board MAC
2. msg_srvInit() → BPF filter setup
3. htons(0x88B5) stored in frame template
4. Signal handlers: SIGTERM/SIGHUP → cleanup, SIGINT → ignore
5. system("ifconfig eth0.1.500 up")  (approximate)
```

### `msg_serveForever`

Main loop — `select()` on the raw socket fd with 50-second timeout:

```
while (true):
    select(fd, timeout=50s)
    if data ready:
        recv_frame()
        dispatch_frame()     ← opcode lookup + handler call
    if timeout:
        msg_handleIdle()     ← pending response cleanup
```

## Dispatch table

12-entry table at `0x429144`, each entry 8 bytes (opcode byte + 3 pad +
4-byte function pointer in big-endian). The dispatcher (`msg_dispatch` at
`0x0040c5a4`) also implements **duplicate-frame detection**: if the incoming
sequence number matches the last one (and opcode ≠ 8), it resends the cached
response instead of re-executing the handler.

| Opcode | Handler (board) | Source file | Resp size | Board-side action |
|--------|-----------------|-------------|-----------|-------------------|
| 1 | `board_op1_handler` → `dslCfgSet` | `msg_handler_econet.c` | 4 B | Parse config: set modulation, annex, bitswap (`"w dmt aoc bitswap on on"`), SRA, link type via `/proc/tc3162/tcci_cmd` |
| 2 | `board_op2_handler` → `dslCfgGet` | `msg_handler_econet.c` | **63 B** | Read `/proc/tc3162/adsl_stats`: link status, US/DS rates, noise margin, attenuation, output power, attainable rates, CRC errors, ATM/PTM flag, uptime |
| 3 | `board_op3_handler` | — | 4 B | Retrain/reset DSL line (calls config-apply, returns 0) |
| 4 | `board_op4_handler` → `dslGetStats` | `msg_handler_econet.c` | **28 B** | Parse `/proc/tc3162/xdsl_stats`: ES, SES, UAS, LOS, LOF, FEC (upstream + downstream pairs) |
| 5 | `board_op5_handler` (`handleDslLinkAdd`) | `msg_handler_econet.c` | 4 B | Add ATM link: `dsl_linkAdd(payload)` with VPI/VCI/encap/linkType/vlanId. Precondition: DSL UP + ATM mode |
| 6 | `board_op6_handler` (`handleDslLinkSet`) | `msg_handler_econet.c` | 4 B | Set/delete ATM link: `vlanId` range **2000–2007**, cmd=3→delete (`dsl_linkDel`), else→set status (`dsl_linkSet`) |
| 7 | `board_op7_handler` | — | 4 B | Reboot board: send response, then `sdk_reboot()` |
| 8 | `board_op8_handler` | — | varies | 4-stage firmware upload (see [firmware.md](commands/firmware.md)) |
| 9 | `board_op9_handler` | — | 4 B | **LED control**: `echo <mode> > /proc/tc3162/led_off_mode`. Byte 0 = direct/off mode, byte 1 = value |
| 15 | `board_op15_handler` (`handleVdslLinkAdd`) | `msg_handler_econet.c` | 4 B | Add PTM/VDSL link: `vdsl_linkAdd(payload)` with vlanEnabled/vid/qosMark/linkType/vlanId. Precondition: DSL UP + PTM mode |
| 16 | `board_op16_handler` (`handleVdslLinkSet`) | `msg_handler_econet.c` | 4 B | Set/delete PTM/VDSL link: same `vlanId` range 2000–2007, same cmd semantics as opcode 6 |
| 20 | `board_op20_handler` | — | 4 B | **Interface query**: get MAC of `eth0.1.500`, re-init socket |

## `dslStatusCheckHandler` thread

Spawned by `dslCreateThreadChkStatus()` as a detached pthread. Polls the DSL
driver directly:

```c
while (true) {
    status = dsl_getStatus();       // read /proc/tc3162/adsl_stats
    if (status != last_status) {
        if (status == UP) {
            getSysUpTime(&g_lastUpMoment);
            dslHandleStatusUp();    // create VLAN eth0.1.<vlan>, bring up
        } else if (last_status == UP) {
            g_lastUpMoment = 0;
            dslHandleStatusDown();  // remove VLAN, bring down
        }
    }
    last_status = status;
    sleep(2);                       // poll every 2 seconds
}
```

The board polls **every 2 seconds** — faster than the host's `cos` daemon
(10 seconds). The board detects status transitions first, creates/removes
the data-plane VLAN interface, and then the host's slower poll picks up the
change for LED driving.

## DSL driver interface

The board-side `remote_board` controls the TC3162 DSL chipset via procfs:

| Path | Purpose |
|------|---------|
| `/proc/tc3162/adsl_stats` | ADSL line statistics (rates, noise, attenuation, power, CRC) |
| `/proc/tc3162/xdsl_stats` | Extended error counters (ES/SES/UAS/LOS/LOF/FEC) |
| `/proc/tc3162/adsl_fwver` | DSL chipset firmware version |
| `/proc/tc3162/vdsl_interface_config` | VDSL interface configuration |
| `/proc/tc3162/tcci_cmd` | Direct chipset command interface |
| `/proc/tc3162/led_off_mode` | LED on/off mode control |
| `/proc/tc3162/vlan_tag_sw` | VLAN tagging switch |
| `/proc/tc3162/wan_ports` | WAN port VLAN configuration |

Example chipset commands (written to `tcci_cmd`):
```
w dmt aoc bitswap on on     ← enable bitswap
w dmt aoc bitswap off off    ← disable bitswap
wan ghs set annex <type>     ← set annex type
```

## Data-plane VLAN

The board creates VLAN interfaces for ATM and PTM connections:

```
eth0.1 → vconfig add eth0.1 500 → eth0.1.500 (management VLAN)
eth0.1 → vconfig add eth0.1 <vlanId> → eth0.1.<vlanId> (per-PVC data VLAN)
```

VLAN ID range: **2000–2007** (enforced in opcodes 6 and 16, matching the
host-side `oal_vlanIdFromIfName` rule).

ATM links use `atmCreateVlanMuxIntf` (LLC/VCMUX encapsulation), PTM links
use `ptmCreateVLAN`.

---

## Client vs server opcode comparison

The host-side `remote_board` has a **13-entry** dispatch table; the
board-side has **12 entries**. The tables below compare every opcode.

### Full comparison table

| Opcode | Host (client) | Board (server) | Match | Status | Purpose |
|--------|---------------|----------------|-------|--------|---------|
| **1** | ✓ | ✓ `dslCfgSet` | ✓ | **Active** | Set DSL config (modulation, annex, bitswap, SRA, link type) |
| **2** | ✓ | ✓ `dslCfgGet` | ✓ | **Active** | Get DSL status (link, rates, noise, attenuation, power, CRC, uptime) |
| **3** | ✓ | ✓ | ✓ | **Active** | DSL line retrain / config apply |
| **4** | ✓ | ✓ `dslGetStats` | ✓ | **Active** | Get channel statistics (ES/SES/UAS/LOS/LOF/FEC) |
| **5** | ✓ | ✓ `handleDslLinkAdd` | ✓ | **Active** | Add ATM link (VPI/VCI/encap/linkType/vlanId) |
| **6** | ✓ | ✓ `handleDslLinkSet` | ✓ | **Active** | Set/delete ATM link (vlanId 2000–2007, cmd) |
| **7** | ✓ | ✓ | ✓ | **Active** | Reboot board |
| **8** | ✓ | ✓ | ✓ | **Active** | Firmware upload (4-stage: announce/stream/verify/complete) |
| **9** | ✓ (dead) | ✓ | ✓ | **Board-only active** | LED control via `/proc/tc3162/led_off_mode` |
| **14** | ✓ (dead) | ✗ | ✗ | **Truly dead** | No handler on board, never sent by host |
| **15** | ✓ | ✓ `handleVdslLinkAdd` | ✓ | **Active** | Add PTM/VDSL link (vlanEnabled/vid/qosMark/linkType/vlanId) |
| **16** | ✓ | ✓ `handleVdslLinkSet` | ✓ | **Active** | Set/delete PTM/VDSL link (vlanId 2000–2007, cmd) |
| **20** | ✓ (dead) | ✓ | ✓ | **Board-only active** | Get `eth0.1.500` MAC address + interface setup |

### Summary

| | Host (client) | Board (server) |
|--|---------------|----------------|
| **Total entries** | 13 | 12 |
| **Actively used** | 9 (opcodes 1–8, 15, 16) | 12 (all entries) |
| **Dead on host** | 3 (9, 14, 20) | — |
| **Dead on board** | — | 0 |
| **Truly dead** (both sides) | 1 (**opcode 14**) | 1 (**opcode 14**) |

### Key findings

1. **Opcode 14 is the only truly dead opcode** — it exists in the host's
   table but has no board-side handler and is never sent. It may be a
   legacy entry or placeholder.

2. **Opcodes 9 and 20 are NOT dead on the board** — the board has full
   handlers for both. They are only "dead" from the host's perspective
   (the current VX830v host firmware never sends them). They may be used
   by:
   - Older host firmware versions
   - Different TP-Link models on the same platform
   - Factory test/debug tools
   - Direct board communication bypassing the host

3. **Opcode 9** (LED control) gives the board its own LED management path
   via `/proc/tc3162/led_off_mode`, independent of the host's GPIO-driven
   LED. See [led_control.md](led_control.md).

4. **Opcode 20** queries the MAC address of `eth0.1.500` (the management
   VLAN interface) and re-initializes the socket — likely used during
   initial board discovery.

5. **Response sizes corrected**: opcode 2 returns **63 bytes** from the
   board (we documented 59 from host-side parsing); opcode 4 returns
   **28 bytes**.

6. **Opcode pairing**:
   - Op 1 ↔ Op 2: config set ↔ status get
   - Op 5/6 ↔ Op 15/16: ATM link management ↔ PTM link management
   - Op 3 (retrain) / Op 7 (reboot): maintenance
   - Op 4 (stats) / Op 9 (LED) / Op 20 (MAC): query/control
   - Op 8 (firmware): special multi-stage protocol

### Polling intervals

| Component | Interval | Purpose |
|-----------|----------|---------|
| Board `dslStatusCheckHandler` | **2 s** | Detect DSL UP/DOWN transitions, create/remove VLAN |
| Host `cos` daemon | **10 s** | Poll DSL status (opcode 2), drive host GPIO LED |

The board detects status changes 5× faster than the host. The board creates
the data-plane VLAN interface immediately on link-up; the host's slower poll
then picks up the new state for LED driving and TR-181 reporting.
