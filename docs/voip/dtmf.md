# DTMF: handset event, dial digit and network signaling

[Board link](board-link.md) · [CMSIP](cmsip.md) · [VoIP index](index.md)

**A key is not sent in the same format on every interface.** Before a call the controller consumes digits locally for dialing and feature codes. During a call it may need to signal a digit to the other SIP endpoint. Merely receiving a key event on the board link does not mean a telephone-event RTP packet or SIP INFO should be emitted.

## Handset → master

The board-facing envelope is DD02 subtype `0x18`, carrying a 132-byte DSP record. `voip_client:415e2c` reads three BE32 words at record offsets 0, 4 and 8: command, zero-based endpoint, digit value. The following are distinct commands (IDs decimal):

| DSP command | Local event queued | Host interpretation |
|---|---|---|
| 24 `DTMF_START` | `0x1000005a` | Start indication; not another completed digit. |
| 20 `DTMF_END` | `0x10000033` | Completed digit. |
| 71 `2833_DTMF` | `0x10000033` | Same completed-digit event at this boundary. Its name alone does not say to transmit an RTP packet. |

`413728` packs the endpoint into the low half of the queue argument and the digit into the high half. There is no digit lookup in these three dispatch arms. `418c18` unpacks that event for `cmEndptNotify` (`433f60`). Downstream handlers compare the low byte with ASCII keys (e.g. `'0'` through `'4'` in `42880c`); the digit-buffer append at `42d094` stores the low byte, not `value + '0'`. Thus the evidence supports ASCII-valued keypad words, **not** the numeric RFC telephone-event enum on this route. Example candidate record for `#` release on the second port: BE32 `20, 1, 0x23`, followed by zero padding in the fixture. This describes values the host consumes, not a capture of the board's keypad. The two digit words use four bytes each on the link even though the character fits in one byte.

Separately, the **voicemail/SIP-side** digit lookup at `460080`, used at `410948`, explicitly maps numeric `0..9,10,11` to ASCII `'0'..'9','*','#'`. That table is not called by the three DSP dispatch arms. The older proxy worker test's synthetic `word(5)` demonstrated forwarding only; use `word(0x35)` when testing the handset key `'5'` against the controller.

## During a call: three possible network representations

* **SIP INFO:** In the client's state machine near `42ce94`, configuration value 2 and additional active-call/media conditions gate enqueueing `0x400003fa`. Its case at `419288` inside `voip_parse_msg_in_queue` (`FUN_00418c18`) builds CMSIP request 1018 (`OUTDTMF`): native LE32 request type `0x800003fa`, LE32 **call index** (not FXS port), LE32 mode `2`, and ASCII digit at byte 12. In the disclosed `pjsip-apps/src/pjsua/pjsua_app.c:4779–4853`, PJSUA sends an in-dialog `INFO` with `Content-Type: application/dtmf-relay` and body `Signal=#\r\nDuration=160` for `#`. `160` is a fixed value here, not a measured interval between the handset edges. These literals are also present in the master PJSUA ELF.
* **RTP telephone-event (RFC 4733, named RFC2833 by the OEM):** The media setup path (`424d24`) copies negotiated TX/RX telephone-event payload types into DSP body offsets `0x34/0x38` and selects mode 1 at `0x30` when an RX event payload type is present. On standard RTP the payload type identifies the negotiated *telephone-event format*; the event number **inside** its payload identifies the key (`0–9`, `10` for `*`, `11` for `#`). The host proxy relays RTP unchanged. The RFC2833 call to `pjsua_call_dial_dtmf` in this vendor OUTDTMF handler is compiled out in the disclosed source. These findings point to the board/media path generating those packets; a board-originated digit RTP sequence, duration and end repeats have not been observed.
* **In-band:** The digit is a pair of frequencies in normal encoded speech, not an extra SIP or RTP event. The same PJSUA OUTDTMF switch does nothing for in-band mode. This does not establish whether a given physical-line profile passes, regenerates or suppresses a tone.

## Offline evidence and limits

Run `python3 -B examples/voip_qemu/dtmf.py`. The [fixture](../../examples/voip_qemu/dtmf.c) runs the real AArch64 DSP dispatcher and queue for all 12 ordinary keypad characters on both ports and all three commands (72 assertions on emitted events). In a separate step it injects the already-selected outgoing INFO event into the OEM formatter and reads 24 complete 816-byte CMSIP writes, checking their headers, digit and call index. It does **not** run PJSUA or force every key through the live call-state policy. No MIPS firmware or physical keypad is used. The fixture establishes host encoding and separates the two interfaces; it does not prove peer generation, mapping of A–D keys, contact debounce or long-press timing.
