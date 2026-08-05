# Symbol Map

Complete mapping of original (auto-generated) names → descriptive names applied
in the Ghidra database for `remote_board`. Use this as a quick lookup when
cross-referencing older analysis notes or the raw decompiler output.

> Convention: functions use `snake_case` (C style). Globals use
> `g_<hungarian><PascalCase>` (e.g. `g_nConnCount`: `g_` = global, `n` = int,
> `ConnCount` = name). See [Hungarian key](#hungarian-prefix-key) below.

## Functions

### Entry & startup

| Address | Original | Renamed | Role |
|---------|----------|---------|------|
| `0x00401330` | `entry` | `entry` | ELF `_start` (unchanged) |
| `0x00401bcc` | `FUN_00401bcc` | `main` | Program entry |
| `0x00401d44` | `FUN_00401d44` | `set_interface_name` | `-i` option handler |
| `0x0040153c` | `FUN_0040153c` | `print_usage` | `-h` help text |

### Raw socket setup (control plane, EtherType `0x88B6`)

| Address | Original | Renamed | Role |
|---------|----------|---------|------|
| `0x00402448` | `FUN_00402448` | `raw_socket_init` | Orchestrate control socket |
| `0x00401d7c` | `FUN_00401d7c` | `raw_socket_create` | `AF_PACKET` socket + `SIOCGIFHWADDR` |
| `0x00401e98` | `FUN_00401e98` | `socket_attach_bpf_filter` | Attach classic BPF program |
| `0x00401f8c` | `FUN_00401f8c` | `socket_bind_interface` | `bind(sockaddr_ll)` |

### libcmm integration

| Address | Original | Renamed | Role |
|---------|----------|---------|------|
| `0x00404880` | `FUN_00404880` | `cmm_init` | `msg_init` + `msg_srvInit(0x3B)` |
| `0x0040157c` | `FUN_0040157c` | `cmm_event_loop` | `select()` / `msg_recv` loop |
| `0x00403314` | `FUN_00403314` | `cmm_msg_handler` | Dispatch by `msg_id - 0x2968` |

### Protocol core (`0x88B5` frame I/O)

| Address | Original | Renamed | Role |
|---------|----------|---------|------|
| `0x00402bc4` | `FUN_00402bc4` | `proto_frame_init` | Build header + open socket + set seq |
| `0x00402d40` | `FUN_00402d40` | `proto_send` | Send frame (pad to min 60 B) |
| `0x00402f88` | `FUN_00402f88` | `proto_recv` | `select` + retry loop |
| `0x00402e20` | `FUN_00402e20` | `proto_recv_frame` | Recv + validate + **MAC learning** |
| `0x00402d1c` | `FUN_00402d1c` | `proto_close` | Close socket fd |
| `0x004025a4` | `FUN_004025a4` | `proto_verify_checksum` | Checksum verify (drop on mismatch) |
| `0x004031b8` | `FUN_004031b8` | `proto_compute_checksum` | CRC-style accumulator |
| `0x0040262c` | `FUN_0040262c` | `proto_postprocess` | Post-checksum processing |

### cmm command handlers (1:1 subtype mapping)

| Address | Original | Renamed | cmm msg | 0x88B5 subtype |
|---------|----------|---------|---------|----------------|
| `0x004040ac` | `FUN_004040ac` | `dsl_config_up` | `0x2969` | 1 (DSL line config: modulation+annex, 12 B) |
| `0x00404154` | `FUN_00404154` | `dsl_get_line_obj` | `0x296A` | 2 (GET DSL line/channel obj, 59 B) |
| `0x0040422c` | `FUN_0040422c` | `dsl_config_down` | `0x296B` | 3 (DSL config down) |
| `0x004042b0` | `FUN_004042b0` | `dsl_get_channel_stats` | `0x296C` | 4 (GET channel stats, 28 B) |
| `0x00404388` | `FUN_00404388` | `atm_link_add` | `0x296D` | 5 (ATM link add + VLAN provisioning) |
| `0x00404464` | `FUN_00404464` | `atm_link_del` | `0x296E` | 6 (ATM link del + local iface down) |
| `0x004035a8` | `FUN_004035a8` | `main_image_check` | `0x296F` | 7 (main-board image check, 100 s) |
| `0x00403b20` | `FUN_00403b20` | `firmware_upgrade` | `0x2970` | 8 (firmware upgrade) |
| `0x004047dc` | `FUN_004047dc` | `cmd9_forward` | `0x2971` | 9 (2-B forward, **DEAD** — no sender in libcmm) |
| `0x00404734` | `FUN_00404734` | `cmd14_forward` | `0x2976` | 14 (7-B forward, **DEAD** — no sender in libcmm) |
| `0x00404548` | `FUN_00404548` | `ptm_link_add` | `0x2977` | 15 (PTM/VDSL link add + VLAN) |
| `0x0040466c` | `FUN_0040466c` | `ptm_link_del` | `0x2978` | 16 (PTM/VDSL link del + local iface down) |
| `0x00403f4c` | `FUN_00403f4c` | `board_identity_check` | `0x297C` | 20 (board identity / MAC check, **DEAD** — no sender in libcmm) |

### Firmware stages

| Address | Original | Renamed | Stage byte | Role |
|---------|----------|---------|:----------:|------|
| `0x0040369c` | `FUN_0040369c` | `fw_announce` | 0 | Send size, board erases |
| `0x00403730` | `FUN_00403730` | `fw_stream` | 1 | 1 KB chunked windowed stream |
| `0x00403968` | `FUN_00403968` | `fw_verify` | 2 | Board verifies flashed image |
| `0x00403a18` | `FUN_00403a18` | `fw_complete` | 3 | Handshake + write upgrade marker |

### Interface / file helpers

| Address | Original | Renamed | Role |
|---------|----------|---------|------|
| `0x00402b94` | `FUN_00402b94` | `set_dest_mac` | Write to `g_abDestMac` |
| `0x00403654` | `FUN_00403654` | `file_get_size` | `stat()` → size / `-1` |
| `0x004033a4` | `FUN_004033a4` | `iface_exists` | `SIOCGIFINDEX` probe |
| `0x0040343c` | `FUN_0040343c` | `iface_wait_exists` | Poll `iface_exists` up to 49× |
| `0x004034b4` | `FUN_004034b4` | `iface_vlan_up` | `ifconfig lan0.<vlan> up` |
| `0x0040354c` | `FUN_0040354c` | `iface_vlan_down` | `ifconfig lan0.<vlan> down` |
| `0x00403d0c` | `FUN_00403d0c` | `parse_mac_string` | Hex MAC string → 6 bytes |

### Linked-list primitives (message queue)

| Address | Original | Renamed | Role |
|---------|----------|---------|------|
| `0x004013e4` | `FUN_004013e4` | `list_init` | Init self-referential sentinel head |
| `0x00401410` | `FUN_00401410` | `list_insert` | Insert node into doubly-linked list |
| `0x00401490` | `FUN_00401490` | `list_remove` | Unlink node |
| `0x0040145c` | `FUN_0040145c` | `msglist_push` | Insert pending message |
| `0x004014c0` | `FUN_004014c0` | `msglist_unlink` | Remove + reset node |
| `0x00401514` | `FUN_00401514` | `msglist_is_empty` | `(head == head->next)` |

### Control-plane serve loop (`0x88B6`, low-level)

A self-contained loop that recv/send directly on `g_nCtrlSocketFd`, separate from
the libcmm `cmm_event_loop`. Uses its own ack/retransmit protocol keyed on
`g_nCtrlSeq`.

| Address | Original | Renamed | Role |
|---------|----------|---------|------|
| `0x004022c0` | `FUN_004022c0` | `msg_serveForever` | Main loop (`"msg_serveForever looping!"`), 50 ms `select` poll |
| `0x00402098` | `FUN_00402098` | `ctrl_recv_frame` | `recv` into `g_abCtrlFrameBuf` + verify checksum |
| `0x00402180` | `FUN_00402180` | `ctrl_process_frame` | Handle type-0x15 frame; retransmit last on dup seq |
| `0x00402084` | `FUN_00402084` | `ctrl_noop_handler` | Stub callback returning 0 (new-seq handler) |
| `0x00402150` | `FUN_00402150` | `request_shutdown` | Set `g_nCtrlFlags` bit 0 (loop exit) |
| `0x00402148` | `FUN_00402148` | `serve_on_idle` | Empty idle/tick callback |
| `0x00402140` | `FUN_00402140` | `serve_on_stop` | Empty stop callback (before socket close) |
| `0x0040203c` | `FUN_0040203c` | `ctrl_socket_close` | Close `g_nCtrlSocketFd`, reset to -1 |

### Misc / lifecycle

| Address | Original | Renamed | Role |
|---------|----------|---------|------|
| `0x00401bb4` | `FUN_00401bb4` | `cmm_shutdown` | Cleanup at exit → `cmm_ctx_close` |
| `0x00404914` | `FUN_00404914` | `cmm_ctx_close` | Close cmm context socket |
| `0x004032c0` | `FUN_004032c0` | `msg_is_reliable` | Classify msg (ops 1/6/16/20) as queueable for retry |
| `0x00402534` | `FUN_00402534` | `proto_set_checksum` | Compute & set checksum before send (called by `proto_send`) |
| `0x00401050` | `FUN_00401050` | `null_call_trap` | Calls NULL (intentional trap/abort) |

> **Status: 0 auto-named functions remaining.** Every function in `remote_board`
> now carries a descriptive name.

## Global variables

| Address | Original | Renamed | Type | Role |
|---------|----------|---------|------|------|
| `0x00416168` | `s_lan0_500` / `DAT_00416168` | `g_szIfaceName` | `char[16]` | Interface name, default `"lan0.500"` |
| `0x00416178` | `DAT_00416178` | `g_nCtrlSeq` | `int` | Last control-plane seq (dup/retransmit detect) |
| `0x00416180` | `DAT_00416180` | `g_abDestMac` | `byte[6]` | Dest MAC (broadcast init, auto-learned) |
| `0x00416188` | `DAT_00416188` | *(dispatch table)* | `cmm_dispatch_entry[13]` | cmm command handler table (13 entries) |
| `0x004162e0` | `DAT_004162e0` | `g_conn_table` *(referenced)* | `undefined` | Connection records (0x28 B each) |
| `0x00416380` | `DAT_00416380` | `g_nConnCount` | `int` | Active connection count |
| `0x00416388` | `DAT_00416388` | `g_nCtrlFlags` | `int` | Control-plane flags (bit 0 = shutdown) |
| `0x00416390` | `DAT_00416390` | `g_abCtrlFrameBuf` | `byte[1514]` | Control-plane frame context buffer |
| `0x00416f64` | `DAT_00416f64` | `g_nCtrlSocketFd` | `int` | Control socket fd (0x88B6) |
| `0x00416f68` | `DAT_00416f68` | `g_dwProtoSeq` | `uint` | 0x88B5 frame sequence counter |
| `0x00416f70` | `DAT_00416f70` | `g_abCmmCtx` | `byte[224]` | libcmm context (0xE0 B) |
| `0x00404b80` | `DAT_00404b80` | `g_awCrcTable` | `uint16[16]` | CRC-16/ARC nibble table (16 entries) |

> `g_conn_table` at `0x004162e0` is referenced by name in docs but the symbol
> itself retains its auto-label (array of 0x28-byte records, layout not yet
> formalized into a struct).

## Data types (created)

| Name | Kind | Size | Layout / purpose |
|------|------|------|------------------|
| `cmm_dispatch_entry` | struct | 16 B | `{int nCmd; int nPad1; void *pHandler; int nPad2;}` — one dispatch row |
| `cmm_dispatch_table` | array | 128 B | `cmm_dispatch_entry[8]` — applied at `0x00416188` |
| `proto_frame_hdr` | struct | 24 B | `0x88B5` frame header (payload starts at 0x18, not part of this struct) |
| `mac_addr_t` | array | 6 B | `byte[6]` — Ethernet MAC |

### `proto_frame_hdr` layout

| Offset | Size | Field | Notes |
|--------|------|-------|-------|
| 0x00 | 6 | `dst_mac` | broadcast on first contact |
| 0x06 | 6 | `src_mac` | local interface MAC |
| 0x0C | 2 | `ethertype` | `0x88B5` (network order) |
| 0x0E | 1 | `magic` | `0x11` |
| 0x0F | 1 | `subtype` | == cmm command key |
| 0x10 | 4 | `seq` | `htonl(g_dwProtoSeq)` |
| 0x14 | 2 | `payload_len` | network order |
| 0x16 | 2 | `checksum` | CRC-16/ARC, network order, zeroed during compute |
| 0x18 | — | *(payload)* | payload byte 0 = `bPayload_type` (e.g. fw stage 0–3); **not part of the header struct** |

## Return-type changes (`undefined` → `int`)

These functions return `0` on success and `-1` (was displayed as `0xffffffff`)
on failure. Prototypes were set to `int` so the decompiler renders `-1`:

| Address | Function |
|---------|----------|
| `0x00404880` | `cmm_init` |
| `0x00402448` | `raw_socket_init` |
| `0x00401d7c` | `raw_socket_create` |
| `0x00401f8c` | `socket_bind_interface` |
| `0x004033a4` | `iface_exists` |
| `0x00403654` | `file_get_size` |
| `0x00403d0c` | `parse_mac_string` |
| `0x00402d40` | `proto_send` |
| `0x00402f88` | `proto_recv` |
| `0x004025a4` | `proto_verify_checksum` |

## Hungarian prefix key

| Prefix | Meaning | Example |
|--------|---------|---------|
| `g_` | global variable | `g_…` |
| `sz` | C string (`char*` / `char[]`) | `g_szIfaceName` |
| `ab` | byte array | `g_abDestMac`, `g_abCmmCtx` |
| `n` | signed `int` | `g_nConnCount`, `g_nCtrlSocketFd` |
| `dw` | unsigned `int` / dword | `g_dwProtoSeq` |
| `p` | pointer | `pHandler` (struct field) |
| `pfn` | function pointer | (used in struct fields) |

## External (libcmm.so) imports — unchanged

These remain as imported symbols (not renamed):

| Symbol | Thunk | Purpose |
|--------|-------|---------|
| `msg_init` | `0x004011d0` | init cmm context |
| `msg_srvInit` | `0x004011b0` | register cmm server |
| `msg_recv` | `0x004010c0` | receive cmm message |
| `msg_send` | `0x00401120` | send cmm message |

---

## Renamed symbols in `libcmm.so`

The OAL (Object Abstraction Layer) functions were renamed during analysis.
These live in `libcmm.so`, not `remote_board`. See [libcmm.md](libcmm.md) for
the full OAL write-up.

### Bridge function

| Address | Original | Renamed | Role |
|---------|----------|---------|------|
| `0x00325300` | `FUN_00325300` | `oal_remote_Cfg` | **Sole** host→server-0x3B bridge; wraps `msg_connCliAndSend` |

### TX serializers (payload builders)

| Address | Original | Renamed | Opcode |
|---------|----------|---------|--------|
| — | `oal_dsl_lineObjToMsg` | *(same)* | 1 (DSL line config) |
| — | `oal_atm_linkObjToMsg` | *(same)* | 5 (ATM link params) |
| — | `oal_atm_qosObjToMsg` | *(same)* | 5 (ATM QoS) |
| — | `oal_atm_setAtmIfStatus` | *(same)* | 3/5 (ATM if status) |
| — | `oal_formatPppoeCmd` | *(same)* | 8 (firmware path) |

### RX deserializers (reply parsers)

| Address | Original | Renamed | Opcode |
|---------|----------|---------|--------|
| — | `oal_dsl_msgToLineObj` | *(same)* | 2 (line status + metrics) |
| — | `oal_dsl_msgToChannelObj` | *(same)* | 2 (channel sub-obj slicer) |
| — | `oal_dsl_msgToLineStatsObj` | *(same)* | 2 (stats sub-obj slicer) |
| — | `oal_dsl_msgToChannelStatsTotObj` | *(same)* | 4 (channel stats totals) |

## Renamed symbols in `wanConfWind3`

| Address | Original | Renamed | Role |
|---------|----------|---------|------|
| — | `FUN_*` | `send_pppoe_scan_packet` | Send raw PPPoE PADI with ISP VLAN tag |
| — | `FUN_*` | `wanConfWind3_CreateWanSocket` | Open AF_PACKET socket for WAN detection |
| — | `FUN_*` | `createWanConnection` | Create per-VLAN WAN connection (keys on vid) |
| — | `FUN_*` | `linkWanDetect` | Detect link type (ADSL/VDSL) |
