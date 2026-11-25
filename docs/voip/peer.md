# Econet voice peer: static compatibility evidence

Companion to the [master wire contract](board-link.md). **For IPv4, the stock peer substitutes its own loopback RTP endpoints. A host need not put a provider address in a board-leg request.** This is an instruction-level result, not a call on hardware. See [test results](test-report.md) for the separate master-side oracles.

Addresses below are hex. **P** = `userfs/bin/voip_client_proxy`, **S** = `userfs/bin/voip_server` (MIPS32 BE ELF VAs); **A** = `lib/libadam.so` (unrelocated ELF VAs). Module addresses are section-relative. All files are from `vx830v/xDSL/econet_rootfs/`. SHA-256, respectively:

```
P 831e3f35ed965cdb903c847e626e3d6080b8951fcd5f775b789311b076b795cc
S e50a7a6d0ccd7e4c6abeab92220c5098bb9cb6344e55dd7275d86cb75d8db5b4
A deb2e8e31833717136a35cd9535e4ec9dabb7ff8973cadd9f9ab26019cc3ece4
```

Method: read-only Ghidra inspection of `/voip_client_proxy` and `/voip_server` in **RemoteBoard Modulus**, cross-checked against ELF disassembly, MIPS GOT entries, BE data tables, library symbols and DWARF. Function names for local routines are descriptive or from diagnostic strings. **High** means the field, branch, table or call is directly visible; **medium** means interpretation across components. DSP command numbers and selectors below are decimal; DD subtypes and byte offsets are hex unless stated otherwise.

## DD01: what a status actually acknowledges

`msg_initServer` (P:403288) binds DD01 on `eth0.1.500`; P:4030b4 checks declared length and CRC, then calls P:402730. That dispatcher recognizes **only three** subtypes (handler table P:41c760). For a recognized subtype it checks the last sequence **before** invoking the handler; a repeat resends the previous reply buffer without executing the action. The cache key comparison here is sequence, not subtype. `msg_sendResponse` (P:402be0) copies request source to destination and echoes subtype and sequence. Peer marker is **`0x10`** for DD01 responses, DD02 requests (P:402fd4–402fec) and DD03 media (P:4042c4–4042f0). The host's `0x11` is not a universal receive-side constant.

| DD01 subtype | Observed on a new transaction | Econet address | Confidence |
|---|---|---|---|
| `1a` init | Stores link state 2, sends BE32 **0**. No payload-value check or DSP reset in the handler. Main waits on this state before initializing the media relay. | P:403614, 403660–403688; main 4017c8–4017fc | High |
| `1b` attach | Calls `vcp_connect_voip_server`, sends BE32 **0** without checking its return. A new Unix connection starts a reader; if already connected, sends DSP event 2 with cached state instead. | P:403690, 4036dc–403708, 402224 | High |
| `1c` | **No reply** in this build: default return at subtype dispatch, before duplicate handling. No nonzero status for “unsupported”. | P:402744–402774 | High |
| `16` record | Declared body shorter than 132 → BE32 **1** (`00 00 00 01`). Otherwise copy first 132 bytes if the 15-record queue has space; return BE32 **0**, even when full and dropped. No DSP completion implied. | P:403450, 403478–4034a8, 4034c8, 4035b0–40360c | High |

**Master restart (high for code, unverified for recovery):** a fresh `1a` reasserts state; it does not clear slots, queues, or the sequence cache. Main initializes the relay after its startup wait, not on every `1a`. An immediate sequence collision can replay an older reply. The peer reader caches the body of event 2 (P:401e38–401e50, 401ee8–401ef0). On initial Unix attach S:`main` 401930 sends event 2 with its current state; on subsequent attach P:402224 can reproduce it. S:403940 skips reapplying unchanged defaults when initialized; at S:408de0 the event-monitor startup/ready path is gated by saved DSP state. Do not infer that every repeated DSP_START elicits a new completion. **T2.8 + T2.1:** restart ten times, include restart with an open slot, check subtype/sequence, event 2 at attach vs after START, both lines, and whether stale media ceases.

## DD02: handset to controller

| Observation | Econet address | Confidence |
|---|---|---|
| `mtkDSPStart` registers a callback with `adamPollEvent`. Callback puts translated events in a mutex-protected 50-entry queue and signals the empty→nonempty transition. Full queue logs a drop. Monitor writes a zero-initialized 132-byte record to its Unix client; proxy sends it as DD02/`18`, waiting 300 ms with up to two retransmits. | S:4047e4, 403184–403378, 40909c–4092bc; A:003b78, 016afc; P:401c90, 40379c | High |
| Hook callback subtype 1/2/0 becomes internal 100/101/102, then DSP 17 on-hook / 18 off-hook / 19 flash. The line goes in the first body word. On this flash path the next word remains zero, not a measured duration. | S:403250–403278, 4092c4–409398 | High |
| For locale indices 0–20 the three halfwords referenced from S:421ab8 are all `(80,200,350)`. Expanded into `flashMin`, `flashMax`, `releaseMin` at offsets 0,4,8 of the hook configuration passed for interfaces 0 and 1. Library DWARF identifies the fields. | S:403674–4036f8; A:`adamInfcConfigHookTs` 009e18, `infcConfigHookTs` 01af74 | High for settings; actual thresholds/debounce unmeasured |
| ADAM digit event type 1: phase word 1 selects internal 1 → command 24 START, otherwise internal 2 → 20 END. Codes 1–9 stay numeric internally, 10→0, 11→10, 12→11. `voip_convertDtmf` maps these to ASCII `0`–`9`, `*`, `#` in the second BE32 body word. No duration word or fixed delay between these records. No producer of 71 identified in this monitor. | S:40d100, 403224, 40341c–403464, 4093a0, 40947c, 404ee4; pairs S:421c98 | High for 20/24; unknown for 71 |

**T2.2/T2.3:** on both ports compare timestamped hook pulses and short/held keys to DD02 edges; verify flash/release boundaries, repeat suppression and actual detector latency. Include a telephone-event media session to distinguish DD02/71, if present, from RTP digits. Neither the queue timer nor the SIP INFO duration in [dtmf.md](dtmf.md) measures a keypress.

## Caller ID, ring and tone

Fields below are relative to the **body after the command**, not the temporary ADAM parameter block.

| Command | Field use | Econet address | Confidence |
|---|---|---|---|
| 4 CID_PLAY | Copies line `+0`, word `+4`, date `+20` (9 bytes), number `+69` and name `+29` (**15 bytes each**; offsets decimal in this row). Internal handler clears a per-line 304-byte CID cache and stores number, optional name, date. No read of the host's line-selection word at decimal 112 in this case. Not an immediate FSK transmission. | S:408b48–408bc4 → 40225c–402308, cache S:422cb8 | High |
| 5 RING_START | Line `+00` selects interface. `+08` selects a cadence entry (low byte, 18-byte stride); mode 14 instead selects entry 9 and a special action which rings then stops after `usleep(3000000)`. Low bytes of `+0c` and `+10` also copied; `+10 == 3` (low byte) configures both interfaces, ordinary ring path rings both. Otherwise targets requested line. No use of `+04` or `+14` seen in this dispatch arm. Queries/configures ring and supplies cached CID data to `adamInfcRing`; exact presentation depends on profile/path. | S:408cf0–408d2c, 408e90, `mtkRingStart` 4023c0–4027b8, `g_ringProfile` 40e1f0; A:0094e4, 009610 | High for field use |
| 6 RING_STOP | Clears the line's CID cache and calls `adamInfcStopRing(line)`. Type-2 CID is a distinct tone-handler branch, not ordinary pre-ring CID. | S:402310–402368; type-2 call S:402bb0; A:0098b8 | High |
| 13 TONE_PLAY | Selector 0 takes the stop path without looking up a tone. Selector 4 normally maps to 1 (host dial tone); per-line flag can substitute selector 19. Other selectors use the pairs below; cadence and audible meaning depend on locale. | S:40882c–408908, 408d74, 408e54–408e8c; `voip_convertTone` 4094a8, `mtkTonePlay` 402838, `mtkToneStop` 4027bc | High |

**Complete selector lookup**, S:41134c, 27 pairs (decimal, input → internal ID). Absent value → −1 in converter, not a defined tone. Selector 0's table entry is distinct from the command's stop shortcut.

| Selectors | Internal IDs (same order) |
|---|---|
| 0, 1, 2, 3, 4, 5, 6, 7, 9, 10, 15, 16, 17, 19, 31 | 0, 10, 11, 5, 1, 12, 13, 6, 4, 2, 2, 9, 8, 14, 7 |
| 60; 61 through 70; 71 | 26; 16 through 25 respectively; 27 |

**Locale:** S:408cc0 uses DSP_DEFAULT's selector; S:403940 uses pairs at 421b30 for hardware profile and S:403468 selects country tone/hook tables. Do not confuse this with command 1's module/status path S:408bd4. For host indices **0 through 20 in order**, the observed hardware-profile IDs are **1,3,4,5,6,38,9,11,12,18,19,23,25,34,40,27,28,37,45,31,30** (second word zero throughout). As a cross-check, `slic3.ko`'s `le9622_country_mapping` (`.data+170`) labels these IDs Australia, Belgium, Brazil, Canada, China, Colombia, ETSI, France, Germany, India, Italy, Mexico, New Zealand, US, Peru, Poland, Portugal, Russia, **unmapped 45**, Spain, Taiwan. Numeric selection high confidence; country interpretation medium, actual installed SLIC/profile unknown. A:01b374 passes the profile down the SLIC interface. **T2.4/T2.5:** verify the deployed locale and both lines, synthetic name/number truncation, CID modulation and inter-ring timing, cadence, dial/stop and audible selector meanings. Do not assume the table is a list of electrical certifications.

### Electrical profile cross-check (ring investigation)

S:421b30 has `(1,0)` for host locale 0 and `(9,0)` for locale 6. The first word feeds hardware-profile configuration (S:4039a0–4039b8); the ordinary ring path copies the second into CID data (S:4024fc–402520). **Changing 0 to 6 does not change that CID selector.** Its electrical encoding is not established by the numeric value.

In the seven static `slic3.ko` country mappings listed by `nm` (`le9622`, `le9641` default, `le9651`, `le9652`, `le9662`, `zl88601`, `zl88801`), row 9 is `ETS-ETSI`. Checked ELF relocations at **table base + 100 × profile ID + 72** (decimal byte offsets): within each model, rows 1 and 9 point to the **same** AC coefficient symbol, named `AC_FXS_*600R_DEF*`. E.g. `le9622_country_mapping` at `.data+0x170` uses `AC_FXS_ZL880A_RF14_600R_DEF` for both. This is high-confidence evidence about the compiled defaults, not a readback of the installed SLIC, a measured impedance, or proof of ETSI conformance. `libadam.so` distinguishes a `SLIC_IMPC_CTR21` enum from `SLIC_IMPC_600ohms`; enum presence does not show that the running driver selected it. Identify the active SLIC and any runtime profile override before treating a locale change as an impedance change. A CID display test verifies receiver interoperability; to distinguish V.23 from Bell 202, verify the actual modulation/configuration separately.

## RTP addressing and codec limits

| Finding | Static evidence | Confidence |
|---|---|---|
| Commands 10/11, slot 0–3: body `+10` selects per-slot data. Both IPv4 addresses (`+3c`, `+44`) become `127.0.0.1`; local RTP `+40` comes from P:41f758, remote RTP/RTCP `+48/+4a` from P:41f698/41f69a, each indexed by `slot * 0x30`. This mutates the **same record** subsequently sent to the Unix server. 12 goes through relay control but not the address rewrite. | P:401be4–401c3c → `vcp_rtp_msg_pre_process` 404fbc | High |
| Relay allocates local port pairs, not provider ports. Counter starts at 60000 (P:41c7da); first four pairs reserved for DSP endpoints, next four bound for relay sockets, advancing by two and retrying bind failures. DD03 carries the datagram with slot/selector. Do not hardcode peer UDP ports in the master. | P:403ba0–403dfc, 4049fc, DD03 workers 403e04/404370, TX 4042c4–40430c | High |
| Server maps fields from the preprocessed record into its internal stream parameters. `mtkRTPActive` checks RTP ports, uses the local address/RTP and RTP+1 for local RTCP, remote address and separate RTP/RTCP ports for the destination, then calls `adamStrmConfig` and `adamStrmStart`. Update has a parallel address path. The family (`+74`) **is not rewritten**: 10 selects 16-byte addresses (`+54/+64`); don't rely on loopback substitution for IPv6. | S:408a0c–408a78, 404270–4042dc, 404460–404488, 40458c, 4043e0, 40441c; update 403d58; A:008cc8, 01d93c, 004bac | High |
| Codec selector `+18`: the six explicit cases 0..5 (PCMU, G726-32, PCMA, G722, G729, T.38 per master map) translate to internal 0,2,8,9,18,24 then ADAM 1,4,0,2,6,12. These three number spaces are distinct from negotiated RTP payload type. No iLBC selector in this translator. | S:40896c, 411334, 408f08–408f48, 4042d0–404318, 40d2fc | High for translation; not proof of working codecs |
| Both ptime words `+28/+2c` go through `MTKPTimeConvert`: 10/20/30/40/50/60 ms → 0/1/2/3/4/5. Other values log invalid and return 1 (20 ms encoding), not a transport rejection. | S:408988–408998, 404310–404330, 403b3c and jump table 40d1bc | High |
| Wideband hints: G.722 is selected above, `ovdsp.ko` has relocations to `ACODEC_Init_G722` at 10bd8/10be4; `fxs3.ko` implements `wideBandDet` at `.text+1d7dc`. Neither establishes that the fitted analog path samples at 16 kHz. | S:408f20; module symbols/relocations | High for code presence; physical path unknown |

**R1 interpretation:** choose a consistent IPv4 family and board-leg profile (PCMA/20 ms initially). For valid slots the DSP-facing IPv4 endpoints are local to the peer, irrespective of the five on-wire IPv4 address/port fields. The host still owns its own network leg. **T2.6/T2.7:** compare two synthetic IPv4 endpoint tuples with the same slot/codec; verify two-way DD03 RTP, RTCP selector, payload size, timestamp step and teardown. Try PCMU, then other advertised codec/ptime pairs separately; the six converter entries are not a tested cross-product. For the 16 kHz question record the active PCM/SLIC mode during a G.722 loop in T2.6; end-to-end bandwidth/benefit remains T4.8. No IPv6 interoperability claim.

### T2.6 follow-up: media direction vs telephone-event settings

Read-only static review, same images as above; K = `ovdsp.ko` `.text` offset. On PCMA activation, the DSP body `+14` word (hex) maps as follows. These are **separate enums**, not interchangeable numeric values.

| Wire | Server intermediate | ADAM | Kernel `xmit_mode` |
|---:|---:|---:|---|
| 0 | 4 | 0 | 1 (`SDMODE_NONE`) |
| 1 | 1 | 1 | 2 (`SENDONLY`) |
| 2 | 2 | 2 | 4 (`RECVONLY`) |
| 3 | 3 | 3 | 8 (`SENDRECV`) |

**High confidence, inspected path:** S:40893c–408968 and 408d48 translate the wire value; S:404350–40439c indexes the table at 40d358. A:1cb64–1cbf4, 1d43c–1d468 and 1e93c translate/program the kernel mode. `mediaXmitModeSet` (A:0f930) passes `xmit_mode` at ioctl argument offset `+58` to K:15ff8–16004. K:0fcdc–0fd18 supplies mode names. The transmit gate at K:27d44 tests mask `0x0a`; the receive gate at K:1da54 tests `0x0c`. NONE passes neither gate; SENDRECV passes both. This is a mechanism for missing speech, not a claim that all incidental RTCP stops.

**Separate DTMF fields:** S:404328–404350, 404580–404588 and 4045bc–4045e4 select `(dtmfRelay,dtmfRemove)` = `(0,1)` for wire mode 0, `(1,1)` for mode 1 and `(0,0)` for mode 2. Mode 1 uses the TX event PT at `+34`, substituting 96 for zero. RX event PT `+38` is copied at S:4089e4–4089f0; no direct read of that member identified in `mtkRTPActive`. Neither this nor the equal TX/RX values in the fixture establishes whether the RX field matters elsewhere. Voice PT remains 8 independently of these settings.

**Live isolation (2026-11-25):** A/B/A/B, `(direction, mode, TX event PT, RX event PT)` = A `(0,0,0,0)` vs B `(3,0,0,0)`. Same channels, codec, ports and ptime on every activation. A: 0 RTP both ways in both 5 s windows. B: 999 RTP each way in each 20 s window. Thus direction alone restored sustained packet flow with telephone-event relay disabled. The listening result for this comparison was unclear; audible two-way speech was verified separately with `(3,1,101,101)` at T2.6. No independent DTMF interoperability claim. To isolate event handling later, compare B → `(3,1,0,0)` → `(3,1,101,0)` → `(3,1,101,101)` with a synthetic key; mode 1 with TX PT zero selects 96. [Counts and test method](test-report.md#m2-closure).
