# Voice board link: compatibility contract from the AArch64 side

[VoIP index](index.md) · [host proxy](master-proxy.md) · [binary identities](map.md)

**Status: transport and command inventory recovered statically; selected stock-host paths verified under QEMU; interoperability not yet established.** This describes the host build in the symbol map. The remote firmware is treated as a black box. “Request” below identifies who initiates a transaction, not a bit in the packet. The analog line number (0–1), media slot (0–3), envelope subtype and inner DSP command are different identifiers.

## Three conversations on `lan0.500`

| EtherType | Initiator | Subtype | Transaction |
|---|---|---|---|
| DD01 | Host | `1a` | Initialize link. Four body bytes **`01 00 00 00`**: a native AArch64 store, not BE32(1). Wait for a four-byte zero status. |
| DD01 | Host | `1b` | Notify after local DSP client connects and peer init is ready. Same four bytes. |
| DD01 | Host | `1c` | Same encoding in a conditional init path. Meaning and reachability in this build unconfirmed; no writer to its enable flag identified. |
| DD01 | Host | `16` | Copy the entire 132-byte DSP record. Wait for a status reply; zero denotes success. |
| DD02 | Board | `18` | Async DSP event/response, normally 132 bytes. Host sends a **DD02** status reply with the request subtype and sequence copied verbatim. |
| DD03 | Either | `19` on host TX | Media datagram, no request/ACK step in the relay. Host TX sequence zero. |

Evidence in `voip_server_proxy`: all four direct callers of request initializer `402028` (`402f58`, `403088`, `403118`, `4031d8`); event dispatch `401ed8` → `402e24` → `402780`; media sender `403a78`. Addresses here and below are hexadecimal Ghidra VAs. The proxy, not the DSL management daemon, opens the sockets. The interface bind uses `ETH_P_ALL` plus a socket filter for the voice EtherType and local/broadcast destination. A VLAN header may be absent at the subinterface capture point.

### Envelope, checksum and transactions

After Ethernet addresses and EtherType, the header is `marker:u8, subtype:u8, sequence:be32, payload_length:be16, crc:be16`. Host marker `11` hex. Payload starts at Ethernet offset 24 on the untagged view. **There is no extra DSL payload-type byte.** CRC-16/ARC, initial zero, reflected polynomial `a001`, no final XOR. Cover the ten-byte header with CRC zeroed, then exactly the declared payload; exclude MACs, EtherType, tags and trailing padding. Store CRC big endian. Verified the 16-entry table at `406a68` against [the checksum reference](../../examples/checksum.py) and the coverage in `401ca8`/`401d10`.

Offline vectors (hex, starting at marker, not captured packets):

| CRC input (CRC field zero) | Expected CRC |
|---|---|
| `11 1a 00000001 0004 0000 01000000` | `277e` |
| `11 18 00000009 0004 0000 00000000` | `591f` |

Host request sequence increments on a **new** request. Retries reuse the saved request; control send pads to at least 60 bytes before FCS. `4024b0` takes milliseconds and a remaining retransmit count: `1a/1b/1c` use 1000 ms, up to 120 retransmits; `16` uses 300 ms, up to two retransmits. These are limits, not required application delays. DD01 and DD03 have separate peer MAC state initialized to broadcast. Following a valid reply the request helper learns the source MAC; the media receiver independently learns from received media. Do not infer that media has already learned the address at control readiness.

**DD01 reply matching (stock host):** after socket filtering, `402344` checks that the received frame covers the declared payload plus the 24-byte prefix and that the CRC is valid. While the requested destination is broadcast it learns the source MAC; thereafter it compares the source with the current peer. It does **not** compare reply subtype or sequence with the outstanding request. Matching peer, subtype, sequence and expected record length is a recommendation for a replacement, not a description of the OEM matcher. The caller still interprets the returned status.

**DD02 event delivery:** the receiver remembers the last sequence and resends its cached reply on a repeat. For a new event `402e24` returns status 1 without enqueueing if the declared body is shorter than 132 bytes. Otherwise it copies the first 132 bytes only if the 15-entry queue has space. A full queue drops the event but returns the same four-byte zero status as a successful enqueue. Thus a success ACK does **not** prove local delivery. Likewise, a DD01 transport reply is distinct from a later DSP completion. A zero status alone cannot establish peer byte order for nonzero errors.

**Local forwarding:** when ready, `403494` passes commands 10–12 to the local media worker before forwarding over DD01. It discards `4031d8`'s return value: neither a DD01 timeout nor a nonzero peer status is reported back to the controller on this path. A local media change therefore does not establish that the peer applied the same change.

### Boot dependencies

The proxy creates `/var/tmp/remote_init` before spawning its workers. A successful `1a` reply removes it and sets the ready state; the controller waits for its disappearance. The proxy has a separate notification for accepting the local client (`1b`). Before readiness the normal proxy path does not forward controller records. In the controller, `SLIC_LOCALE` and `DSP_START` wait for async DSP results, not merely a transport ACK. On receipt of DSP result `2`, `413438` wakes the waiter and conditionally sends channel defaults for slots 0 through 3. This is a dependency graph, not an observed cold-boot transcript.

At cold start the two failure exits differ (`402f58`, `4038b0`):

| Outcome of DD01 `1a` | Proxy state (`418f50`) | Consequence |
|---|---|---|
| Reply with zero status | Set to 2 | Marker removed; local attach can proceed. |
| Reply with a nonzero status | Remains 0 | Init thread returns. After accepting the local client, main polls every 500 ms while holding mutex `419728`; without another state change it never leaves that loop. Marker remains. |
| Retries exhausted | Set to 1 | Init thread returns without scheduling another attempt. Main can leave its wait but skips attach; the marker remains, so `voip_client` continues waiting for it. |

These are static cold-start paths, not a measurement of peer reboot or call recovery. The [controller startup](controller-startup.md) SLIC and DSP reply gates are separate.

## Inner DSP inventory

`voip_client:4134c8` emits a fixed 132-byte record: a BE32 command followed by 128 bytes of body storage. The helper accepts **at most 127 caller-supplied body bytes** (`len < sizeof(msg.buf)`); it zero-fills unused space and does not serialize `len`. TX lengths below count the bytes copied, not the padded record. RX is from client `415e2c`. Names come from `(integer, name pointer)` pairs at proxy `406e90` and client `443230`, not string order. A table entry alone does not establish a required startup operation.

| ID (decimal) | Firmware name (prefix `VOIP_`) | Host observation |
|---:|---|---|
| 0 | DSP_DEFAULT | TX 4, BE32 default/locale selector (values above 20 mapped to zero). |
| 1 | SLIC_LOCALE | TX 4, BE32 country enum ≤20. RX status word: controller recognizes −1 and −2 and signals SLIC init waiter. |
| 2 / 3 | DSP_START / DSP_STOP | TX 4: start word 1 or `0x101`. The stop helper `412f08` sends word 1, but no code references to that helper were found. RX 2 carries state in the first body word; see [restart guard](controller-startup.md#dsp-initialization-and-recovery). |
| 4 | CID_PLAY | TX 116 before ringing: BE32 line at 0, 9-byte date field at 20, two 40-byte identity fields at 29 and 69, BE32 line-selection value at 112. Field selection depends on the number; see [caller-ID encoding below](#what-crosses-the-master-boundary-speech-vs-analog-signaling). |
| 5 / 6 | RING_START / RING_STOP | Start TX 24: BE32 line, argument 5 of `412f6c`, ring mode, `ffffffff`, line selection, final word unspecified. Stop TX 4: BE32 line. |
| 7 / 8 / 9 | CHANNEL_DEFAULT / CHANNEL_OPEN / CHANNEL_UPDATE | TX 7: 4-byte slot. TX 8: 44-byte channel settings, first BE32 slot. 9 named, no direct send identified. |
| 10 / 11 / 12 | RTP_ACTIVE / RTP_UPDATE / RTP_DEACTIVE | TX 120 / 120 / 4. Deactivate body = BE32 slot. All three also go to the proxy's local relay-control pipe when ready. |
| 13 / 14 | TONE_PLAY / TONE_STOP | TX 13: 100 bytes; four BE32 fields (target, tone selector, argument 2, argument 4 of `417278`), two 40-byte text fields, final BE32. 14 named; no direct send identified. |
| 15 / 16 | THREE_WAY_START / THREE_WAY_STOP | TX 8: two BE32 channel identifiers; ordering depends on conference state. |
| 17 / 18 / 19 | HOOK_ON / HOOK_OFF / HOOK_FLASH | RX: BE32 line at body 0; flash also consumes next BE32 word. |
| 20 / 24 / 71 | DTMF_END / DTMF_START / 2833_DTMF | RX: BE32 endpoint and digit, unchanged character through event dispatch; see [DTMF evidence](dtmf.md). The 12-byte TX helper for command 20 (`417c24`) has only unwind metadata references; no code references found. |
| 21 / 22 | FAX_START / FAXMODEM_END | RX: line; 21 also reads a second word. |
| 23 | SLIC_CONTROL | `417ad0` sends nothing for first argument 0 or 1 (`417af8–417afc`). Otherwise it sends two BE32 words: argument − 2 and the resulting selector. Selectors 0/1/3/4 pass unchanged; others normally become 6. If `46b91c` and `46bd24` both equal 1, those other selectors become 6 only when `usbvm_getStaticsFromIndexFile` returns a value other than −1 **and** reports a count > 0; otherwise the original selector is sent. |
| 31 | CHANNEL_CLOSE | TX 4: BE32 channel. |
| 70 | unnamed | TX 16: four BE32 words from a management message handler. |
| 72 / 73 | unnamed | TX 4 each in `42af24` call handling. |
| 74 (`0x4a`) | CALL_INTERNAL_TRANSFER | Named in both tables; no direct send identified. |
| 91 / 92 / 93 | FXS_STAT_GET / FXS_STAT_RESET / VMS_STATUS | Named in both tables. TX 4 each. 91 follows media teardown; RX 91 supplies statistics to call logging. 92 follows a counter reset. 93 is used in a SIP status path. Exact peer semantics not established. |
| 94 | DSPMSG_CMDMAX | Sentinel, not a command. Gaps below 94 do not establish additional implemented commands. |

### Media configuration vs media packets

For the peer-side direction enum and DTMF flags, see the [activation-field trace and hardware A/B test](peer.md#t26-follow-up-media-direction-vs-telephone-event-settings). For the profile that carried audible two-way speech, see the [M2 board-leg settings](foss-design.md#m2-board-leg-settings).

For commands 10/11 the following offsets are **relative to the DSP body**, not the Ethernet header or DSP command. Client serializers `4165f8` / `415908`, bandwidth log `41a450`, codec lookup `41a1a4`, proxy consumer `4040e4`:

| Offset | Encoding | Meaning or limit of evidence |
|---|---|---|
| `00` | BE16 | Activation argument: both paths in `424d24` that call `4165f8` pass `0xfffe`. |
| `02–07` | bytes | Cleared in serializer. |
| `08`, `0c` | BE32 | Controller configuration words from `4214c8` and `4214d8`; exact meanings unconfirmed. |
| `10`, `14`, `18` | BE32 | Slot, DSP media direction, DSP codec. `424d24` calls `41a110`: CMSIP directions 0/1 → DSP 0, 2 → 2, 3 → 3. Codec map: PCMU=0, G726-32=1, PCMA=2, G722=3, G729=4, T.38=5; these are **not** RTP payload types. |
| `1c`, `20`, `24` | BE32 | TX and RX voice payload types from CMSIP words 16/17, then a call-state word (meaning unconfirmed). |
| `28`, `2c` | BE32 | Uplink and downlink packetization time from CMSIP words 14/15. |
| `30`, `34`, `38` | BE32 | DTMF relay mode, TX telephone-event payload type, RX telephone-event payload type. `424d24` selects mode 1 if an RX payload type is supplied; otherwise chooses 0 or 2 from configuration. Mode labels for 0/2 require verification. |
| `3c`, `40` | IPv4 bytes, BE16 | Local IPv4 address and RTP port. Relay derives local RTCP port as RTP+1. |
| `44`, `48`, `4a` | IPv4 bytes, BE16, BE16 | Remote IPv4 address, remote RTP and control ports as serialized. Verify actual RTCP destination separately for each IP family. |
| `4c`, `4d`, `50` | byte, byte, BE32 | `IP_TOS` values for the RTP and RTCP sockets respectively (`setsockopt` at level `IPPROTO_IP`), then another media-state word. |
| `54`, `64`, `74` | 16 bytes, 16 bytes, BE32 | Local and remote IPv6 addresses, then address family. For IPv6 the client writes `htonl(10)` (`00 00 00 0a`). The little-endian proxy loads this word without `ntohl` and compares it to native `10` (`4048ec–4048f0`): it reads `0x0a000000`. **The stock client/proxy pair therefore selects the IPv4 relay path even for an IPv6 media profile.** |

For **media packets**, the DD03 body is `be16 slot, be16 selector, datagram bytes`. Slot is 0–3; selectors 3 and 4 select RTP and RTCP sockets respectively. `403bb0` strips these four bytes and sends the remaining datagram to UDP; `4040e4` prepends them in the reverse direction. Thus the host relay has no reason to decode G.711 to PCM on this path. Four slots are not four analog ports. Confirm negotiated payload numbers, timestamps, RTCP destinations and DTMF with traffic before freezing the ATA media API.

### What crosses the master boundary: speech vs analog signaling

`412f6c` starts with zeroed `CID_PLAY` fields (offsets here are **decimal**, relative to the body). A supplied nonempty name goes at 29. The number goes at 69 **only if `41237c(number) == 0`**. If the result is positive and no nonempty name was supplied, the number goes at 29 instead. On a negative result the number is not copied to either field. The fixture's name-at-29, number-at-69 vector exercises the zero-result branch.

For ordinary calls the proprietary part is the Ethernet wrapper, not a new speech codec. The RTP header and encoded payload travel together. In the [PCMA fixture](worker-transcript.md), a 20 ms G.711 A-law block is 160 octets (8 kHz, one companded octet per sample); the 12-byte RTP header uses payload type 8. The DSP codec selector in command 10 is **2**, not 8. This differs from 16-bit linear PCM (320 sample bytes for the same interval). The stock relay preserved the full datagram. This tests transport, not the analog codec or every advertised mode.

Hook changes and keypad edges arrive as DD02 DSP events even without a media stream. `HOOK_FLASH` is a separate event; no master-side hook-flash timing threshold has been established. The local DTMF edge indication is distinct from how a digit is conveyed to a SIP peer (in-band audio or a negotiated telephone-event RTP payload, for example).

In the ordinary incoming-ring path tested, the master sends caller name, number and date as fields in `CID_PLAY`, then issues `RING_START`. It does not stream an FSK waveform in RTP. That puts physical caller-ID encoding and scheduling behind the board interface. The command order does **not** establish the order on the telephone wire: Bell 202 vs V.23 FSK, DTMF caller ID, inter-ring timing, polarity reversal and off-hook/call-waiting CID have not been verified. The host supplies a locale selector (`SLIC_LOCALE`); its presence alone does not map a locale to a particular electrical profile.

## Reproducible offline checks (no OEM shell needed)

From the repository root run `python3 -B examples/voip_qemu/run.py`. Requires `qemu-aarch64` (tested with 10.2.1), Clang with AArch64 support, `ld.lld` or the LLD shipped with Rust, `readelf`, Python 3, and the extracted master rootfs. Run without Python `-O` so the byte comparisons execute.

The [runner](../../examples/voip_qemu/run.py) checks the two executable hashes, reconstructs missing library symlinks in a temporary directory, and loads the original executable with its musl runtime. A [guest constructor](../../examples/voip_qemu/probe.c) invokes selected OEM routines before `main`, supplies initialized file descriptors backed by private Unix socket pairs, and exits. The parser, serializers, CRC implementation, dispatcher and ACK caching under test are original ARM instructions. This is a **function-level exercise**, not an emulation of the complete proxy, client, peer, raw-socket filter, scheduler, or physical ports. No router access or raw sockets are required.

| Path | What was exercised and compared |
|---|---|
| Local DSP → DD01 | Client generates stop-ring for lines 0/1 and default-channel for slots 0–3. All six records match exactly, including padding. Proxy request builder + sender produce a 60-byte init vector (`277e`) and a 156-byte `16` request containing the client's unchanged line-1 (second port) stop-ring record (`da0a`, sequence 2). The harness supplies the init payload; it does not demonstrate a successful boot handshake. |
| DD02 → local DSP | A synthetic 132-byte off-hook event for the second port, sequence 9, is received twice. The OEM code returns two identical 60-byte success ACKs (`591f`), addresses them to the synthetic source MAC, and queues just one event. Invoking the delivery routine on that slot writes the exact DSP record back out. No physical hook movement is implied. |
| DD03 TX | The OEM wrapper preserves a synthetic 172-byte RTP datagram with a four-byte slot/selector prefix and produces a valid 200-byte `19` frame. Socket selection, codec negotiation and audio quality are not tested by this vector. |

The [stock proxy worker transcript](worker-transcript.md) extends this test to the original proxy main and workers. It covers init/attach, two sequential lines, a retransmission, RTP/RTCP, media-port updates, teardown and controller reattachment. The SIP/controller decisions and board responses are fixture inputs. Comparing independent implementations or a physical peer against this host behavior remains a separate step.

## Eventual on-device conformance check

The classic Ethernet pcaps `vx830v/debug_sessions/first/sniff.pcap` and `second/sniff2.pcap` contain 166 and 664 frames respectively and **zero voice EtherTypes**, including after stripping VLAN tags. `fourth/s4.pcap` has 629 readable records with no voice frames before an inconsistent record length at file offset 64988; the remainder is unverified. None provides a verified voice exchange.

1. Capture the **stock master** from before voice initialization on the board-facing interface. If capturing on `lan0`, preserve VLAN tags; on `lan0.500` they may already be stripped. On a running OpenWrt test unit, for example: `tcpdump -i lan0.500 -s 0 -w /tmp/voice.pcap 'ether proto 0xdd01 or ether proto 0xdd02 or ether proto 0xdd03'`. Arrange capture early enough for cold boot; a capture started by hand after login may miss the handshake.
2. Record action times: idle → line 1 off-hook → known digits → on-hook; incoming ring → answer → two-way G.711 call → hangup. Repeat on line 2. Include normal disconnect/reconnect, and IPv6 or other features only when claiming support. Keep captures private: signaling records can include caller identity, and media contains conversation audio.
3. Run `python3 examples/voip_trace.py -v /path/to/voice.pcap` offline. It prints metadata, distinguishing truncation and CRC mismatch without displaying audio. Match request/ACK pairs, async events and media-slot transitions to the action log. Preserve redacted expected transcripts and regression fixtures.
4. **Pass** when every frame in those scenarios is classified, all CRCs of complete frames match, required body fields/endianness are accounted for, and an independent host implementation reproduces both line lifecycles against the unchanged peer. Unknown optional functions can be excluded from a **two-line ATA subset**, but not called a complete reverse engineering of the vendor protocol. Opaque ring/codec fields, undocumented IDs, `1c`, recovery and IPv6 remain outstanding.
