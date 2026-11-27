# Plan: FOSS VoIP for the VX830v master (OpenWrt, baresip)

Status: **rev. 3 — decisions closed, ready for confirmation.**

Replaces the whole AArch64 master VoIP stack (`voip_server_proxy`,
`voip_client`, `pjsua`, `cos` glue, `voip.ko`) with free software on OpenWrt.
The Econet (MIPS) voice firmware stays stock and is the compatibility target.

Older plans in this directory (`rbctl-*`, `protobuf.md`, …) target the xDSL
path only. `protobuf.md` §"EtherType safety" is stale for VoIP (two
EtherTypes listed, `voip_client` said to use `0x88B6`); this plan supersedes it
for voice.

---

## 1. Decisions

| # | Decision |
|---|---|
| D1 | Nothing OEM remains on the master; full FOSS stack. |
| D2 | OpenWrt target; configuration via UCI, control/status via ubus, service via procd. |
| D3 | baresip/libre **4.9.0**, matching the VX830v SDK. Full uses the telephony feed; lite uses the same upstream release with a private crypto build. |
| D4 | Econet voice firmware stays stock; its host-facing contract is fixed. |
| D5 | Fixed G.711 board leg, baresip transcodes for the network leg (§6). |
| D6 | VoIP package licence **GPL-2.0-or-later** (§11). |
| D7 | New codebase; `rbctl-agent` (Econet/xDSL) is not reused. |
| D8 | No master-side OEM capture; validation per §9. |
| D9 | v1 feature set in §7. |

## 2. Goals and non-goals

| ID | Goal | Measured by |
|---|---|---|
| G1 | Interoperate with the stock Econet voice peer over VLAN 500 (`DD01/DD02/DD03`). | M1, M2 tests |
| G2 | Two FXS lines usable as SIP phones: register, call in/out, DTMF, caller ID, tones. | M3, M4 tests |
| G3 | Standard OpenWrt integration: UCI, ubus, procd, native OpenWrt package. | M5 tests |
| G4 | Robust unattended operation: recovers from peer reboot, link loss, provider outage. | M6 tests |
| G5 | Every protocol decision traceable to a document in `docs/voip/` or to a recorded test. | Doc map (§3), review at each milestone |

Non-goals for v1: media offload (board RTP directly to provider), T.38,
conference, transfer/forward/DND, LuCI app, IPv6 media, replacing Econet firmware.

## 3. Documentation map

### 3.1 Existing documents and where they are used

| Document | What it provides | Used by |
|---|---|---|
| [docs/voip/index.md](../docs/voip/index.md) | Entry point, audit scope | all |
| [docs/voip/architecture.md](../docs/voip/architecture.md) | OEM process model, startup chain (`rcS` → `cos` → proxy/client), `/dev/voip` ioctls, VLAN `lan0.500` | §5 architecture, M5 init script, risk R5 |
| [docs/voip/board-link.md](../docs/voip/board-link.md) | **Wire contract**: envelope, CRC-16/ARC + vectors, transactions `1a/1b/1c/16/18/19`, retries, MAC learning, DD02 dedupe/ACK, boot dependencies, DSP command inventory, command 10/11 field table, DD03 media body, IPv6 family byte-order bug | M1 library spec, M2 bring-up, M3 media |
| [docs/voip/master-proxy.md](../docs/voip/master-proxy.md) | Raw socket setup, BPF filter, frame offsets, relay behaviour | M1 transport, M3 DD03 handling |
| [docs/voip/controller-startup.md](../docs/voip/controller-startup.md) | Init order (SLIC_LOCALE → DSP_DEFAULT → DSP_START → CHANNEL_DEFAULT), slot affinity, CHANNEL_OPEN body, dial tone/first digit, restart guard | M2 board lifecycle, M3/M4 line FSM |
| [docs/voip/dtmf.md](../docs/voip/dtmf.md) | Digit events 20/24/71 (ASCII values), local vs network DTMF, SIP INFO format | M3 DTMF, M4 digit collection |
| [docs/voip/worker-transcript.md](../docs/voip/worker-transcript.md) | Reference two-line exchange (init, attach, ring, answer, media, update, release), IPv4 RTCP loop quirk, update-destination limitation | M1 differential tests, M2/M3 scenario order |
| [docs/voip/cmsip.md](../docs/voip/cmsip.md) | OEM SIP↔controller IPC | Background only (CMSIP not reimplemented, D1) |
| [docs/voip/replacement.md](../docs/voip/replacement.md) | Earlier boundary analysis and validation milestones | Superseded by this plan; keep scenario list for M4 |
| [docs/voip/map.md](../docs/voip/map.md) | Binary identities and Ghidra addresses | Traceability of every finding |
| [docs/checksum.md](../docs/checksum.md), [examples/checksum.py](../examples/checksum.py) | CRC-16/ARC reference | M1 |
| [vx830v/host_board/docs/backdoor.md](../vx830v/host_board/docs/backdoor.md) | `tcconsole` L2 console on Econet | M2+ peer-side diagnostics |

### 3.2 Existing test assets

| Asset | Purpose | Milestone |
|---|---|---|
| `examples/voip_qemu/run.py` (+ `probe.c`) | OEM routine vectors: client records, proxy framing, DD02 dedupe, DD03 frame | M0 baseline, M1 oracle |
| `examples/voip_qemu/workers.py` (+ `worker_io.c`) | Full OEM proxy worker transcript, two lines | M0 baseline, M1 oracle |
| `examples/voip_qemu/controller.py` (+ `controller.c`) | OEM controller startup, off-hook, dial tone, first digit, CMSIP/media serializers | M0 baseline, M2/M3 expected sequences |
| `examples/voip_qemu/dtmf.py` (+ `dtmf.c`) | DTMF event encoding, 72 assertions | M0 baseline, M3 |
| `examples/voip_trace.py` | Offline pcap decoder for DD01/02/03 (metadata, CRC check) | M2–M6 capture checks |

### 3.3 Documents to create

| Document | Content | Created in |
|---|---|---|
| `docs/voip/peer.md` | Econet peer contract: reply statuses, `1a/1b/1c`, event generation, tone/DTMF/CID tables, `RTP_ACTIVE` field use, codec/ptime limits, 16 kHz path, behaviour on master restart; with Econet addresses | M0 |
| `docs/voip/foss-design.md` | As-built design of library, modules, UCI and ubus API | M1, updated each milestone |
| `docs/voip/test-report.md` | Results per milestone test ID, captures index (private captures referenced, not committed) | M0 onwards |
| Update `plans/protobuf.md` | Correct VoIP EtherType section | M0 |

## 4. Evidence on the peer (basis for M0)

| Evidence | Location | Consequence |
|---|---|---|
| `voip_client_proxy` (MIPS32 BE): `eth0.1.500`, `/var/tmp/sock_CM_DSP`, `rtp-upstream-relay`/`rtp-downstream-relay`, `DD01/02/03` | `vx830v/xDSL/econet_rootfs/userfs/bin/` | Board-side counterpart of the master proxy; analyse statically. |
| `voip_server` + `libadam.so`: `mtkDSPStart`, `mtkChannelOpen`, `mtkRTPActive/Update/Deactive`, `mtkRingStart`, `mtkTonePlay`, `adamChanPlayType2Cid`, `voip_convertTone`, `voip_convertDtmf`, `MTKPTimeConvert` | same, `lib/libadam.so` | Command → DSP API, tone/DTMF/CID tables, ptime limits. |
| `ovdsp.ko`: G.711/G.722/G.726/G.729/iLBC, T.38; `fxs3.ko`: LEC, fax-tone detection; `slic3.ko`, `pcm*.ko`, `DSPCore.ko` | `econet_rootfs/lib/modules/` | DSP capabilities; echo cancellation on the peer. |

## 5. Architecture

### 5.1 Runtime

Full and lite are alternative board owners. Full loads the module in the feed's baresip; lite loads it in the private 4.9.0 runtime. The service package installs the profile marker, not the user. Upgrade and test both builds together when changing D3.

```mermaid
flowchart LR
    UCI["/etc/config/voip"] --> INIT["/etc/init.d/voip (procd)"]
    INIT -->|"renders /var/run/voip/{config,accounts}"| BS
    INIT -->|"netifd 8021q device"| VLAN["lan0.500"]
    subgraph BS["baresip process (service 'voip')"]
        FXS["module rbctl_voip<br/>board lifecycle, line FSM,<br/>ausrc/auplay 'rbctl_voip'"]
        UB["module rbctl_voip_ubus"]
        CORE["baresip core<br/>g711, g722, opus, srtp, auresamp"]
        LIB["rbctl_voip_proto (Rust, C ABI)"]
        FXS --> LIB
        FXS <--> CORE
        UB <--> CORE
        UB <--> FXS
    end
    LIB <-->|"AF_PACKET on lan0.500<br/>DD01 / DD02 / DD03"| PEER["Econet voice peer (stock)"]
    CORE <-->|"SIP + RTP/SRTP"| PROV["SIP provider / PBX"]
    UB <-->|"ubus object 'voip'"| MGMT["rpcd, scripts, LuCI (later)"]
```

- One process: board link, line state, SIP and media share the libre event loop.
- Service restart repeats the board `1a` init (same as the OEM proxy at start).
- Signal path: DD02 event → `vx830v` line FSM → baresip UA API (`ua_connect`, `ua_answer`, `call_hangup`, `call_send_digit`) and back via UA events → DSP commands over DD01.
- Media path: DD03 RTP (board leg, G.711) ↔ `rbctl_voip` audio device (8 kHz S16) ↔ baresip audio pipeline ↔ network codec RTP.

### 5.2 Components

| Component | Responsibility | Spec source |
|---|---|---|
| `rbctl_voip_proto` + `rbctl_voip_ffi` + `rbctl_voip_net` (Rust, C ABI) | Envelope, CRC, DD01 engine (seq, retries, MAC learning, subtype/seq matching), DD02 receiver (dedupe, cached ACK, status 1 for short), DD03 framing, DSP record codecs, `AF_PACKET` transport, fake transport for tests | board-link.md, master-proxy.md, peer.md |
| `rbctl_voip` baresip module | Board lifecycle with timeouts; slot allocator (4 slots, affinity `slot & 1`); per-line FSM (idle, dialtone, digits, outgoing, ringing, connected, flash, released); tones, CID, ring; audio device with jitter buffer | controller-startup.md, dtmf.md, peer.md |
| `rbctl_voip_ubus` baresip module | ubus object `voip` (`status`, `lines`, `calls`, `dial`, `hangup`, `reload`), events `voip.board`, `voip.line`, `voip.call`, `voip.reg`; libubus fd in the libre loop | §8.3 |
| `rbctl-voicectl` (Rust) | Bring-up/diagnostic CLI: `init`, `events`, `monitor`, `keypad`, `ring` (with CID), `tone`, `loop` | M2.1 |
| Package `rbctl-voip` | Init script, UCI defaults, rpcd ACL, firewall include | §8 |

### 5.3 Source layout

Target layout from M2.1 (Cargo workspace inside the rbctl feed, next to `rbctl-dsl`):

```
rbctl-feed/rbctl-voip/
  Makefile                      OpenWrt package (rust-package.mk; DEPENDS: baresip, libubus, libubox)
  Cargo.toml, Cargo.lock        workspace (pattern from rbctl-dsl)
  crates/rbctl_voip_proto/      protocol library (sans-I/O)
  crates/rbctl_voip_net/        AF_PACKET sockets over af_packet
  crates/rbctl_voip_ffi/        C ABI staticlib + include/rbctl_voip.h (cbindgen)
  crates/rbctl_voicectl/        bring-up/diagnostic CLI (binary rbctl-voicectl)
  modules/rbctl_voip/           baresip module (C shim over rbctl_voip_ffi), M3
  modules/rbctl_voip_ubus/      baresip ubus control module, M5
  tests/vectors/                golden vectors (from the C prototype and OEM oracle)
  tests/qemu/                   differential tests against examples/voip_qemu oracles
  tests/ffi/                    C smoke test against the generated header
  tests/live/                   scenario scripts for hardware
  files/                        voip.init, voip.config, acl.d/voip.json, firewall include
```

The four crates, recipe and vector/QEMU/FFI tests are present at M2.1 exit. The C/CMake feed was retired after T2.17. `tests/vectors/c_prototype.txt` is the frozen reference; normal builds do not run its historical C generator. Modules and service packaging are later milestones.

## 6. Codec evaluation

Two legs: `phone ─ SLIC/DSP (Econet) ═ DD03 board leg ═ baresip ─ network leg ─ provider`.

- **Network leg: multiple codecs needed** (PCMA, PCMU, G.722; Opus optional). Provided by stock baresip modules, selected per account. Transcoding 2 calls on aarch64 is negligible.
- **Board leg: G.711 sufficient for v1** (PCMA default, PCMU option). Native DSP format; analog handsets are narrowband.

| Board codec | Verdict |
|---|---|
| G.711 | v1 default. |
| G.722 | [T4.8 evaluated](../docs/voip/test-report.md#t48-live-board-leg-g722-bandwidth-evaluation): PT 9 and two-way narrowband audio work, but no 5/6 kHz benefit demonstrated on the two tested handsets. Retain PCMA by default. |
| G.726/G.729/iLBC | No benefit on a local link. |
| T.38 | Not in baresip; fax as G.711 passthrough in v1. |

A multi-codec board leg is only required for media offload (deferred).
Echo cancellation stays on the DSP; baresip AEC not loaded; `auresamp` handles 16/48 kHz network codecs.

## 7. v1 feature set

v1 is a **complete analog-to-SIP ATA** on both lines: everything a handset user
can hear, dial or signal on the analog side has a SIP equivalent. Most of it lives in the
`rbctl_voip` baresip module. Features that work through number handling, user policy or
history, rather than the analog interface, are deferred.

| v1 (M3–M4) | Later |
|---|---|
| 2 FXS lines, line↔account mapping, ≤4 accounts; incoming call rings all lines mapped to the account (first off-hook answers), or hunts to a free line | more accounts |
| Registration UDP/TCP/TLS, outbound proxy, digest auth; optional "no dial tone while unregistered" | WebSocket |
| In/out calls, hangup both ends, early media (183), local ringback (180), remote hold/resume and re-INVITE codec change | |
| Full call-progress tone plan: dial, stutter dial (MWI), ringback, busy, congestion/reorder, number unobtainable/SIT, call waiting, disconnect, off-hook warning (howler) | user-defined tone cadences |
| Off-hook timers: first-digit, inter-digit (short/long), dial-tone timeout and post-disconnect timeout → off-hook warning | |
| Digit map per line (`x`, ranges, `.`, `T`), `#` to send; immediate dial on unique match | number mapping, speed dial, feature codes (`*xx`) |
| DTMF: RFC 4733, SIP INFO, in-band | |
| Caller ID: name, number, date/time, withheld/unavailable; source PAI → RPID → From; call-waiting CID (type II via TONE_PLAY 31) | |
| Call waiting + hook flash (answer, swap); flash-hold with recall dial tone on a single call | transfer, 3-way conference (DSP commands 15/16 exist) |
| Distinctive ring from `Alert-Info` → board cadence entry | |
| MWI: SIP `message-summary` subscription → stutter dial tone | visual MWI: no peer path |
| Per-line receive/transmit gain (board digital gain); port LED if T4.23 confirms | |
| Fax/modem: G.711 passthrough on detection | T.38 |
| SRTP (SDES) | DTLS-SRTP, ICE |
| ubus, UCI, procd | LuCI, call log, DND, call forward, blacklist, hotline/warmline |
| IPv4 | IPv6 |
| | Rotary/pulse dialling: no peer path (T4.21) |

## 8. Configuration and API

### 8.1 UCI `/etc/config/voip`

```
config board 'board'
	option enabled '1'
	option device 'lan0'
	option vid '500'
	option locale 'IT'            # FXS country code or ETSI; renderer maps to host index
	option board_codec 'pcma'     # pcma | pcmu (g722 after M4)
	option ptime '20'
	option init_timeout '120'
	option log_level 'info'

config account 'acc1'
	option enabled '1'
	option user '0123456789'
	option auth_user ''
	option password 'secret'
	option domain 'sip.example.net'
	option registrar ''
	option outbound ''
	option transport 'udp'        # udp | tcp | tls
	option expires '3600'
	list codec 'PCMA'
	list codec 'PCMU'
	list codec 'G722'
	option dtmf 'rfc4733'         # rfc4733 | info | inband
	option srtp 'off'             # off | optional | mandatory
	option display_name ''
	option digitmap '[0-9*]+'     # default; # sends the collected digits
	option interdigit_timeout '4'
	option interdigit_long_timeout '10'
	option require_registration '0'
	option incoming 'ring_all'    # hunt selects the first free physical port
	list alert_info 'internal=2'  # optional; at most one entry per account
	option check_origin '1'       # match the registrar's observed IP

config line 'line0'
	option enabled '1'
	option port '0'
	option account 'acc1'
	option cid '1'
	option call_waiting '1'
	option flash_min_ms '100'
	option flash_max_ms '800'
	# M4 additions
	option first_digit_timeout '15'
	option offhook_warning_timeout '60'  # after disconnect / idle off-hook
	option rx_gain '0'            # raw CHANNEL_OPEN +04, toward handset
	option tx_gain '0'            # raw CHANNEL_OPEN +08, toward network
	option mwi '1'
	option cw_cid '0'
	option howler_duration '15'

config line 'line1'
	option enabled '1'
	option port '1'
	option account 'acc1'
	option cid '1'
	option call_waiting '1'
```

### 8.2 Rendering

| UCI | Target |
|---|---|
| `board.device`, `board.vid` | netifd `config device` (type `8021q`, `<device>.<vid>`); module binds there |
| `board.*` | baresip `config`: `rbctl_voip_device`, `rbctl_voip_locale`, `rbctl_voip_codec`, `rbctl_voip_ptime`, `rbctl_voip_init_timeout` |
| `account` | baresip `accounts`: `<sip:user@domain>;auth_user=;auth_pass=;outbound=;audio_codecs=;mediaenc=srtp*;dtmfmode=;regint=` |
| `line.account`, account policy and port settings | Per-port keys `rbctl_voip_accountN`, `rbctl_voip_digitmapN`, `rbctl_voip_short_digitN`, etc. No binding for unassigned, deleted, disabled or incomplete accounts. |

procd: instance `baresip -f /var/run/voip`, respawn, `/var/run/voip` mode 0700; `service_triggers` on `voip` (restart for `board`, ubus `voip reload` for accounts/lines); firewall include for SIP/RTP when enabled.

### 8.3 ubus `voip`

| Method / event | Content |
|---|---|
| `status` | board state (`down/init/ready/error`), peer MAC, per-account registration, per-line state |
| `lines`, `calls` | line FSM state, slot, call id, peer URI, codec, duration |
| `dial {line, uri}`, `hangup {line}` | test/automation hooks |
| `reload` | re-read accounts/lines without board re-init |
| `voip.board`, `voip.line`, `voip.call`, `voip.reg` | state change events |

## 9. Milestones and completion tests

Each milestone is complete only when **all** its tests pass and results are
recorded in `docs/voip/test-report.md` (test ID, date, commit, pass/fail,
capture reference). Live tests use a LAN test PBX (e.g. Asterisk) and two
analog phones.

### M0 — Evidence baseline

Goal: close peer unknowns; confirm OEM oracles still run.
Docs: board-link.md, master-proxy.md, worker-transcript.md, controller-startup.md, dtmf.md → produce peer.md.

| Test | Command / method | Pass criteria |
|---|---|---|
| T0.1 | `python3 -B examples/voip_qemu/run.py` | exit 0, all byte comparisons pass |
| T0.2 | `python3 -B examples/voip_qemu/workers.py` | exit 0; 17 DD01 transactions, 1 retransmit, 8 DD02 events as documented |
| T0.3 | `python3 -B examples/voip_qemu/controller.py` | exit 0 |
| T0.4 | `python3 -B examples/voip_qemu/dtmf.py` | exit 0, 72 event assertions + 24 INFO writes |
| T0.5 | Review of `docs/voip/peer.md` | every item in §3.3 answered with Econet address, or marked unknown with a live test assigned in M2 |
| T0.6 | `plans/protobuf.md` VoIP section corrected | reviewed |

### M1 — Protocol library

Goal: `libvx830v-voice` byte-exact with the OEM host.
Docs: board-link.md (envelope, transactions, inventory, field tables), master-proxy.md, peer.md.

| Test | Command / method | Pass criteria |
|---|---|---|
| T1.1 | `make -C tests/unit check` | CRC vectors `277e`, `591f`, `bb3d`; header encode/decode round-trip |
| T1.2 | unit: DD01 engine with fake transport | seq increments per new request; retransmit identical frame; 1000 ms×120 / 300 ms×2 limits; reply matched on subtype+seq; failures returned |
| T1.3 | unit: DD02 receiver | duplicate seq → identical cached ACK, single delivery; short body → status 1; queue full → non-zero status |
| T1.4 | unit: DSP encoders | for each command in board-link.md inventory: length and BE layout; family field `htonl` |
| T1.5 | `tests/qemu/diff.py` | our encoders vs OEM serializers from `examples/voip_qemu` for identical inputs: byte-identical records and frames (ring/stop-ring, channel default, CHANNEL_OPEN, RTP_ACTIVE/DEACTIVE, DD03 frame) |
| T1.6 | ASan/UBSan unit run + fuzz decoders 10 min | no findings |

### M1.1 — Library polish (post-M1, before M2)

Goal: make `libvx830v-voice` maintainable and ready to be consumed by the
baresip modules and `voicectl`, **without changing wire behaviour** except for
the listed review fixes. Every T1.x test must still pass afterwards.
Docs: [foss-design.md](../docs/voip/foss-design.md), M1 review notes (§13.5).

Deliverables:

1. **API documentation.**
   - Split `voice.h` into topic headers with an umbrella include: `vx/wire.h` (frame, CRC, media prefix), `vx/dsp.h` (DSP records), `vx/link.h` (transaction engine), `vx/packet.h` (AF_PACKET transport); `vx/voice.h` includes all four.
   - Plain inline comments written for a developer reading the code (no doc generator required). On every public function, struct, struct member, enum and constant: what it is for, each argument (direction in/out, units, byte order, valid range), return values and `-errno` codes, buffer ownership/lifetime (borrowed views), threading and re-entrancy rules, and which document section defines the field (board-link.md / peer.md anchor).
   - Implementation files (`.c`) get short comments at each non-obvious step: why a check exists, which OEM/peer behaviour it mirrors or deliberately differs from, and protocol constants explained where they are used (retry budgets, 60-byte padding, marker values, 15-entry FIFO, CRC coverage).
   - Comments explain intent and protocol meaning, not restate the code line by line.
   - `struct vx_media_cfg`, `struct vx_dsp` union arms and `struct vx_link` state fields: one comment per member naming its wire offset and meaning; private state of `vx_link` marked as such.
   - Short usage example in `vx/link.h` (event-loop integration) matching foss-design.md.
2. **Formatter and linters (Linux kernel C style).**
   - `.clang-format`: Linux style (tabs, 8-column indent, 80-column limit, `BreakBeforeBraces: Linux`, pointer `*` attached to the name), written for this project.
   - `checkpatch.pl --no-tree --strict --file` from a Linux source tree as an external tool (not vendored); SPDX line form per kernel rules (`//` in `.c`, `/* */` in `.h`).
   - `clang-tidy` with a checked-in `.clang-tidy` (`bugprone-*`, `cert-*`, `clang-analyzer-*`, `misc-*`, selected `readability-*`; documented exclusions), plus `cppcheck --enable=warning,portability,performance`.
   - Reformat the existing sources once in a separate commit (format-only), so later diffs stay reviewable.
3. **CMake build** replacing the hand-written Makefiles in `src/lib` and `tests/unit`.
   - Top-level `CMakeLists.txt` (CMake ≥ 3.16): library target `vx830v-voice` (static and shared, `SOVERSION`), installed public headers under `include/vx/`, generated `vx830v-voice.pc` for pkg-config.
   - Options: `VX_BUILD_TESTS`, `VX_SANITIZE` (ASan/UBSan), `VX_FUZZ` (Clang libFuzzer), `VX_BUILD_TOOLS`, `VX_BUILD_MODULES` (off until M3; will find baresip/libre via pkg-config).
   - CTest: unit tests, sanitizer variant, `qemu_diff` (skipped with a clear message if `qemu-aarch64`/OEM rootfs are missing).
   - Custom targets: `format` (apply), `format-check`, `checkpatch`, `tidy`, `cppcheck`, `lint` (all checks).
   - OpenWrt package `Makefile` switched to `include $(INCLUDE_DIR)/cmake.mk`, as the telephony feed already does for baresip.
4. **Review fixes from the M1 review** (behavioural; each with a unit test):
   - `1c` no longer blocks the link: rejected by `vx_request`, or allowed only with an explicit caller-supplied retry budget.
   - Initial DD01 sequence is caller-seedable (random seed recommended) instead of always starting at 1.
   - Decide and document whether failure ACKs (status 1/2) are cached; if not cached, a peer retry after the FIFO drains is accepted.
   - Ignore our own transmitted frames at the socket (`PACKET_OUTGOING`) in `vx_packet_read`.
   - `docs/voip/index.md` links `peer.md`, `foss-design.md`, `test-report.md`; scratchpad §13.2 tidied (diagnostic rows grouped under the final T0.3 result).

| Test | Command / method | Pass criteria |
|---|---|---|
| T1.7 | `cmake -S . -B build -DVX_BUILD_TESTS=ON && cmake --build build && ctest --test-dir build --output-on-failure` | builds with `-Wall -Wextra -Werror`; all tests pass (T1.1–T1.5 equivalents) |
| T1.8 | `cmake -B build-san -DVX_SANITIZE=ON -DVX_BUILD_TESTS=ON` + `ctest` | pass, no sanitizer report |
| T1.9 | `cmake --build build --target lint` | `format-check` clean; checkpatch 0 errors / 0 warnings (any accepted exception listed in `docs/voip/foss-design.md`); clang-tidy and cppcheck 0 findings |
| T1.10 | Documentation review of `include/vx/*.h` and `src/lib/*.c` against a checklist in `docs/voip/test-report.md` | every public symbol, argument and struct member has an inline comment covering purpose, units/byte order, ownership and errors; each non-obvious step in the `.c` files explains why; a reviewer unfamiliar with the code can map each wire field to its doc section from the comments alone |
| T1.11 | Review fixes unit tests | `1c` behaviour, seeded sequence, ACK-cache policy, outgoing-frame filter covered and passing |
| T1.12 | Regression: `python3 -B tests/qemu/diff.py` and 10 min fuzz via `-DVX_FUZZ=ON` | unchanged T1.5 result (19 comparisons, 2528 bytes); no fuzz findings |
| T1.13 | Packaging smoke: OpenWrt SDK `make package/vx830v-voip/compile` (aarch64), if the SDK is available; otherwise record as deferred to T5.1 | library and headers staged, `.pc` file installed |

Exit: T1.7–T1.12 pass (T1.13 pass or explicitly deferred), Makefiles removed,
foss-design.md updated for the CMake commands and header layout.

### M2 — Board bring-up on hardware

Goal: our code drives the real peer without baresip.
Docs: board-link.md §boot dependencies, controller-startup.md §DSP initialization, peer.md.

| Test | Command / method | Pass criteria |
|---|---|---|
| T2.1 | `vx830v-voicectl init` | `1a` reply status 0, then SLIC event 1 and DSP event 2 received; CHANNEL_DEFAULT ×4 sent; ≤ 5 s |
| T2.2 | `vx830v-voicectl events` + lift/replace handset on each line | HOOK_OFF/HOOK_ON with correct line 0/1 |
| T2.3 | press `0-9*#` on each line | DTMF_START/END with ASCII digits per dtmf.md |
| T2.4 | `vx830v-voicectl ring --line N --cid name,number` | phone rings, CID displayed; `RING_STOP` stops it |
| T2.5 | `vx830v-voicectl tone --line N --sel 4` / `--sel 0` | dial tone on / off |
| T2.6 | `vx830v-voicectl loop --line 0 --to 1` | two-way audio between the two phones over DD03 |
| T2.7 | `tcpdump -i lan0.500 -w m2.pcap 'ether proto 0xdd01 or 0xdd02 or 0xdd03'` then `python3 examples/voip_trace.py -v m2.pcap` | all frames classified, 0 CRC mismatches |
| T2.8 | restart `voicectl` 10× | peer re-initialises each time (answers T0.5 restart item) |

### M2.1 — Rust port of library and CLI (post-M2, before M3)

Goal: replace the C `libvx830v-voice` and `rbctl-voicectl` with Rust
equivalents that are easier to read and extend, **without changing wire
behaviour**. The library keeps a C ABI so the M3 baresip module can embed it.
Docs: [foss-design.md](../docs/voip/foss-design.md), [board-link.md](../docs/voip/board-link.md),
[peer.md](../docs/voip/peer.md), M2 closure in [test-report.md](../docs/voip/test-report.md#m2-closure).

**Naming (rbctl family):** feed package `rbctl-feed/rbctl-voip` (OpenWrt package `rbctl-voip`), crates `rbctl_voip_proto`, `rbctl_voip_ffi`, `rbctl_voip_net`, `rbctl_voicectl` (binary `rbctl-voicectl`), C header `rbctl_voip.h` with symbol prefix `rbctl_voip_`, baresip modules `rbctl_voip` and `rbctl_voip_ubus`. User-facing names stay generic: UCI config `voip`, ubus object `voip`, procd service `voip`. The M1–M2 C artifacts (`libvx830v-voice`, `vx830v-voicectl`, `voip-feed/vx830v-voip`) keep their historical names until removed at M2.1 exit.

**Reuse from `rbctl-feed/rbctl-dsl`** (same toolchain, conventions and OpenWrt recipe):

| Item | Reuse | Note |
|---|---|---|
| Workspace layout, `[workspace.dependencies]`, release profile (`opt-level="z"`, LTO, `codegen-units=1`, `panic="abort"`, strip) | Copy pattern | Same `rust-version` floor (1.80). |
| `crc` crate (`CRC_16_ARC`), as in `rbctl_proto::checksum` | Direct dependency | MIT/Apache-2.0. Do **not** depend on `rbctl_proto` itself (0x88B5-specific). |
| `af_packet` crate (`RawSocket::open(iface, ethertype)`, `recv` with `pkt_type`) | Path dependency, one socket per EtherType (DD01/DD02/DD03) | **Licence blocker below.** Fix its `pkt_type` doc comment (Linux: HOST=0, BROADCAST=1, MULTICAST=2, OTHERHOST=3, OUTGOING=4) and filter `PACKET_OUTGOING`. |
| `clap` (derive), `log`, `nanologger`, `libc` | CLI and logging | Same versions/features as rbctl-dsl. |
| OpenWrt `rust-package.mk`, `PKG_BUILD_DEPENDS:=rust/host`, `RUST_PKG_LOCKED:=1`, workspace-copying `Build/Prepare` | Packaging | `ubus-zero`/`rust-uci` not needed in M2.1; candidates for M5. |

**Licence constraint (D6, §11):** the VoIP library is loaded into baresip,
which links OpenSSL 3 (Apache-2.0), so every crate it links must be compatible
with GPLv3. The shared `af_packet` crate was relicensed **MIT** in `becbc9e` and can be used by both workspaces. `rbctl_proto` is not reused.

**Workspace** `rbctl-feed/rbctl-voip/` (replaces the CMake C tree once parity is proven):

| Crate | Kind | Responsibility |
|---|---|---|
| `rbctl_voip_proto` | `rlib` | Pure protocol, sans-I/O: envelope/CRC, DSP record encode/decode (typed enums/structs per command), DD01 request engine, DD02 receiver/ACK cache/FIFO, DD03 framing. No `unsafe`, no allocation in hot paths, no panics on any input (`clippy::unwrap_used`, `expect_used`, `panic`, `indexing_slicing` denied). Time passed in by caller. |
| `rbctl_voip_ffi` | `staticlib` (+ `cdylib` for tests) | Stable C ABI for the baresip module: opaque `rbctl_voip_link` handle, send callback, `-errno` returns, borrowed views, no panic across FFI. Header `include/rbctl_voip.h` generated by `cbindgen` and committed; CI fails if regenerated header differs. |
| `rbctl_voip_net` | `rlib` | Thin wrapper over `af_packet`: three sockets on the VLAN device, nonblocking fds for an external event loop, drops `PACKET_OUTGOING`. |
| `rbctl_voicectl` (binary `rbctl-voicectl`) | `bin` | Port of every M2 subcommand (`init`, `events`, `monitor`, `keypad`, `ring`, `tone`, `loop`) and the warm-attach behaviour (skip DSP_START when attach reports running, reclaim slots). Test-only media profile override as a CLI flag, not link-time wrapping. |
| `xtask` (optional) | `bin` | `cargo xtask lint/vectors/oracle` helpers. |

Documentation: rustdoc (`///`) on every public item with the same content
required in M1.1 (purpose, units, byte order, ranges, errors, ownership,
doc-section link); module-level `//!` overview per protocol area;
`#![deny(missing_docs)]` on `rbctl_voip_proto` and `rbctl_voip_ffi`.

Deliverables:

1. Rust crates above, with `cargo fmt`, `cargo clippy -- -D warnings`, `cargo deny check licenses` (allow list: MIT, Apache-2.0, BSD-*, ISC, Unicode, GPL-2.0-or-later; deny GPL-2.0-only in the library graph).
2. **Golden vectors**: before removing C, generate `tests/vectors/*.bin|json` from the C library and OEM oracle for every DSP command, frame type, ACK/dedupe sequence and media frame; Rust must reproduce them byte-for-byte.
3. Ported unit tests (`cargo test`), property tests with `proptest` (dev-dependency) for encode/decode round-trips and decoder robustness, `cargo fuzz` targets for frame and record decoders.
4. QEMU differential (`tests/qemu/diff.py`) driven by a Rust vector binary.
5. FFI test: a small C program linked against `librbctl_voip_ffi.a` using only the generated header (proves the M3 embedding path), built under ASan/UBSan.
6. OpenWrt package recipe using `rust-package.mk`; packages `rbctl-voicectl` (bin) and staged `librbctl_voip_ffi.a` + header for the M3 module build.
7. Remove the C library/CLI and CMake files after T2.1x hardware parity; update `foss-design.md` (as-built Rust API) and §5.3 layout.
8. M2 carry-over: T2.5 dial tone on PHONE2 (line 1).

| Test | Command / method | Pass criteria |
|---|---|---|
| T2.10 | `cargo build --workspace --locked && cargo test --workspace` | all pass on host |
| T2.11 | `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check` | no findings; library dependency graph contains no GPL-2.0-only or Apache-only-incompatible item |
| T2.12 | Golden vectors | Rust output byte-identical to C/OEM vectors for all records, frames, ACK/dedupe and media cases (incl. CRC `277e`, `591f`, `bb3d`) |
| T2.13 | `python3 -B tests/qemu/diff.py` with the Rust vector binary | 19 comparisons / 2528 bytes, as T1.5 |
| T2.14 | `cargo +nightly fuzz run` frame and record targets, 10 min each; proptest suites | no crash, panic or sanitizer report |
| T2.15 | FFI smoke: C test against generated header + staticlib, ASan/UBSan | pass; header up to date (`cbindgen` diff empty) |
| T2.16 | Cross-build `aarch64-unknown-linux-musl` via OpenWrt rust toolchain (`make package/rbctl-voip/compile`, or the configured tree as in T1.13) | `rbctl-voicectl` static/stripped binary and `librbctl_voip_ffi.a` produced; size recorded |
| T2.17 | Hardware parity with Rust `rbctl-voicectl`: T2.1, T2.2, T2.3, T2.5 (**both lines**), T2.6 (direction 3 profile), T2.7 capture + `voip_trace.py`, T2.8 (3 warm restarts incl. one with active slots) | same pass criteria as M2; results recorded in test-report.md |
| T2.18 | Rustdoc review | `cargo doc --no-deps` warning-free with `missing_docs` denied; checklist as T1.10 |

Exit: T2.10–T2.18 pass, C sources removed, §5.2/§5.3 updated, licence
of every linked crate recorded in foss-design.md. M3 starts from the Rust
library via `rbctl_voip_ffi`.

### M3 — Single call through baresip

Goal: one line, one account, full call.
Docs: controller-startup.md (off-hook → first digit), dtmf.md, board-link.md §media.

| Test | Command / method | Pass criteria |
|---|---|---|
| T3.1 | start baresip with `rbctl_voip` module, account on test PBX | registered; `rbctl_voip` reports board ready |
| T3.2 | outgoing: off-hook, dial tone, dial extension, `#` | INVITE to the right URI; ringback; answer → two-way audio |
| T3.3 | incoming from PBX | phone rings with CID; off-hook answers; two-way audio |
| T3.4 | hangup from phone and from PBX (each) | BYE sent/received; slot freed (RTP_DEACTIVE + CHANNEL_CLOSE); line idle |
| T3.5 | DTMF in call, account `rfc4733` and `info` | PBX receives correct digits (Asterisk DTMF log) |
| T3.6 | *Moved.* Latency target → T6.8 (M6); latency growth and echo → T4.20 (M4) | — |
| T3.7 | 30 calls back-to-back | no leaked slots/sockets/memory (ubus `status`, `/proc` RSS stable) |

### M4 — Two lines and complete ATA feature set

Goal: the §7 v1 feature set on both lines, so that an analog handset behaves as
on a commercial ATA. Advanced telephony services (number mapping, feature codes,
DND, forwarding, call log, transfer, conference) stay deferred (§7 "Later").
Docs: replacement.md scenario list, controller-startup.md (slot affinity), dtmf.md, peer.md (tones, ring, CID, locale,
[ATA feature reachability](../docs/voip/peer.md#ata-feature-reachability-m4-evaluation-basis)).

**Constraint (D4):** the Econet DSP/SLIC firmware is fixed. A board-side feature exists only if the stock
`voip_server` accepts it as a DD01 command or reports it on DD02/DD03. Every "evaluation" below means:
static reachability in peer.md first, then a live confirmation. If the peer has no path, the feature is
recorded as unsupported or done host-side, never by changing the board.

Prerequisites (implementation, not tests):

0. **M3 review follow-ups (2026-11-26).** Runtime command rejections are reported per command, with re-attach
   when teardown state is uncertain or an ACK times out. Failed queue entries are consumed. Both paths have
   host regression coverage. `rbctl_voip_playout_clock board` enables packet-driven playout for the T4.20
   comparison; `timer` remains the live-tested default. Verify on the stock peer, including DTMF intervals
   and the ten-minute two-line drift/echo measurement, before selecting board pacing by default.
1. **Per-line module structure.** Replace the M3 single static line with one FSM per line
   (idle, dialtone, collecting, outgoing, early, ringing, connected, held, waiting, flash,
   disconnected, howler, lockout). Each line has its own account mapping, slot and timers;
   serialized DSP commands are shared.
2. **Tone plan.** A table from module tone names to `TONE_PLAY` selectors, built from T4.10.
   Any tone without a board selector is generated by the module on the audio path (the slot is opened for it).
3. **SIP-response → tone map:** 486/600/603 busy; 404/410/484/604 number unobtainable;
   408/480/487-from-remote and no answer as busy; 5xx, timeout, transport failure, unregistered congestion.
   Configurable per account.
4. **Gain.** Board digital gain via CHANNEL_OPEN body `+04` (receive) and `+08` (transmit); the peer applies
   fixed offsets +2/−3 (peer.md). Saturating software gain on the 16-bit audio at the board boundary only
   as a fallback if T4.18 shows the board values have no usable effect.

| Test | Command / method | Pass criteria |
|---|---|---|
| T4.1 | T3.2–T3.5 on line 1; DTMF additionally in `inband` mode on both lines | pass; Asterisk receives in-band digits |
| T4.2 | simultaneous calls on both lines | both two-way, independent hangup, slots per affinity |
| T4.3 | call waiting + flash | second call signalled with CW tone; flash answers/swaps (held party gets re-INVITE `sendonly`); hangup of either works; on-hook with a held call re-rings the line |
| T4.4 | busy, unreachable, congestion | correct tone per case (Asterisk returning each code in prerequisite 3) |
| T4.5 | digit map / inter-digit timeout | dial triggers on unique match, `#`, short timeout after a possible match, long timeout otherwise |
| T4.6 | SRTP `mandatory` against PBX | encrypted media, call succeeds |
| T4.7 | G.722 network codec | wideband negotiated to PBX; board leg stays G.711 |
| T4.8 | G.722 board-leg evaluation (RTP_ACTIVE codec selector 3; selectable per peer.md) | two-way audio with selector 3; energy above 4 kHz measured in both directions with a wideband test signal; supported/not and benefit recorded |
| T4.9 | On each line, end an established call from the PBX while the handset stays off-hook | SIP and RTP release normally. The lifted handset plays a disconnect tone; replacing it stops the tone. Lifting it again gives normal dial tone and allows a new call. The other line is unaffected. |
| T4.10 | Tone characterisation: play every `TONE_PLAY` selector (peer.md table) on both lines at the configured locale | each selector named (or "silent") by operator ear plus frequency/cadence from an acoustic capture; table recorded in peer.md and used by the module |
| T4.11 | Call-progress tones | dial, ringback (180 without SDP), early media (183 with SDP: remote audio, no local ringback), busy, congestion, unobtainable, CW, disconnect, howler each heard in its state and stopped on the next state change |
| T4.12 | Off-hook timers | no digit within first-digit timeout → howler; after disconnect timeout → howler; howler then lockout until on-hook; on-hook from any state returns line to idle with no DSP resource left |
| T4.13 | Hold and re-INVITE | flash on a single call holds it and gives recall dial tone, flash again resumes; remote hold (`sendonly`/`inactive`) keeps the call without tone glitches; remote codec/port change applies via RTP_UPDATE without dropping audio |
| T4.14 | Caller ID | name, number, date/time from PAI/RPID/From precedence, 15-byte truncation, withheld ("P") and unavailable ("O") displayed on a known-capable receiver (closes Q8); call-waiting CID via CID_PLAY + TONE_PLAY 31 (type II, peer.md) shown during a call on a capable handset, or recorded as not displayed |
| T4.15 | Distinctive ring | `Alert-Info` values mapped (UCI) to board cadence entries give audibly distinct cadences; ring stops on CANCEL, "completed elsewhere" and answer |
| T4.16 | Multi-line account | incoming rings every line mapped to the account; first off-hook answers, others stop; second incoming rings the free line; both busy → CW or 486 per config |
| T4.17 | Registration dependency | with `require_registration`, an unregistered line gives congestion (or no dial tone, configured) and recovers when registration returns; without it, local dial tone is unchanged |
| T4.18 | Gain (CHANNEL_OPEN `+04`/`+08`) | Evaluated: independent raw per-port adjustments −6…+6 with zero defaults. Record direction and measured response; handset-independent dB accuracy and maximum unclipped gain deferred by operator decision. |
| T4.19 | Fax/modem passthrough | on board fax/modem event (DSP 21/22) or CED/ANS detection: fixed jitter buffer, no PLC/VAD, G.711 only; a fax to Asterisk `ReceiveFAX` on the LAN completes (if no fax machine is available, verify the mode switch with a recorded tone, and record the limit) |
| T4.20 | Call stability on both lines (retained from T3.6) | 10-minute simultaneous calls: master downlink delay in the last minute within 20 ms of the first minute (no growth); no audible echo in either direction on either line |
| T4.21 | Pulse (rotary) dialling — **evaluated 2026-11-26: unsupported** ([report](../docs/voip/test-report.md#t421--rotary-dialling-early-evaluation)) | with a rotary handset, each digit 1–0 arrives as a DD02 digit event (peer.md: pulse events share the DTMF digit path), mapping of 10 pulses recorded; if nothing arrives, record unsupported (no host-side pulse decoding: the peer reports only debounced hook state) |
| T4.22 | MWI | `message-summary` NOTIFY waiting → stutter dial tone (TONE_PLAY 19, or VMS_STATUS flag) on mapped lines, cleared when no messages. Visual MWI (FSK/lamp) is unsupported by the peer (peer.md) and not tested |
| T4.23 | Front-panel FXS state via master-side OpenWrt LED hotplug, like DSL | Existing green/red FXS indicators follow aggregate board, registration, ringing and active state. Hanging up one of two calls leaves the active indication; the last hangup restores idle. Peer command 23 is diagnostic only. |

Exit: T4.1–T4.23 pass or carry a recorded evaluation decision (T4.8, T4.10 tone labels, T4.14 partial CID per Q8, T4.18 units, T4.19 fallback, T4.20 echo carried to M6, T4.21, T4.23);
feature-status.md describes the user-visible behaviour.

### M5 — OpenWrt integration

Goal: installable package configured by UCI, controlled by ubus.
Docs: architecture.md (startup chain replaced), this plan §8.

| Test | Command / method | Pass criteria |
|---|---|---|
| T5.1 | Build `rbctl-voip` in the matching OpenWrt SDK and install on a clean image with its native package manager. The OpenWrt packager chooses the format. | installs, service enabled, no OEM files needed |
| T5.2 | edit UCI account, `uci commit voip` | `reload` applied without board re-init; new registration |
| T5.3 | edit `board.locale`, commit | service restart, board re-init |
| T5.4 | `ubus call voip status`, `ubus listen voip.*` during a call | JSON matches §8.3; events in order |
| T5.5 | `ubus call voip dial '{"line":0,"uri":"sip:100@pbx"}'` / `hangup` | call placed/ended |
| T5.6 | reboot | service up, registered, lines ready without manual steps |
| T5.7 | rpcd ACL | unauthenticated ubus-over-HTTP cannot call `dial` |

### M5.1 — M5 review follow-ups and UCI schema split (before M5 closes)

Goal: close the findings of the 2026-11-27 M5 review and move service policy from the FXS line
to the SIP account in UCI. M5 is marked done only when M5.1 exits.
Docs: this plan §8, [m5-integration.md](../docs/voip/m5-integration.md), module README.

**Review baseline (2026-11-27, working tree on `c8b0157`).**

- `scripts/check.sh`: pass.
- Module host suite against a baresip/libre **4.9.0** host prefix built from the SDK tarballs: **32/33**.
  `baresip_call_solo_hangup` fails deterministically: no audio either way after flash-hold then flash-resume of a single call.
- The suite no longer compiles against the 3.16 prefix (`BEVENT_*` API).
- The module does not build from a source path containing spaces: CMake `LINK_DEPENDS` on `exports.map`.
- `render.uc` under the target `ucode` (QEMU) aborts on configs the LuCI page can save:
  - a port set to "Unassigned" (`invalid port/account`);
  - a port whose account was deleted (`invalid port/account`);
  - a new account without user/domain (`invalid user`);
  - a user containing a space (`invalid user`).

  On reload the old instance keeps running while LuCI reports success. At boot, voice stays down.

#### UCI schema: account vs line

Rule: options that describe the **SIP service/number** (provider policy) belong to the `account`; options
that describe the **physical port or the handset on it** belong to the `line`.

- **Single direction of linkage.** `line.account` remains the only link. An account's ring group is every enabled line referencing it. Do **not** add `account.lines`.
- **One account per line remains a v1 limit.** A line answering several accounts is a "Later" item.
- **Dial plan per phone.** A per-phone outgoing restriction on a shared account is call barring (deferred). It is not a reason to keep `digitmap` on the line.
- **Module unchanged.** The renderer keeps emitting per-port `rbctl_voip_*<port>` keys. Account values are copied to every member line.

| Option | Owner | Note |
|---|---|---|
| `incoming` (`ring_all`/`hunt`) | account | Hunt rings the first free assigned port in physical order (PHONE1, then PHONE2). There is no `hunt_order` option. |
| `digitmap` | account | |
| `interdigit_timeout`, `interdigit_long_timeout` | account | part of the dial plan |
| `require_registration` | account | depends on that account's registration |
| `alert_info` (`value=entry`) | account | Optional single mapping from a provider token to a board cadence (0–9); multiple entries are rejected. |
| `dtmf`, `codec`, `srtp`, registration options, MWI subscription | account | unchanged |
| `account` | line | single link |
| `enabled`, `port` | line | |
| `cid`, `cw_cid`, `call_waiting`, `mwi` (stutter indication) | line | handset capability / local feature |
| `tx_gain`, `rx_gain` | line | |
| `first_digit_timeout`, `offhook_warning_timeout`, `howler_duration` | line | local analog behaviour |
| `flash_min_ms`, `flash_max_ms` | line | fixed by the stock peer; validated only |

Target §8.1 excerpt:

```
config account 'acc1'
	# existing SIP options ...
	option incoming 'ring_all'          # ring_all | hunt
	option digitmap '[0-9*]+'
	option interdigit_timeout '4'
	option interdigit_long_timeout '10'
	option require_registration '0'
	list   alert_info 'internal=2'      # one mapping only

config line 'line0'
	option enabled '1'
	option port '0'
	option account 'acc1'
	option cid '1'
	option cw_cid '0'
	option call_waiting '1'
	option mwi '1'
	option tx_gain '0'
	option rx_gain '0'
	option first_digit_timeout '15'
	option offhook_warning_timeout '60'
	option howler_duration '15'
```

**Migration.** Ship `/etc/uci-defaults/` migration that moves the account-owned options from lines to
their account. When lines of one account disagree, keep the lowest port's value and log the conflict.
`render.uc` keeps the line fallback for **one release** with a deprecation warning, then removes it.

#### Work items

**A. Required for M5 exit**

| ID | Task | Done when |
|---|---|---|
| A1 | `render.uc` never aborts on a config LuCI can save. Fatal only for malformed values. Otherwise skip with a `logger` warning: a line with empty, missing or disabled account; an enabled account lacking user/domain. | T5.8 |
| A2 | LuCI validation mirrors `render.uc`:<br>• datatypes/validators for user, domain, outbound and dial-plan characters;<br>• refuse deleting an account still referenced (or set the port to Unassigned);<br>• account dropdown shows the SIP address, not `cfgXXXX`;<br>• default-on flags use `rmempty=false` so unticking writes `0`. | T5.9 |
| A3 | Fix single-call flash hold/resume on baresip 4.9 (`baresip_call_solo_hangup`). | T5.10; T4.13 hold rechecked live |
| A4 | Host regression on baresip 4.9:<br>• document building the host prefix;<br>• fix `LINK_DEPENDS`/version-script for paths with spaces;<br>• run the module suite from `scripts/check.sh` or CI when the prefix is present. | T5.10 from the real repo path |
| A5 | Record the baresip 4.9.0 move:<br>• D3/R6 deviation in §13.4;<br>• §5/§11 updated: feed baresip vs private lite runtime, mbedTLS (Apache-2.0) under GPL-2.0-or-later → GPLv3 combination. | plan consistent with shipped packages |
| A6 | UCI schema split above: `render.uc`, LuCI (account tab: ring group with fixed port order, dial plan, inter-digit timers, registration requirement, Alert-Info; port tab: per-port options only), `uci-defaults` migration, §8.1 and §8.2 updated. | T5.11 |
| A7 | Open M5 gates: T5.7, T5.1 on a **clean** image, T5.6 reboot with a **registered** account. | recorded in test-report.md |

**B. Should fix in M5.1**

| ID | Task |
|---|---|
| B1 | Ghost-call protection: option (default on) to accept SIP requests only from the account's registrar/outbound proxy. At least, LuCI warns when the SIP firewall zone is `wan`. (T5.12) |
| B2 | `uci commit` during a call: defer `reload` until all lines are idle instead of failing and rolling back. Expose `reload_pending` in ubus `status` and log it. (T5.13) |
| B3 | Preserve MWI state across reload, or re-query immediately after it. |
| B4 | Lite package: replace the install-time `sed` of `/etc/init.d/voip` with a profile file/variable read by the script. |
| B5 | Split the `rbctl-dsl/Makefile` change into its own commit. Verify `panic=abort` still applies (compare binary size) and that builds work offline despite `PKG_SKIP_DOWNLOAD` + cargo network access. |
| B6 | List config-file-only options (no LuCI field) in the module README and feature-status.md. |

**C. Deferred to M6**: ubus `dial` on an on-hook line rings the handset first (click-to-dial); finer ubus error codes (busy); T4.20 echo and T6.8 latency (already carried).

| Test | Command / method | Pass criteria |
|---|---|---|
| T5.8 | Host: run `render.uc` (target ucode under QEMU) on: unassigned port, dangling account reference, disabled account, enabled account without user/domain, invalid user | exit 0 with a warning for each skip case; exit ≠0 only for malformed values; no secret printed |
| T5.9 | LuCI on target: create, edit and delete accounts and ports, including the cases of T5.8 | every saved state leads to a running service whose rendered config matches the UI; unticked flags written as `0` |
| T5.10 | Module host suite against baresip/libre 4.9.0, run from the repository path (contains spaces) | 33/33 pass |
| T5.11 | Migration: old-style config (dial plan, `incoming`, timers, `alert_info`, `require_registration` on lines, including two lines with different dial plans) → `uci-defaults` → render | options moved to the account; conflict logged; rendered `rbctl_voip_*` keys equal to the pre-migration render for non-conflicting configs |
| T5.12 | With the account registered, send an INVITE from a host that is not the registrar/proxy | rejected (no ring) with protection on; accepted with it off |
| T5.13 | `uci commit voip` (account change) during a call | call unaffected; change applied automatically after both lines are idle; `status` shows pending meanwhile |

Exit: A1–A7 done; T5.1–T5.13 pass (T5.12–T5.13 only if B1/B2 are implemented in M5.1, otherwise
recorded as moved to M6); M5 then set to `done`.

**Current exit status:** A3 passed on the VX830v: PHONE2's R button held and resumed the same call on baresip 4.9, with two-way speech confirmed. A6 passed with the r7 LuCI editor and r6 service. Only A7's **T5.1 clean-image installation** remains (deferred); T5.6 and T5.7 already have live evidence. [Results by test](../docs/voip/test-report.md#m5-review-verification).

### M6 — Hardening

Goal: unattended operation (G4).
Docs: board-link.md §boot dependencies (OEM failure modes to avoid), worker-transcript.md §recovery.

| Test | Command / method | Pass criteria |
|---|---|---|
| T6.1 | power-cycle Econet board while idle | `voip.board` down → ready; lines usable again ≤ 60 s |
| T6.2 | power-cycle Econet board during a call | call ended cleanly; recovery as T6.1 |
| T6.3 | unplug/replug VLAN link | as T6.1 |
| T6.4 | kill baresip (`kill -9`) | procd respawn; board re-init; registered |
| T6.5 | provider unreachable 10 min | re-registration after recovery; lines give congestion tone meanwhile |
| T6.6 | 24 h soak with scripted calls every 5 min on both lines | 0 failures; RSS growth < 5 % |
| T6.7 | malformed/fuzzed DD02/DD03 frames injected from a test host | no crash; counters increment |
| T6.8 | Audio latency tuning (moved from T3.6): loopback and acoustic measurement on both lines, both directions, wired LAN | master-added mouth-to-ear latency ≤ 150 ms; jitter-buffer defaults chosen and documented |

## 10. Test environments

| Environment | Used in |
|---|---|
| Host (x86_64) unit tests, sanitizers, fuzzers | M1 |
| QEMU user-mode with OEM AArch64 binaries (`examples/voip_qemu`) | M0, M1 |
| Python peer emulator (`tests/peer-emu`) on a veth pair | M1–M5 CI |
| OpenWrt on the VX830v master + stock Econet board + 2 analog phones + LAN PBX | M2–M6 |
| Optional private reference: OEM `voip_server_proxy` run on OpenWrt against the real peer, captured with tcpdump; never distributed | M2 if T0.5 leaves gaps |
| Econet `tcconsole` diagnostics (`vx830v/host_board/docs/backdoor.md`) | M2–M6 debugging |

## 11. Licensing

- `rbctl-voip` (library, modules, CLI, scripts): **GPL-2.0-or-later**; SPDX header `GPL-2.0-or-later` in every source file, `LICENSE` = GPLv2 text with the "or later" notice.
- Full libre links OpenSSL 3.x; lite uses mbedTLS crypto. Both include Apache-2.0 code; the GPL-2.0-or-later grant permits distributing the combined process under GPLv3. baresip/libre are BSD-3.
- The DSL workspace remains GPL-2.0-only except for the shared `af_packet` crate, relicensed MIT in `becbc9e`. The VoIP crates are GPL-2.0-or-later; `cargo deny` checks their dependency licence allowlist.
- No OEM code or binaries are included in the package or tests committed to the repo; OEM binaries are used only locally as test oracles.

## 12. Risks

| ID | Risk | Mitigation | Closed by |
|---|---|---|---|
| R1 | Peer addresses RTP via `RTP_ACTIVE` IPs/ports | M0 RE; choose values keeping media on DD03 | T0.5, T2.6 |
| R2 | Board-leg latency/jitter | DSP LEC; small jitter buffer | T4.20, T6.8 |
| R3 | Peer rejects re-init after master restart | M0 RE; restart tests | T2.8, T6.4 |
| R4 | Locale → electrical profile mapping | `locale` in UCI; per-country ring/CID check | T2.4 |
| R5 | `/dev/voip` ioctls (`0x403476xx`) provided RTP QoS/conntrack | nftables/QoS rules if needed | T3.6, T6.6 |
| R6 | baresip/libre API drift (including the private lite build) | pin 4.9.0 headers, use matching runtime/modules, run `BARESIP_PREFIX=... scripts/check.sh` before upgrades | T5.1/T5.10 |
| R7 | Legal | docs ↔ implementation separation; §11 | review |

---

## 13. Scratchpad (living section — update during development)

Rules: newest entry first; one entry per work session; reference test IDs
(§9), commits and documents. Do not paste credentials, caller IDs or audio.
Move stable conclusions into `docs/voip/*` and leave a pointer here.

### 13.1 Milestone status

| Milestone | Status | Started | Completed | Commit | Notes |
|---|---|---|---|---|---|
| M0 Evidence | done | 2026-11-25 | 2026-11-25 | Uncommitted | 6/6 pass; [test report](../docs/voip/test-report.md) |
| M1 Library | done | 2026-11-25 | 2026-11-25 | Uncommitted | T1.1–T1.6 pass; [as built](../docs/voip/foss-design.md) |
| M1.1 Library polish | done | 2026-11-25 | 2026-11-25 | Uncommitted | T1.7–T1.12 pass; AArch64 staging verified, standalone SDK deferred. |
| M2 Bring-up | done | 2026-11-25 | 2026-11-25 | Uncommitted | [8/8 passed](../docs/voip/test-report.md#m2-closure); T2.4 caller number displayed. Extended CID tracked separately. |
| M2.1 Rust port | done | 2026-11-25 | 2026-11-25 | `feature/rbctl-voip` (workspace); `becbc9e`, `c96803a`, `0b79262` (af_packet) | [Hardware parity and closure](../docs/voip/test-report.md#m21-closure). C prototype retired; T2.16 accepted with SDK build deferred to T5.1. |
| M3 Single call | done | 2026-11-25 | 2026-11-26 | `feature/rbctl-voip` | T3.1–T3.5 and T3.7 passed live. T3.6 moved: latency growth and echo → T4.20, ≤150 ms target → T6.8 (§13.4). |
| M4 Two lines + features | closed with carry-over | 2026-11-26 | 2026-11-26 | `79cdad2`–`f56d948` + bench results | 16 gates passed live. T4.8, T4.10, T4.18, T4.19, T4.21 evaluated; T4.14 partial (Q8 parked). T4.20 partial: board-clock drift passed both lines; conversational echo remains unverified and is carried into M6. |
| M5 Integration | awaiting T5.1 (deferred) | 2026-11-26 | | feature/rbctl-voip | T5.2–T5.7 passed on hardware; clean-image installation has not been tested. |
| M5.1 Review follow-ups + UCI split | awaiting T5.1 (deferred) | 2026-11-27 | | feature/rbctl-voip | T5.8–T5.13 passed; A7 shares the M5 clean-image gate. [Review evidence](../docs/voip/test-report.md#m5-review-verification). |
| M6 Hardening | not started | | | | |

Status values: `not started`, `in progress`, `blocked (<reason>)`, `done`.

### 13.2 Test results

All entries below used an **uncommitted working tree**. The base column identifies HEAD at the time, not a commit containing the implementation or test results.

| Test ID | Date | Base revision / tree | Result | Evidence / note |
|---|---|---|---|---|
| T5.3 / T5.7 | 2026-11-27 | VX830v r6 service | pass | [Fresh process confirmed warm ETSI init; unauthenticated HTTP dial denied](../docs/voip/test-report.md#m5-review-verification). |
| T5.8 | 2026-11-27 | Target ucode / QEMU | pass | Incomplete accounts and unassigned/dangling lines skipped; malformed user rejected without disclosing credentials. |
| T5.9 | 2026-11-27 | LuCI / VX830v | pass | Created, assigned and deleted an account; invalid user rejected, ports unassigned safely, unticked flags stored as `0`. |
| T5.10 | 2026-11-27 | Linux host / baresip 4.9.0 | pass | 33/33 tests passed from the repository path containing spaces, including immediate double-FLASH. |
| T5.11 | 2026-11-27 | Target ucode / QEMU | pass | Non-conflicting migration preserved rendered policy; lowest port won conflicting dial plans; second migration was a no-op. |
| T5.12 | 2026-11-27 | AArch64 QEMU / registered account | pass | INVITE from a different source IP got 403 with no ring; disabling origin filtering produced 180 and a board ring command. |
| T5.13 | 2026-11-27 | VX830v r6 service | pass | Account change stayed pending during the established call; new AOR applied automatically after hangup. [Detailed T5.8–T5.13 results](../docs/voip/test-report.md#m5-review-verification). |
| T5.6 | 2026-11-27 | VX830v r5 service | pass | [Registered account across reboot](../docs/voip/test-report.md#t56-live-registered-account-across-reboot-2026-11-27). |
| T5.1 | 2026-11-27 | SDK r38588 overlay on VX830v | package upgrade pass; clean-image gate open | r5 packages ran post-upgrade hooks and preserved config. Existing firmware already had full stack; this does not demonstrate install onto a clean image. |
| T5.4 / T5.5 | 2026-11-27 | VX830v `r36953+1633-0ab51fb9c1` | loopback pass | [ubus dial/hangup, PCMA call state and outgoing/established/closed events](../docs/voip/test-report.md#t54t55-live-ubus-call-control-and-events-2026-11-27). No real handset/provider in this check. |
| T5.2 / T5.6 | 2026-11-27 | VX830v image `r36953+1633-0ab51fb9c1` | reload pass / boot partial | [Temporary account registered, both ports idle without board re-init; first-attach retry recovered at boot](../docs/voip/test-report.md#m5-live-new-image-boot-and-account-reload-2026-11-27). Boot with a provisioned account not yet tested. |
| T5.1 | 2026-11-27 | `c8b0157` + WT | bench install blocked | [Signed APK verified on VX830v](../docs/voip/test-report.md#m5-preflight-openwrt-integration); matching baresip 3.16 packages unavailable in snapshot indexes. Clean SDK/image gate open. |
| T4.20 | 2026-11-26 | `c8b0157` + bench scripts | partial | [Board clock +2.18/+2.16 ms](../docs/voip/test-report.md#t420-board-clock-two-line-drift-trial), 600/600 markers per slot. Timer +74/+80 ms with incomplete early window. Echo not tested. |
| T4.19 | 2026-11-26 | `d4ec01f` + WT | live fallback verified | [1 kHz negative control and 2100 Hz mode switch on PHONE2](../docs/voip/test-report.md#t419-live-fax-answer-tone-fallback). No fax document transferred. |
| T4.18 | 2026-11-26 | `d4ec01f` + WT | raw units accepted | [Both ports, measured deltas and rejected acoustic data](../docs/voip/test-report.md#t418-live-raw-per-port-gain-evaluation). Electrical calibration deferred. |
| T4.17 / T4.4 / T4.14 | 2026-11-26 | `d4ec01f` + WT | pass / follow-up pass / partial | [Real registration loss and recovery, silent INVITE timeout, unavailable caller display](../docs/voip/test-report.md#registration-loss-unanswered-invite-and-unavailable-cid). |
| T4.16 | 2026-11-26 | `d4ec01f` + WT | functional live pass | [Third INVITE → 486 with both calls preserved](../docs/voip/test-report.md#t416-live-both-busy-admission); prior dual-echo noise remains an open audio investigation. |
| T4.22 / T4.16 | 2026-11-26 | `d4ec01f` + WT | live pass / partial | [Audible MWI set/clear passed; shared-account routing observed, dual echo stopped after loud noise](../docs/voip/test-report.md#t422-live-mwi-and-interrupted-t416-shared-account-check). T4.17 not run. |
| T4.13–T4.15 | 2026-11-26 | `d4ec01f` + WT | live pass / partial / pass | [Single-call hold, codec/port change, CID and two analog ring cadences](../docs/voip/test-report.md#t413t415-live-hold-displayed-identity-and-distinctive-ring). |
| T4.11–T4.12 | 2026-11-26 | `d4ec01f` + WT | live pass | [180/183 distinction, warning timers, lockout and recovery](../docs/voip/test-report.md#t411-and-t412-live-early-media-and-off-hook-warning). |
| T4.8 | 2026-11-26 | `d4ec01f` + WT | evaluated | [Both handset directions](../docs/voip/test-report.md#t48-live-board-leg-g722-bandwidth-evaluation): G.722/PT 9 works; no measurable 5/6 kHz benefit. |
| T4.6, T4.7 | 2026-11-26 | `d4ec01f` + WT | live pass | [Separate PHONE2 calls](../docs/voip/test-report.md#m4-live-mandatory-srtp-and-network-g722): mandatory SDES-SRTP; G.722/PT 9 on LAN with PCMA/PT 8 on board. |
| T4.3 | 2026-11-26 | `d4ec01f` + WT | live pass | [PHONE2 waiting beep, R-key hold/answer/swap, on-hook recall and final release](../docs/voip/test-report.md#t43-live-phone2-call-waiting-r-key-swap-and-recall). |
| T4.2 | 2026-11-26 | `d4ec01f` + WT | live pass | [Concurrent dual-slot calls](../docs/voip/test-report.md#t42-live-two-simultaneous-independent-calls), independent speech and hangup on stock peer. |
| T4.9 | 2026-11-26 | `d4ec01f` + WT | live pass | [Five PBX-initiated releases](../docs/voip/test-report.md#t49-live-remote-disconnect-on-both-ports); audible tone, stop/close and redial on both physical ports (other idle). |
| T4.21 | 2026-11-26 | `4390ec4` + WT | evaluated: unsupported | Rotary on PHONE2: only hook on/off with an acknowledging master; peer never enables pulse detection. [Report](../docs/voip/test-report.md#t421--rotary-dialling-early-evaluation). |
| T3.7 | 2026-11-26 | `4390ec4` + WT | live pass | 31/31 calls released, same PID, stable idle fds/sockets and RSS. |
| T3.6 | 2026-11-26 | `4390ec4` + WT | live latency fail | 40 downlink tone onsets, 105–238 ms, 26 above 150 ms; conversational echo still untested. |
| T3.2–T3.5 | 2026-11-26 | `4390ec4` + WT | live pass | PHONE2/PBX outgoing, incoming with CID, speech round trip, both hangup directions, slot reuse, correct RFC 4733 and INFO digits. |
| T3.2–T3.4 harness | 2026-11-26 | `4390ec4` + WT | host pass | Two SIP UAs, two fixture boards, bidirectional PCMA payloads and release from each end. Live gates open. |
| T3.1 | 2026-11-25 | `4390ec4` + WT | pass | VX830v warm board ready (locale 10) and LAN Asterisk registration, confirmed on both ends in one run. |
| T2.18 | 2026-11-25 | `c96803a` + WT | pass | rustdoc warning-free, `missing_docs` denied. |
| T2.17 | 2026-11-25 | Rust CLI `abcc62e2…` | pass | Cold/warm, hooks, keypads, tones, two-way audio, frame validation and owner-death recovery; [live evidence](../docs/voip/test-report.md#m21-closure). |
| T2.16 | 2026-11-25 | `c96803a` + WT | partial | aarch64 musl cross-build via rustup std + OpenWrt GCC linker; 529 KB dynamic binary runs under QEMU. SDK package build deferred to T5.1. |
| T2.15 | 2026-11-25 | `c96803a` + WT | pass | C smoke (ASan/UBSan) against generated header + staticlib; header current. |
| T2.14 | 2026-11-25 | `c96803a` + WT | pass | proptest 4×4096; fuzz frame/record/link/frame_valid 10 min each, 419 M execs total, no findings. |
| T2.13 | 2026-11-25 | `c96803a` + WT | pass | QEMU differential 19/2528 with Rust vector binary. |
| T2.12 | 2026-11-25 | `c96803a` + WT | pass | 163/163 golden vectors from the C prototype. |
| T2.11 | 2026-11-25 | `c96803a` + WT | pass | fmt, clippy (both feature sets, -D warnings), cargo deny licences/bans/sources. |
| T2.10 | 2026-11-25 | `c96803a` + WT | pass | workspace build + all tests incl. 17 CLI scenarios. |
| M2 closure / T2.4 | 2026-11-25 | `5670764` + WT | pass (review) | Existing live number-display/ring/stop evidence meets §9. Name/date not claimed. |
| T2.6 isolation B2 | 2026-11-25 | `5670764` + WT | pass (flow) | Direction 3, relay off, 999 RTP per slot in 20 s; acoustic comparison unclear. |
| T2.6 isolation A2 | 2026-11-25 | `5670764` + WT | expected no RTP | Direction 0, relay off, 0 RTP each way in 5 s; one incidental RTCP. |
| T2.6 isolation B1 | 2026-11-25 | `5670764` + WT | pass (flow) | Direction 3, relay off, 999 RTP per slot in 20 s. |
| T2.6 isolation A1 | 2026-11-25 | `5670764` + WT | expected no RTP | Direction 0, relay off, 0 RTP each way in 5 s. |
| M2 closure regressions | 2026-11-25 | `5670764` + WT | pass | Host CTest 4/4 and ASan/UBSan CTest 4/4. |
| T2.8 live runs 1–10 | 2026-11-25 | `5670764` + WT | pass | Nine idle, tenth after master owner killed with two active slots; 51–58 ms. All ten sequences verified. [Per-run results](../docs/voip/test-report.md). |
| T2.8 stale-media validation | 2026-11-25 | `5670764` + WT | pass | DD03 after owner death, none in post-recovery sample; fresh two-slot loop forwarded 99 RTP each way. Earlier fractional-sleep setup aborted, not a counted run. |
| T2.8 offline preparation | 2026-11-25 | `5670764` + WT | pass | cli_startup: cold vs warm gates and failed slot cleanup; binary hash verified. |
| T2.8 legacy warm baseline | 2026-11-25 | `5670764` + WT | fail | Fresh DSP_START completion absent after matching transport ACK; 5 s timeout. |
| T2.7 live | 2026-11-25 | `5670764` + WT | pass | All 1,232 frames classified/full-length/CRC OK (20 DD01, 2 DD02, 1,210 DD03); no kernel drops. Raw temporary capture deleted. |
| T2.6 live, 45 s | 2026-11-25 | `5670764` + WT | pass | 2250 RTP / 9 RTCP each way; reverse audio confirmed clearly, forward already confirmed. |
| T2.6 live, 12 s | 2026-11-25 | `5670764` + WT | partial | OEM fixture profile: 600 RTP / 3 RTCP each way; PHONE1→2 audible. |
| T2.6 reference preparation | 2026-11-25 | `5670764` + WT | pass | CLI tests passed for reference profile and rejected activation; AArch64 binaries verified. |
| T2.6 diagnostic, 6 s | 2026-11-25 | `5670764` + WT | fail | No forwarded packets; independent counter saw just two inbound DD03 frames around run. |
| T2.6 initial live, 20 s | 2026-11-25 | `5670764` + WT | fail | DD01 succeeded; direction 0 / DTMF disabled profile yielded no forwarded packets or audible audio. |
| T2.6 initial preparation | 2026-11-25 | `5670764` + WT | pass | Offline cross-slot relay and cleanup regression. |
| T2.5 live | 2026-11-25 | `5670764` + WT | pass | Simple PHONE1 off-hook: selector 4 dial tone, selector 0 silence after ~6 s; slot 0 closed three seconds later. |
| T2.5 CLI preparation | 2026-11-25 | `5670764` + WT | pass | Socket-pair CLI test covered command sequence and standalone stop. AArch64 cross-build and deployed hash verified. |
| T2.4 PHONE1 cadence | 2026-11-25 | `5670764` + WT | pass (audible cadence) | Approximate starts at 0/5/10/15/20 s; ends at 1/6/11/16/21 s. No CID or locale commands. |
| Cadence CLI preparation | 2026-11-25 | `5670764` + WT | pass | Host cli_startup passed; extended bounded window and verified AArch64 deployment. |
| T2.4 PHONE2 cadence follow-up | 2026-11-25 | `5670764` + WT | inconclusive timing | No locale change or CID_PLAY. Brief initial alert then normal ring; about 10 s total, no measured inter-burst interval. |
| T2.4 Italy / PHONE2 | 2026-11-25 | `5670764` + WT | partial | DSP_DEFAULT 10 ACK before CID/ring; number displayed, date/time absent, name unconfirmed. Rang/stopped; timing unassessed. PHONE1 stop also confirmed. |
| T2.4 CID preparation | 2026-11-25 | `5670764` + WT | pass | CLI peer fixture checked profile → CID fields → ring/stop, rebuilt and verified AArch64 binary. |
| T2.4 live | 2026-11-25 | `5670764` + WT | partial | PHONE1 audible ring confirmed; start/stop ACKs, no explicit audible-stop confirmation. PHONE2 not rung; CID not sent. |
| T2.4 CLI preparation | 2026-11-25 | `5670764` + WT | pass | Offline CLI suite includes same-line stop even after lost start ACK; AArch64 binary built and verified. |
| T2.2/Q2 live extension | 2026-11-25 | `5670764` + WT | pass | One FLASH press → DD02 command 19 between lift/hangup, first on PHONE2/line 1 then PHONE1/line 0. Not T4.3. |
| Flash monitor preparation | 2026-11-25 | `5670764` + WT | pass | Host `cli_startup` warm monitor regression and AArch64 cross-build. |
| T2.3 live | 2026-11-25 | `5670764` + WT | pass | One handset moved between ports; 12 exact ASCII press/release pairs per line; exit 0. |
| T2.3 CLI preparation | 2026-11-25 | `5670764` + WT | pass | Offline CLI startup suite including mismatched release word; cross-build and remote checksum verified. |
| T2.1 live | 2026-11-25 | `5670764` + WT | pass | Mainline OpenWrt VX830V, locale 0: attach state 1, fresh DSP-start result 0, slots 0–3 acknowledged, ready in 115 ms. No capture. |
| T2.2 live | 2026-11-25 | `5670764` + WT | pass | PHONE1 then PHONE2 physically lifted and replaced; matched OFF/ON pairs on lines 0 then 1. |
| M2 CLI cross-build | 2026-11-25 | `5670764` + WT | pass | Standalone AArch64 ELF, no dynamic section; QEMU `--help` passed. |
| M2 CLI preparation | 2026-11-25 | `5670764` + WT | pass (offline) | Host and sanitized CTest each 4/4; init, hook events, missing completion and peer errors tested. Not T2.1/T2.2. |
| T1.9 tool discovery review | 2026-11-25 | `5670764` + WT | pass | Fresh optional configure and lint reported skips; strict configure with missing tools was rejected as expected. |
| T1.9/T1.10 review | 2026-11-25 | `5670764` + WT | pass | Actual full lint with local explicit tool paths; formatted header declarations and caller-facing comments reviewed. |
| T1.7 review | 2026-11-25 | `5670764` + WT | pass | Rebuilt and ran all three CTest cases. |
| T1.13 | 2026-11-25 | `5670764` + WT | staging pass; SDK deferred | AArch64 OpenWrt package-local compile/stage + sysrooted pkg-config; standalone SDK at T5.1. |
| T1.13 setup | 2026-11-25 | `5670764` + WT | failed attempts | Top-level scanner group collision; inner make lacked target; first package-local invocation lacked staged Ninja on PATH. Host CMake install succeeded separately. |
| T1.12 | 2026-11-25 | `5670764` + WT | pass | Standalone diff 19/2528; Clang libFuzzer 96,953,265 runs in 601 s. |
| T1.11 | 2026-11-25 | `5670764` + WT | pass | Three link review regressions plus outgoing/incoming receive metadata. |
| T1.10 | 2026-11-25 | `5670764` + WT | pass | [API and implementation review checklist](../docs/voip/test-report.md). |
| T1.9 | 2026-11-25 | `5670764` + WT | pass | Final aggregate lint, 14 files checkpatch/format, 8 C sources tidy/cppcheck. Initial passes identified style, nonportable advice, assertion side effects and cppcheck data-path setup; relative header paths fixed external checkpatch on spaced checkout. |
| T1.8 | 2026-11-25 | `5670764` + WT | pass | Initial and final ASan/UBSan build: CTest 3/3. |
| T1.7 | 2026-11-25 | `5670764` + WT | pass | Initial and final host build: CTest 3/3; optional-QEMU skip verified separately. |
| T1.5 review | 2026-11-25 | `5670764` + WT | pass | 19/19 differential comparisons, deterministic replay seed. |
| T1.6 unit review | 2026-11-25 | `5670764` + WT | pass | ASan/UBSan unit suite. Decoder implementation unchanged. |
| T1.1–T1.4 review | 2026-11-25 | `5670764` + WT | pass | Unsupported `1c`, seeded sequence wrap, queue-full recovery and duplicate ACK replay. |
| M1 build/peer smoke | 2026-11-25 | `5670764` + WT | pass | Static and shared library built; Python peer init/replay/truncation checked offline. |
| T1.5 | 2026-11-25 | `5670764` + WT | pass | QEMU differential: 19 comparisons / 2528 bytes; ring word caveat in [report](../docs/voip/test-report.md). |
| T1.6 unit | 2026-11-25 | `5670764` + WT | pass | Unit suite under ASan/UBSan. |
| T1.1 | 2026-11-25 | `5670764` + WT | pass | CRC vectors and bounded frame round-trip. |
| T1.2 | 2026-11-25 | `5670764` + WT | pass | Fake clock and transport, retry limits, reply matching and errors. |
| T1.3 | 2026-11-25 | `5670764` + WT | pass | DD02 cache, single delivery and negative ACKs. |
| T1.4 | 2026-11-25 | `5670764` + WT | pass | Whole DSP inventory, opaque layouts and both address-family encodings. |
| T1.5 preparation | 2026-11-25 | `5670764` + WT | observed | Isolated probe confirms distinct synthetic control/event/media MAC headers. |
| M1 build | 2026-11-25 | `5670764` + WT | pass | Host archive and AF_PACKET backend compiled. |
| T1.5 preparation | 2026-11-25 | `5670764` + WT | observed | Worker CID/ring sample, followed by three identical-input invocations with differing final ring word. |
| T1.6 fuzz | 2026-11-25 | `5670764` + WT | pass | Decoder libFuzzer/ASan/UBSan run: 107820452 inputs, 601 s. |
| T0.1 | 2026-11-25 | `5670764` + WT | pass | Routine vectors, exact ACK replay and DSP delivery |
| T0.2 | 2026-11-25 | `5670764` + WT | pass | 17 DD01, 1 identical retry, 8 DD02 |
| T0.4 | 2026-11-25 | `5670764` + WT | pass | 72 DSP assertions, 24 INFO writes |
| T0.5 | 2026-11-25 | `5670764` + WT | pass (review) | [Peer evidence and M2 assignments](../docs/voip/peer.md) |
| T0.6 | 2026-11-25 | `5670764` + WT | pass (review) | [Corrected voice transport](protobuf.md#ethertype-safety--no-voip-conflict) |
| T0.3 | 2026-11-25 | `5670764` + WT | **pass (final)** | Full repaired oracle, all six scenarios, exit 0; [results and hashes](../docs/voip/test-report.md). |

<details><summary>T0.3 repair history (superseded runs, newest first)</summary>

| Test ID | Date | Base revision / tree | Result | Evidence / note |
|---|---|---|---|---|
| T0.3 diagnostic | 2026-11-25 | `5670764` + WT | partial | Second post-stub run decoded the two release headers as 1020, indexes 2/3; diagnostics identified on stdout. |
| T0.3 diagnostic | 2026-11-25 | `5670764` + WT | partial | First post-stub teardown exited 0; interleaved OEM text broke binary alignment. |
| T0.3 diagnostic | 2026-11-25 | `5670764` + WT | fail | Address-filtered guest trace located the last call at the management call-history PLT entry. |
| T0.3 diagnostic | 2026-11-25 | `5670764` + WT | fail | Unfiltered guest trace confirmed SIGSEGV inside a shared library. |
| T0.3 diagnostic | 2026-11-25 | `5670764` + WT | fail | Initialized call table + 100 Trying: guest SIGSEGV during teardown. |
| T0.3 diagnostic | 2026-11-25 | `5670764` + WT | partial | Without call status, teardown returned two make-call records, not releases. |
| T0.3 diagnostic | 2026-11-25 | `5670764` + WT | fail | 180 after media serialization: guest SIGSEGV. |
| T0.3 diagnostic | 2026-11-25 | `5670764` + WT | fail | 180 before media serialization: ringback tone interleaved before activation; guest exit 90. |
| T0.3 diagnostic | 2026-11-25 | `5670764` + WT | fail | Baseline bad_digits guest exited 76; reader incorrectly required two originates in negative scenario. |
| T0.3 diagnostic | 2026-11-25 | `5670764` + WT | fail | Baseline teardown guest exited 70; raw output confirmed two channel-close records and terminator. |
| T0.3 diagnostic | 2026-11-25 | `5670764` + WT | fail | Initial isolated teardown: exit 70 at old sequence check. |
| T0.3 diagnostic | 2026-11-25 | `5670764` + WT | partial | Initial isolated default: 4164 bytes; decoded native samples and media offsets. |
| T0.3 | 2026-11-25 | `5670764` + WT | fail | Startup passed; parser reads CHANNEL_OPEN prefix as sample; [diagnosis](../docs/voip/test-report.md) |

</details>

### 13.3 Open questions

| ID | Question | Raised | Owner | Resolution / pointer |
|---|---|---|---|---|
| Q8 | Why is the ordinary caller-ID timestamp blank on the Italy profile, and is ordinary name display supported? | 2026-11-25 | v1 CID follow-up | [PHONE1 ordinary call](../docs/voip/test-report.md#t414-ordinary-caller-id-follow-up-phone1-display): timestamp present in DD01, blank on VX830v, displayed on POTS with the same handset (operator report). Inspect peer serialization / on-hook analog date parameter. Name capability remains unknown. |
| Q7 | Can a clean SDK discover and install the package without the local OpenWrt tree's unrelated group-ID collision? | 2026-11-25 | M5 | T5.1; AArch64 recipe/staging verified in T1.13. |
| Q6 | Does the peer retry a DD02 event after a nonzero ACK, or only after a missing ACK? | 2026-11-25 | M2 | T2.2/T2.7: compare event sequences and delivery after transient receiver backpressure. |
| Q5 | Does a clean zero final RING_START word work on both physical lines? | 2026-11-25 | M2 | Host word varies; no peer read identified. Verify ring/stop and CID at T2.4. |
| Q0 | Controller oracle transcript mismatch | 2026-11-25 | Oracle maintainer | Resolved: all T0.3 scenarios pass; [test report](../docs/voip/test-report.md). |
| Q1 | Are both lines ready after reattach, including a previously active slot and reused sequence? | 2026-11-25 | M2 | T2.8 verified warm recovery, stale media stops and both slots reusable. Ten randomized seeds distinct; deliberate sequence collision only characterized statically. |
| Q2 | What are the physical hook/DTMF boundaries, and is 71 ever emitted? | 2026-11-25 | M2 | T2.2/T2.3, compare timed stimuli to DD02. |
| Q3 | What CID, ring and tone profiles are observable on the fitted SLIC (including locale index 18)? | 2026-11-25 | M2 | T2.4/T2.5, synthetic identities and both ports. |
| Q4 | Does the loopback-address rewrite yield two-way DD03 RTP/RTCP; which codec/ptime pairs work? Is PCM wideband? | 2026-11-25 | M2 | T2.6/T2.7; measure bandwidth benefit in T4.8. [Peer evidence](../docs/voip/peer.md) |

### 13.4 Deviations from this plan

| Date | Section | Change | Reason | Approved by |
|---|---|---|---|---|
| 2026-11-27 | D3, R6, §8.1 | Moved the active module and host harness from 3.16.0 to SDK baresip/libre 4.9.0. Account policy overrides old line keys; warn on fallback for one release. | Match deployed event API and preserve existing installs during migration. | M5 review |
| 2026-11-27 | §9 M5.1, §8.1 | Added M5.1: M5 review follow-ups (renderer/LuCI robustness, baresip 4.9 regression and harness, record 4.9 move) and UCI split: ring group, dial plan, inter-digit timers, registration requirement and Alert-Info move from `line` to `account`; line keeps port/handset options. Migration via uci-defaults. M5 closes only after M5.1. | Service policy belongs to the SIP account; review found blockers. | User |
| 2026-11-26 | §9 M4 exit | Accept partial T4.20 for transition to M5. Carry the two-line, both-direction conversational echo check into M6 alongside (not as a substitute for) T6.8. | No independent listener; two-line drift measured, loud-burst incident still open. | User |
| 2026-11-26 | §9 M4 exit, T4.10 | Accept both-port sweep and measured cadence/frequency, with individual listening labels only where observed. | No additional sweep needed for v1. | User review |
| 2026-11-26 | §9 M4 exit, T4.14 | Accept partial: number, withheld and unavailable displayed. Timestamp missing on VX830v despite same handset displaying it on POTS; ordinary name unknown. Q8 stays open, investigation parked. CW CID off by default. | No attribution from a board ACK; type-II signal uncomfortably loud. | User review |
| 2026-11-26 | §8.1, §9 T4.18 | Corrected TX/RX word mapping; accept per-port raw steps rather than a ±1 dB pass. | Board gain has a measured effect; tip/ring reference setup unavailable. | User |
| 2026-11-26 | §7, §8.1, §9 M4 | M4 evaluations limited to what the stock peer exposes (D4). Static check (peer.md): CW CID, board gain, stutter dial reachable; visual MWI not; rotary likely; added T4.23 port LED. Gain uses CHANNEL_OPEN, software fallback only. | Board firmware is fixed. | User |
| 2026-11-26 | §7, §8.1, §9 M3/M4/M6 | T3.6 removed from M3: latency growth and echo kept as M4 gate T4.20, ≤150 ms latency tuning moved to M6 T6.8. M4 rewritten as complete analog-to-SIP ATA (T4.10–T4.22: tone plan, timers, hold, full CID, distinctive ring, multi-line accounts, registration dependency, gain, fax passthrough, pulse-dial and MWI evaluation); advanced services stay deferred. | Close M3 on functional criteria; v1 must behave like a commercial ATA. | User |
| 2026-11-25 | T2.16 | Accepted hardware-tested dynamic musl CLI for M2.1. Cross-build used rustup's aarch64 std + OpenWrt GCC because the tree has no Rust host toolchain. | Match the OpenWrt packaging model; full SDK package build required at T5.1. | User |
| 2026-11-25 | M2.1 API | `Link::abandon()` replaces the C CLI's full link re-init after a lost ACK (keeps learned peers, sequence, queued events). T2.6 profile override is a hidden `loop --profile A..E` flag instead of link-time wrapping. | Cleaner API; no wire change (golden vectors). | — |
| 2026-11-25 | rbctl-dsl | `af_packet` relicensed MIT (`becbc9e`); `pkt_type` doc and sniff direction labels fixed (`c96803a`); `set_nonblocking` + `AsRawFd` added (`0b79262`). | Licence compatibility in baresip; correct TX/RX labels; poll support. | User |
| 2026-11-25 | §5, §8, §9 M2.1+, §10–§11 | Renamed forward components to the rbctl family (`rbctl-voip`, `rbctl_voip_*`, `rbctl-voicectl`); historical M1–M2 names unchanged. | Consistency with rbctl-dsl. | User |
| 2026-11-25 | §5.2, §9, §11, §13.1 | Added M2.1: library and CLI ported to Rust (C ABI for baresip), reuse of rbctl-dsl crates/recipe; `af_packet` relicensing prerequisite; C tree removed after parity. | Readability and feature velocity; align with rbctl-dsl. | User |
| 2026-11-25 | M1.1 process | Formatting performed as a separate pass, no format-only commit. Used configured AArch64 OpenWrt source tree for packaging smoke. | M1 sources untracked and user requires working-tree-only changes; no standalone SDK available. §§1–12 unchanged in this session. | — |
| 2026-11-25 | §9, §13.1 | Added milestone M1.1 (docs, Linux-style lint/format, CMake, M1 review fixes); tests T1.7–T1.13. | Maintainability before M2; M1 review findings. | User |
| 2026-11-25 | M1 review | Request engine rejects unsupported `1c`; replay cache covers delivered events only. §§1–12 unchanged. | Avoid blocking other commands or permanently rejecting a retry after backpressure. | User review |
| 2026-11-25 | T1.5 comparison detail | Replay the OEM's unspecified final ring word as input for full-record equality. §§1–12 unchanged. | Independent OEM calls disagree on that word; no constant wire value is evidenced. | — |
| 2026-11-25 | T0.3 fixture only | Repaired guest setup and reader. No change to §§1–12, milestone command or exit criterion. | Baseline could neither parse mixed records nor exercise indexed release. | User (oracle repair) |
| 2026-11-25 | — | No deviation; §§1–12 unchanged. | M1 held at the §9 M0 gate. | — |

### 13.5 Session log

#### 2026-11-27 — M5 review; M5.1 added
- Review: check.sh pass; module suite 32/33 on a host baresip 4.9 prefix (`solo` hold/resume fails), does not compile on 3.16 and not from a path with spaces; `render.uc` aborts on LuCI-producible configs (unassigned/dangling port, incomplete account). Added §9 M5.1 with work items and T5.8–T5.13; UCI account/line split agreed with the user.

- 2026-11-26: G3 and §9 T5.1 are package-format neutral per user decision. Record the SDK-selected format in the test result; no recipe change or new acceptance result.

#### 2026-11-26 — M5 implementation and offline preflight
- Accepted partial T4.20 under the §9 exit deviation above. Implemented service packaging, UCI renderer, procd lifecycle, managed reload and the §8.3 ubus interface. The adapter is linked into the existing module instead of a separately loaded module (§5/§8 design detail). Rendered line policy uses the M4 per-port keys. [Offline evidence](../docs/voip/test-report.md#m5-preflight-openwrt-integration); [live acceptance queue](../docs/voip/m5-integration.md). No production install or reboot.

#### 2026-11-26 — T4.20 two-line drift comparison
- Board-paced playout met the 20 ms growth criterion on both ports. Timer showed substantially greater growth. No changes to §§1–12 or clock defaults; ordinary conversational echo remains the outstanding part of T4.20.

#### 2026-11-26 — M4 checkpoint and boot review
- Committed module, bench tools and evidence separately. Initial attach now retries (1/2/4/8 s capped, reset on ready); the delayed-board fixture reaches ready without restarting baresip. CID disabled now clears the date too. All 33 host CTests passed; router module cross-built. Real cold boot remains T5.6.
- §9 exit decisions recorded above. Keep T4.20 in M4, compare timer and board on hardware before selecting the M5 default. T4.16 remains a functional pass; the loud burst remains unexplained. Other review follow-ups: bound the LED worker wait on unload, and simplify dense call paths as touched.

#### 2026-11-26 — Gain disposition and M5 preparation
- Recorded the T4.18 electrical follow-up. T4.10 and T4.20 remain open; T4.14 attribution is parked. Prepared the [M5 integration handoff](../docs/voip/m5-integration.md) from the feed recipe and local target configuration. No M5 gate run; no change to M5 criteria in §§1–12.

#### T4.10 — Short-tone listening and measurement follow-up
- Individually resolved selectors 3/16/17 on PHONE2: three/two/one short ~425 Hz pulses. Raw CH2 recovered the 500/500 ms busy reference and corroborated their cadence. See peer.md. T4.14 remains partial; no independent receiver, decoder or known-good analog CID source available today.

#### 2026-11-26 — T4.16 off-ear isolation diagnostic
- Independent low-level markers, no echo: both shared-account downlinks reached the correct port, with no unexpected digital payload or peak across 24,841 frames. CH2 found only the intended marker above background at each earpiece. Added a host isolation regression. Earlier loud burst remains unexplained; T4.16 is not closed.

#### T4.10 — Dual-port acoustic sweep
- Measured the stock selector set on PHONE1 and PHONE2; independently repeated PHONE2 on Scarlett input 2. Updated peer.md with the numeric frequency/cadence map. The acoustic sweep is complete; remaining listening labels require one tone per question. No change to §§1–12.

#### 2026-11-26 — T4.8 evaluated
- Compared PCMA and G.722 with calibrated source check, both handset directions and decoded board uplink. §6 updated with the measured codec decision. No change to acceptance criteria.

#### 2026-11-26 — T4.1/T4.4/T4.5/T4.23 live batch
- Both-port in-band digits, three PBX failure indications, timed dial-map decisions and aggregate master LED verified. [Measured timings and coverage limits](../docs/voip/test-report.md#m4-live-in-band-digits-progress-tones-dial-map-and-panel). Next: SDES-SRTP and network G.722 against the PBX. §§1–12 unchanged.

#### 2026-11-26 — T4.23 master-side FXS indicator
- Updated §9 T4.23 to use the established OpenWrt LED mapping. Added optional hotplug worker/handler; two-call host regression passed, AArch64 module built. Live transition test pending; no router LED settings changed.

#### 2026-11-26 — M4 autonomous ATA feature pass
- Implemented per-line dial maps/timers, progress tones, registration/MWI, solo flash, identity/cadence policies, shared AOR handling, in-band DTMF, gain words, fax-ready profile, CLI G.722/LED probes and buffer telemetry. Built optional host and target modules. [31 module tests / 20 CLI scenarios](../docs/voip/test-report.md#m4-autonomous-verification-pass); no live gates closed by host tests. [Bench queue](../docs/voip/m4-bench.md). §§1–12 unchanged.

#### T4.3 — Waiting-call and R-key harness
- Implemented a local 300 ms waiting beep, one waiting/held SIP dialog per line, flash swap and on-hook recall. Host call/media test passed; [physical R-key check pending](../docs/voip/test-report.md#t43-call-waiting-host-preflight).

#### 2026-11-26 — T4.2 live pass
- Per-port SIP, PCMA and timer state plus shared serialized board transport. Two-account harness and physical concurrent calls passed. Next: T4.3 waiting/flash and hold; T4.20 ten-minute drift/echo remains separate. No change to §§1–12.

#### 2026-11-26 — T4.9 live pass
- Both ports now give audible disconnect feedback after Asterisk hangs up. Switched line configuration between batches; concurrent calls are not covered. Next: per-line state and media ownership for T4.2. §§1–12 unchanged.

#### 2026-11-26 — M4 tone characterization and disconnect feedback
- Extended `voicectl tone` to peer-supported selectors, measured four on PHONE2 and confirmed selector 1 by ear. Implemented established-call disconnect tone until on-hook, with a real-SIP fixture checking stop/close/re-lift. [Results](../docs/voip/test-report.md#m4-start-disconnect-feedback-and-tone-characterization). Next: verify remote hangup on hardware and continue the per-line refactor. §§1–12 unchanged.

#### 2026-11-26 — T4.21 rotary evaluation (live)
- Operator's rotary phone on PHONE2. With an acknowledging monitor, dialling produced only HOOK_OFF/HOOK_ON. Static follow-up: `pulseEnable` is never set by `voip_server`. Recorded unsupported. One unacknowledged run showed three unexplained `#` digit events; not counted. Router tmpfs cleaned.

#### 2026-11-26 — M4 evaluations grounded in the peer protocol
- Read-only static pass over `voip_server`/`libadam.so` ([peer.md](../docs/voip/peer.md#ata-feature-reachability-m4-evaluation-basis)): dispatched host commands enumerated; CW CID = CID_PLAY + TONE_PLAY 31; digital gain via CHANNEL_OPEN `+04`/`+08` (peer offsets +2/−3); VMS_STATUS only substitutes stutter dial; pulse digits share the DTMF event path; SLIC_CONTROL drives `/proc/tp_led`. T4.8/14/18/21/22 rewritten, T4.23 added. No live tests.

#### 2026-11-26 — M3 closed, M4 rescoped
- At the user's request moved T3.6 out of M3 (growth/echo → T4.20, target → T6.8) and closed M3 on T3.1–T3.5, T3.7. Rewrote §7 and M4 as a complete ATA feature set; added M4 UCI line options. Static checks: no OEM gain setting on the master; board offers fax events, VMS status, 3-way and ring cadence entries. Next: per-line module refactor, then T4.10 tone characterisation.

#### T3.6 — Deadline-paced player and LAN buffer trial
- Anchored 20 ms playout to monotonic deadlines and counted missed periods. Host 9/9; AArch64 build and two PHONE2 calls passed setup. Master downlink median/max 156/189 ms on default settings vs 91/128 ms at `audio_jitter_buffer_delay 2-8`. Residual growth persists. T3.6 remains open; §§1–12 unchanged.

#### 2026-11-26 — T3.6 earpiece follow-up
- 60/60 chirps matched; PHONE2 acoustic arrival relative to the simultaneous Scarlett electrical reference was 157 ms initially, 297 ms median, 357 ms maximum. [Measurement boundaries and results](../docs/voip/test-report.md#acoustic-follow-up-phone2-earpiece-scarlett-solo). Next: instrument and correct playout pacing, then repeat. §§1–12 unchanged.

#### 2026-11-26 — T3.7 passed; T3.6 latency issue reproduced
- Ran the stability test first on PHONE2, then the non-echo tone test. One-way master delay grew during sustained playback. Investigate audio pacing/buffering and retest T3.6. Temporary services and logs removed; no change to §§1–12.

#### 2026-11-26 — M4 scope update
- At the user's request, added T4.9 to the §9 M4 acceptance tests for the missing off-hook disconnect tone observed in T3.4. M3 acceptance criteria are unchanged.

#### 2026-11-26 — Live PHONE2 call and DTMF tests
- Operator confirmed outgoing/incoming calls and post-release redial. Asterisk verified both DTMF modes. Remote hangup leaves the lifted handset silent: track disconnect tone as a UX follow-up. T3.6 latency/echo and T3.7 soak remain open. §§1–12 unchanged.

#### 2026-11-26 — M3 single-line SIP calls integrated
- Added FXS event dispatch, serialized runtime DSP commands, CID/ringing, dial/answer/hangup and a PCMA audio source/player. Two loopback call scenarios and seven startup/lifecycle checks passed. Next: physical handset and routed LAN PBX call checks. §§1–12 unchanged.

#### 2026-11-25 — T3.1 passed on target
- Cross-built and temporarily deployed the non-fixture module with baresip/libre 3.16.0. Warm board startup and an Asterisk contact observed together. Temporary test runtime removed. Next: T3.2 handset-to-SIP call control and audio. §§1–12 unchanged.

#### 2026-11-25 — T3.1 post-attach startup
- Implemented cold init and warm recovery in the module. Both module variants built; seven host checks passed, including missing completions and a rejected final slot default. Next: deploy against the stock peer and test PBX for T3.1. §§1–12 unchanged.

#### 2026-11-25 — T3.1 autonomous attach checks
- Ran real baresip with stopped/running board fixtures in opposite reply orders, a missing-state timeout, and board lifecycle checks: 4/4 host CTests passed. No physical call or board-ready result; T3.2–T3.7 await call control/media. §§1–12 unchanged.

#### 2026-11-25 — M3 harness attach
- Added packet-socket C ABI and libre fd/timer integration for `1a`/`1b`. Host harness can attach over an inherited Unix datagram fd. Both module variants compiled and linked; no runtime tests or board activity this pass. Next: cold/warm startup after attach. §§1–12 unchanged.

#### 2026-11-25 — M3 autonomous host preflight
- Host CTest 2/2: Rust link lifecycle and real baresip registration against a loopback registrar. Loader still returns `ENOSYS` for the scaffold. [Evidence](../docs/voip/test-report.md#m3-host-preflight); no M3 hardware gate claimed. Next: board transport/event loop for T3.1. §§1–12 unchanged.

#### 2026-11-25 — M3 module scaffold
- Added a standalone CMake module target, baresip lifecycle exports and a C owner for the Rust link. Next: integrate the board socket descriptors and timer, then verify board readiness and test-PBX registration (T3.1). No changes to §§1–12.

#### 2026-11-25 — M2.1 closed
- T2.17 passed with the operator on both phones. Retired the historical C feed, reran host checks without it and recorded the target crate licences. §§5.2–5.3 updated to reflect the implemented components and retired layout. T2.16 deferral approved (§13.4). Next: M3 single call via `rbctl_voip_ffi`.

#### 2026-11-25 — M2.1 — Rust port, host checks complete
- Done: workspace `rbctl-feed/rbctl-voip` with `rbctl_voip_proto` (sans-I/O), `rbctl_voip_net` (AF_PACKET over shared `af_packet`), `rbctl_voip_ffi` (staticlib + cbindgen header), `rbctl_voicectl` (all M2 subcommands); OpenWrt `Makefile` (rust-package.mk, stages header + staticlib); `deny.toml`; `scripts/check.sh`; README. af_packet → MIT, pkt_type fix committed.
- Findings: zero wire differences (163 golden vectors, QEMU 19/2528); the 17 CLI scenarios pass unchanged against the Rust binary.
- Tests run: T2.10–T2.15, T2.18 pass; T2.16 partial (no SDK Rust toolchain locally). [Report](../docs/voip/test-report.md#m21--rust-port-host-checks).
- Next: T2.17 on hardware (operator steps in the report), then remove `voip-feed/vx830v-voip` C sources and close M2.1. Commit the new workspace when approved (af_packet additions committed as `0b79262`).
- Blockers: none for the host side.

#### 2026-11-25 — M2 closed — 8/8 criteria met
- Reviewed the physical T2.4 evidence, reran both CTest suites and isolated the direction field on the board. [Results](../docs/voip/test-report.md#m2-closure), [known-good board-leg settings](../docs/voip/foss-design.md#m2-board-leg-settings). Q8 retains extended CID work. No deviation or change to §§1–12. Next: M3 single call.

#### 2026-11-25 — M2 — ten warm master restarts and orphaned streams
- Reproduced redundant DSP_START timeout, implemented warm-slot recovery, verified 10/10 ready and a functional post-recovery media loop. [Results](../docs/voip/test-report.md). No fresh DSP start claimed on warm attach. T2.4 extended metadata remains open. No change to §§1–12.

#### 2026-11-25 — M2 — CRC gate and independent activation-field review
- T2.7 passed using the unchanged working profile. [Counts and capture disposition](../docs/voip/test-report.md). Next milestone test: T2.8 repeated restart/readiness.
- Delegated read-only review traces direction 0/3 to DSP NONE/SENDRECV. [Evidence and one-field-at-a-time follow-up](../docs/voip/peer.md#t26-follow-up-media-direction-vs-telephone-event-settings); Q4 live isolation still open. No plan deviation or changes to §§1–12.

#### 2026-11-25 — M2 — two-way analog audio over DD03
- T2.6 passed with PCMA, 20 ms, OEM activation fixture fields. Both directions confirmed by listening; teardown ACKs received. No audio retained. Q4 now has a working IPv4 profile; isolate direction vs DTMF fields separately if needed. [All four live runs](../docs/voip/test-report.md). No plan deviation.

#### 2026-11-25 — M2 — PHONE1 dial tone on/off
- T2.5 passed with handset feedback, not just ACKs. Warm attach, no locale change. [Results](../docs/voip/test-report.md). Next: T2.6 two-way media, with T2.4 extended CID still open. No new deviations or open questions.

#### 2026-11-25 — M2 — normal cadence confirmed on simple analog PHONE1
- Operator timings support a one-second burst followed by four seconds of silence. No electrical measurements or profile changes. [Live results](../docs/voip/test-report.md). Q3 remains open for impedance and extended CID; no plan deviation.

#### 2026-11-25 — M2 — PHONE2 cadence observation
- Repeated the bounded ring on line 1, leaving the last-requested Italy profile alone. The first audible alert was too short to time; Q3 cadence and alert cause remain open. [Observed result](../docs/voip/test-report.md). No plan deviation.

#### 2026-11-25 — M2 — Italian handset profile and CID
- Requested locale 10 before testing PHONE2; number display and audible stop confirmed. Q3 still covers active electrical settings and cadence; extended metadata not validated. [Results](../docs/voip/test-report.md). No plan deviation.

#### 2026-11-25 — M2 — PHONE1 audible ring; investigate European profile
- Locale 0 remained unchanged. Compiled ETSI row uses the same 600R-default AC symbol as Australia on all seven inspected SLIC tables; runtime impedance and CID modulation remain Q3. Operator reports a CID-capable phone and requested investigation before a second ring. [Ring result](../docs/voip/test-report.md). No plan deviations.

#### 2026-11-25 — M2 — handset FLASH recognized on both ports
- Warm monitor observed both FLASH events. Q2 timing bounds remain open. T2.4 display check deferred for lack of a CID receiver; audible ring/stop and T2.5 tones remain available. No changes to §§1–12.

#### 2026-11-25 — M2 — physical keypad validation
- T2.3 passed on both ports. Reattached to the running DSP without another START. No capture or raw key logging. [Results](../docs/voip/test-report.md). Next: bounded ring/stop and synthetic caller ID, T2.4. No new deviations or open questions.

#### 2026-11-25 — M2 — first live initialization
- Verified master and binary over SSH. T2.1 passed on `lan0.500`; T2.2 verified on both ports in that session. Monitor stopped. Next: verify known digit values (T2.3); no digit values or audio recorded. [Results](../docs/voip/test-report.md). No plan deviation.

#### 2026-11-25 — M2 — first diagnostic executable
- Implemented same-session initialization and hook monitoring; prepared standalone AArch64 binary. [Offline results](../docs/voip/test-report.md). No new plan deviations or changes to §§1–12.
- Next: obtain SSH destination and perform authorized T2.1/T2.2. Warm-peer readiness remains Q1. No live frames sent this session.

#### 2026-11-25 — M1.1 review — reproducible lint and readable declarations
- Missing host tools are explicit skips by default; full milestone lint requires strict configuration. Both modes verified locally. Moved long comments above affected members and clarified return values. No new open questions or deviations; §§1–12 unchanged.

#### 2026-11-25 — M1.1 — CMake, documented API and Linux-style checks
- Split public headers; installed versioned library, archive and pkg-config metadata; removed both old source/test Makefiles. Added offline outgoing-packet regression and CMake quality targets.
- T1.7–T1.12 passed. AArch64 staging verified using the package recipe, standalone SDK check deferred to T5.1. [Results](../docs/voip/test-report.md), [developer commands](../docs/voip/foss-design.md#offline-verification). Next: M2 on authorized hardware.

#### 2026-11-25 — M1 — review follow-up
- Addressed retry, backpressure, sequence and packet filtering findings; linked the replacement documents from the VoIP index. [Integration notes](../docs/voip/foss-design.md) cover command queuing, DSP completion and CID formatting.
- Re-ran T1.1–T1.5 and the sanitizer unit checks offline. M1 remains complete, M2 pending. No commit or real-interface test.

#### 2026-11-25 — M1 — offline protocol library complete
- Added `voip-feed/vx830v-voip`, [design](../docs/voip/foss-design.md), fake transport and differential tests; all six [M1 results](../docs/voip/test-report.md) pass.
- The only value taken from oracle output as an encoder input is the unspecified final ring word. Q5 assigned to T2.4. Next: M2 bring-up on authorized hardware.

#### 2026-11-25 — M0 — controller oracle repaired; M1 gate open
- Reconciled DSP records and native samples; validated all six offline scenarios including indexed release and cross-line isolation. [Results](../docs/voip/test-report.md).
- T0.3 passed on `5670764` + working tree. M0 complete; next milestone M1. No hardware run or T1 result in this repair session.

#### 2026-11-25 — M0 — peer evidence and controller oracle blocker
- Base `5670764` plus working tree. T0.1/2/4/5/6 passed; T0.3 failed after startup. No M1 or hardware tests.
- [Peer analysis](../docs/voip/peer.md): P:404fbc rewrites the IPv4 media endpoints; P:402730 has no `1c` handler. Peer transmits marker `0x10`. Re-init is not a DSP reset.
- [Test report](../docs/voip/test-report.md) records the commands and parser mismatch. Next: oracle maintainer resolves Q0, rerun T0.3; open M1 only after a pass.

<!-- template
#### YYYY-MM-DD — <milestone> — <short title>
- Done:
- Findings (with addresses / doc refs):
- Tests run: T?.? pass/fail
- Next:
- Blockers:
-->
