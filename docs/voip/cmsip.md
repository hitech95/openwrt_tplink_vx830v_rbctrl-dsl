# PJSUA and CMSIP on the master

[VoIP index](index.md) · [Architecture](architecture.md) · [Replacement plan](replacement.md)

## Vendor-modified PJSUA

The AArch64 `usr/bin/pjsua` includes `/var/tmp/sock_CM_SIP`, `cmsip_msg_process`,
`can not create cmsip socket!!!` and `TP-Link SIP Stack V1.0.0` strings, and
imports the vendor `libos.so` and `libusbvmcommon.so` libraries. This is not
an unmodified PJSUA. Its SIP user agent communicates with the provider using
SIP; the Unix socket is a separate **local binary control interface** to the
call controller, not a SIP network proxy.

The GPL tree supplies a PJSIP source tree at
`vx830v/VX830v_GPL/bba_3_0_platform/platform/apps/public/pjsip_1.10/`
(abbreviated **PJSIP tree** below). Its `pjlib/include/pj/config.h` defines
version 1.10.0. The socket path, error string and TP-Link user-agent string
also appear in the executable. This is strong correspondence, but an exact
binary/source build match has not been demonstrated.

## Connection and records

In the disclosed tree:

- `pjsip/src/pjsua-lib/pjsua_core.c:736–748`: `pjsua_create()` connects to
  CMSIP before ordinary PJSUA initialization and fails on connection error.
- `cmsip/src/cmsip_transport.c:158–206`: `cmsip_sockCreate()` uses
  `PF_UNIX`, `SOCK_STREAM`, then `connect()` to the CM server.
- `cmsip/include/cmsip_msgdef.h:93–95`: the server path is
  `/var/tmp/sock_CM_SIP`.
- `pjsip-apps/src/pjsua/pjsua_app.c:4526–4579,5657–5674`: after startup,
  the application receives and dispatches CMSIP messages.
- `cmsip/src/cmsip_transport.c:226–298`: send/receive loop until a full record
  is transferred. A stream does not preserve write boundaries; an
  implementation must accumulate complete records.

Ghidra independently shows `voip_client` creates/listens on the socket and
reads **0x330 (816) bytes** at a time from the accepted SIP connection. The
disclosed source defines `MAX_MSG_LEN` as `sizeof(CMSIP_SIP_INCALL)` and
`CMSIP_MSG` as a 32-bit type followed by a fixed body. Layout depends on build
macros and C ABI: 816 is the observed firmware build size, not a universal
protocol constant. Check both directions and actual target builds before
claiming compatibility.

The header defines a request flag `0x80000000`, a continuation flag
`0x40000000`, and type mask `0x3fffffff`. The host dispatcher checks the
request bit and mask on native 32-bit words. Because both processes are on the
little-endian master and source serializes C structures directly, the local
ABI should be investigated as host-endian, not assumed to be a network-order
protocol. Confirm with byte-level tests.

Examples of declared message IDs (the request and response namespaces overlap;
interpret the flag and direction):

| Namespace | ID | Meaning in disclosed header |
|---|---:|---|
| request | 1000 / 1001 | register / unregister |
| request | 1002 / 1003 | RTP create / destroy information |
| request | 1006 | make call |
| request | 1009 | incoming call |
| request | 1016 / 1017 | hold / unhold |
| request | 1018 / 1019 | outgoing / incoming DTMF |
| request | 1020 | release call |
| response | 1000 / 1001 | registration / call status |

See `cmsip/include/cmsip_msgdef.h:48–65,166–249,834–873` in the PJSIP tree.
The source also contains callbacks sending call status and RTP-create details.
Field layouts, lifetime rules, continuation handling and asynchronous event
ordering require separate verification.

## Replacement implication

An unmodified upstream PJSUA cannot directly substitute for this binary while
the original `voip_client` expects CMSIP. Either preserve and implement this
interface in a SIP adapter, or replace both SIP and call control together. A
fresh SIP implementation must still provide media negotiation, registration,
call state and event delivery expected by the rest of the system. See
[replacement.md](replacement.md).
