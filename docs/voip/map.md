# Master VoIP binary and Ghidra map

[VoIP index](index.md) · [Architecture](architecture.md) · [Proxy](master-proxy.md) · [CMSIP](cmsip.md)

## Programs and provenance

File paths in the table are relative to the checked rootfs,
`vx830v/squashfs-root/`. The Ghidra project is `RemoteBoard Modulus`.

| Master file | SHA-256 | Ghidra project path |
|---|---|---|
| `usr/bin/voip_client` | `716f620f8b761c20378d9cb8d0d22b8b6ef253af63f839b72b8e4a5e4f5bf3c8` | `/voip_client.host` (pre-existing) |
| `usr/bin/voip_server_proxy` | `2e0c84398ba9c480784c2cedfd19933c444614c55905b4f4667b975252ac7d99` | `/voip_server_proxy.host` (pre-existing) |
| `usr/bin/pjsua` | `2299db1dbcc0bf5428ecc0676a24c7265bc82fe593dd5ef05c51a2dda812f947` | `/pjsua` (new import) |
| `bin/cos` | `e98c9f0f758e5b079ccd598d97bd7168ef3e1b91a0ba0b3b3916d62fe4609c8c` | `/cos` (pre-existing) |
| `usr/bin/remote_board` | `3cd2153c4e62bc04bd07aa400c99ba073ecc76b4e861780de6dc0a08ca0ac8b2` | `/remote_board` (pre-existing) |
| `lib/modules/voip.ko` | `1d8c1f4b9cbd08e238048958b9c502cadc1d9273544291b34a874a05666a5597` | `/voip.ko` (new import) |

The first five are stripped AArch64 ELF executables with musl interpreter;
`voip.ko` is an AArch64 relocatable kernel module. For all six entries,
`sha256sum` of the complete rootfs file matches Ghidra's **Program Information →
Executable SHA256** recorded at import. The addresses below therefore refer to
these specific ELF inputs, even where the original import path differs.

Auto-analysis completed for the new imports. `pjsua` is stripped and the first
pass found 335 functions; do not treat that as a complete function inventory.
`voip.ko` is relocatable; Ghidra's analysis initially identified only module
entry/exit, so its device handlers need additional work. Existing imports and
analysis were retained.

## First-pass functions

Addresses are Ghidra virtual addresses for the imported images, not offsets in
the files. Names below were applied during this pass; names labelled inferred
are descriptive analyst names, not recovered source symbols.

### `/voip_client.host`

| Address | Ghidra name | Basis |
|---|---|---|
| `0x00413954` | `voip_client_main` | Entrypoint passed to `__libc_start_main`; inferred. |
| `0x00410e94` | `voip_select_socks_thread` | Assertion string and socket/select behavior. |
| `0x0040fd08` | `voip_process_sip_msg` | Assertion string and CMSIP dispatch. |
| `0x00415e2c` | `voip_process_dsp_msg` | Assertion string and DSP dispatch. |
| `0x004141f8` | `voip_start_pjsip` | Assertion string and command construction. |
| `0x00403924` | `FUN_00403924` | Creates Unix stream listener; not renamed. |
| `0x00403a60` | `FUN_00403a60` | Connects Unix stream client; not renamed. |
| `0x00403b20` | `FUN_00403b20` | Reads an exact number of bytes, or reports error/EOF; not renamed. |

### `/voip_server_proxy.host`

| Address | Ghidra name | Basis |
|---|---|---|
| `0x00403718` | `voip_proxy_main` | Entrypoint passed to `__libc_start_main`; inferred. |
| `0x00402f58` | `vsp_remote_board_init_thread` | Thread name and request logic; inferred. |
| `0x004029b8` | `vsp_attach_packet_filter` | `SO_ATTACH_FILTER` implementation; inferred. |
| `0x00405da0` | `vsp_rtp_relay_init` | Relay initialization and thread names; inferred. |
| `0x00403bb0` | `vsp_rtp_upstream_relay` | Created with `rtp-upstream-relay` thread name; inferred. |
| `0x004040e4` | `vsp_rtp_downstream_relay` | Created with `rtp-downstream-relay` thread name; inferred. |
| `0x00403904` | `rtp_msg_recv` | Assertion string identifies function. |
| `0x00403a78` | `FUN_00403a78` | RTP frame sender; assertion identifies `rtp_msg_send`; not renamed. |
| `0x004031d8` | `FUN_004031d8` | Sends 132-byte DSP request and waits for response; not renamed. |
| `0x00402cb8` | `FUN_00402cb8` | Initializes `0xDD02` control receive socket; not renamed. |

### `/cos`

`FUN_00406398` is the decompiled initialization routine with embedded
`cos_init` source-log strings. It starts the master proxy and client. This
function was not renamed. Ghidra comments were added to selected client and
proxy functions and the affected master programs were saved.
