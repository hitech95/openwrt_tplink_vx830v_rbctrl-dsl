# M4 verification queue

**T4.10 follow-up:** isolated PHONE2 listening and reference-validated raw CH2
measurement identified selectors 3/16/17 as three/two/one dial-pitch pulses;
do not replay these to resolve the earlier "silent or short" ambiguity. See
[durations and evidence scope](peer.md). The full sweeps already exist; a
strict both-port ear sign-off must distinguish measured-only entries from
individual listening observations. For T4.14, PHONE1 already displays CID timestamps
on POTS. T4.10 is accepted as evaluated; T4.14 is accepted partial. Attribution
of the missing timestamp and ordinary caller-name support are parked under Q8.

## T4.20 live comparison

Run two independent calls concurrently for ten minutes with `timer`, then repeat
with `board`. Keep accounts, PCMA profile, jitter settings and zero gains the
same. Keep `cw_cid` off. Start the clock once both calls carry media. For drift,
keep the handsets off-ear; send distinct quiet markers from the two SIP endpoints
without returning microphone audio. Timestamp each marker at SIP ingress and at
its matching DD03 slot on the same router clock. For each slot record first- and
last-minute median delay, marker counts and the signed difference; note packet
loss and buffer depth alongside them. Missing markers are not zero delay.

Compare each port separately to the 20 ms criterion. Echo is a separate
observation: whether each talker hears delayed self-speech, at both ends of
both calls. Do not recreate the at-ear dual-echo loop; if ordinary conversation
cannot be checked, report drift alone and leave echo open. Give instructions and
await an explicit Ready before each live phase. [Board pacing passed drift](test-report.md#t420-board-clock-two-line-drift-trial)
on both slots (+2.2 ms each). Timer showed +74/+80 ms with partial first-minute
coverage. Only the conversational echo check remains for this gate.

## Earlier bench results

**T4.19 evaluated:** [live CED fallback](test-report.md#t419-live-fax-answer-tone-fallback) passed on an existing G.711 call. End-to-end document delivery remains untested.

**Registration batch:** T4.17 and the T4.4 no-response follow-up passed; PHONE1
also displayed unavailable/out of area. T4.14 remains partial (ordinary name and
date/time). [Evidence](test-report.md#registration-loss-unanswered-invite-and-unavailable-cid).

**T4.16 functional gate passed:** [both-busy 486 with both original dialogs preserved](test-report.md#t416-live-both-busy-admission), together with the earlier shared-line and isolation runs. Keep the prior loud dual-echo incident tracked separately; do not use at-ear dual echo for these checks.

**T4.22 passed** on PHONE2 with a real SIP notification exchange and a synthetic
mailbox. In the initial T4.16 run ring-all and free-port routing worked, but a very loud
noise was reported during dual echo. Investigate off-ear before another operator
listening run. [Evidence](test-report.md#t422-live-mwi-and-interrupted-t416-shared-account-check).

**Hold, CID and distinctive ring:** T4.13 and T4.15 passed. For this setup use
PHONE1 for the display, PHONE2 for analog cadence. T4.14 partial: private and
truncated number displayed; date/time missing, waiting name only. Selector 31
was uncomfortably loud; keep `cw_cid` off outside a controlled decoder test.

**Progress tones and timers:** [T4.11–T4.12 passed live on PHONE2](test-report.md#t411-and-t412-live-early-media-and-off-hook-warning). T4.10 individual ear labels remain separate.

**Tone-map update:** T4.10 has [frequency/cadence observations](peer.md) for the full selector sweep on both ports; only individual listening labels and unresolved short/quiet responses remain. Do not repeat the entire sweep solely for those labels.

**Latest live batch:** [T4.8 evaluated: keep PCMA on the board](test-report.md#t48-live-board-leg-g722-bandwidth-evaluation). [T4.6 and T4.7 passed on PHONE2](test-report.md#m4-live-mandatory-srtp-and-network-g722). T4.1, the three T4.4 failure categories, T4.5 and T4.23 were also
verified on the router. See [results](test-report.md#m4-live-in-band-digits-progress-tones-dial-map-and-panel).
The table below also serves as a repeatable regression procedure. The separate
no-response case for T4.4 subsequently passed in the registration batch above.

Use the module's [config reference](../../rbctl-feed/rbctl-voip/modules/rbctl_voip/README.md#m4-bench-controls).
Retain command statuses, measured levels/cadences, anonymized SIP outcomes and
aggregate counts; use synthetic identities. Change only one test profile at a
time. The standalone CLI and baresip must not own the board simultaneously.

| Gate | Preparation/evidence | What the bench must establish |
|---|---|---|
| T4.1 | In-band mode tested on both fixture ports through real PCMA SIP media: frequency pair present at the far end, ordinary audio returns after release. | Dial a PBX answering extension on each port; Asterisk in-band detector logs each synthetic key once. Recheck SIP INFO/RFC 4733 separately. |
| T4.2, T4.3, T4.9 | Passed live previously. Regression fixtures retained. | Regression only after the new policy changes. |
| T4.4 | Fixture returns 486/600/603, 404/410/484/604, 408/480/487 and 500/503. Verifies busy/fast cadence selector or local SIT path and slot cleanup. | Listen and distinguish each tone at locale 10; repeat a no-response/transport failure. Adjust selector mapping if necessary. |
| T4.5 | Per-line maps, explicit send, exact match, ambiguous short timeout, long prefix expiry. | Try `202T\|2021\|3[0-5]x` against PBX extensions; check URI, delay, immediate `#`, bad prefix and on-hook cancellation. |
| T4.6, T4.7 | Host calls with SDES mandatory and G.722-only network codec + resampling. Optional modules cross-built for AArch64. | SIP/SDP on Asterisk (SDES required, then separately G.722), two-way speech; confirm board still uses PT 8. No clear-RTP fallback in mandatory test. |
| T4.8 | `rbctl-voicectl loop --line 0 --to 1 --g722 --duration-ms 60000`; fixture checks cmd 10 codec 3/PT 9 and unchanged DD03 datagrams. | Measure above-4-kHz energy vs PCMA reference on each handset and swap source/sink. Packet flow alone is not a wideband result. |
| T4.10, T4.11 | CLI/table covers peer-supported selectors. Software SIT, CW beep, recall, ringback and bounded howler are available. | Complete operator labels and cadence/frequency table on both lines (31 is type-II CID). Compare 180 without SDP to 183 with remote audio. Board selector table is not an acoustic certification. |
| T4.12 | Fast host tests for no first digit, incomplete number, post-disconnect warning, lockout and on-hook cleanup. | Check actual durations and audibility, including hook-on during each phase and the unaffected line. |
| T4.13 | Single-call R puts call on hold with recall dial tone; second R resumes. Established remote SDP refresh sends RTP_UPDATE with unchanged board endpoints. | PBX hold directions and mid-dialog codec/port change, no stale audio or loss of slot. |
| T4.14 | Fixture checks asserted identity before RPID/From, privacy suppresses name/number, missing number `O`, 15-byte number cap/date and optional type-II command sequence. | Known-capable display: ordinary and waiting name/number/date, P/O, UTF-8 repertoire; repeat Q8 and explicitly report unsupported display fields. |
| T4.15 | Known `Alert-Info` maps to cmd 5 body +08; no match maps to 0. Real SIP CANCEL fixture sees ring stop. | PBX sends two alerts; listen/measure physical cadence, CANCEL, answer, completed-elsewhere. |
| T4.16 | Same AOR on both ports; two-endpoint harness verifies ring-all, second port answering first, hunt, a concurrent second call to free port and independent audio. | Repeat with actual shared registration; confirm busy/CW policy when neither port is free. |
| T4.17, T4.22 | Real SUBSCRIBE/NOTIFY fixture: withheld registration → congestion; REGISTER OK restores dial tone; yes/no NOTIFY changes selector 4↔19. | Disable/re-enable LAN PBX registration, then set/clear actual mailbox waiting. Audible stutter only, no visual lamp claim. |
| T4.18 | [Evaluated on both ports](test-report.md#t418-live-raw-per-port-gain-evaluation): independent raw TX/RX settings, −6…+6, default 0. | Electrical calibration deferred until an isolated tip/ring setup and known load are available. |
| T4.19 | Opt-in fixed-buffer G.711 profile: fixture drives 21/22 plus synthetic 2100 Hz CED in PCMA; checks relay-off/on updates and continuing speech. | Send a real test fax to ReceiveFAX or record the fallback detection result. Codec migration from a non-G.711 call is **not implemented**. |
| T4.20 | Board-clock and timer-clock modes, per-slot periodic buffer-depth and pacing counters, previous one-minute measurements. | Two concurrent ten-minute calls, first/last minute comparison and conversational echo in both directions. Drift mitigation may still need code. |
| T4.21 | Evaluated on physical rotary handset: unsupported on this peer. | No host pulse decoder planned. |
| T4.23 | Master-side `rbctl-voip-led` hotplug handler uses the existing `green:fxs` and `red:fxs` LED class devices. Host fixture checks that hanging up one call preserves ACTIVE for the other. | During normal calls check registered idle → solid green, ringing/waiting → fast green blink, off-hook/active → slow green blink, last on-hook → registered idle. Unavailable board/no registered account while idle → red. Module stop → both off. No wiring check or Econet LED command required. |

UCI generation, service installation and ubus are M5. Whether unlabelled
tones, gain, wideband, CW CID, fax and LEDs work on this board is an output
of these measurements. Do not mark an evaluation passed merely because the
peer ACKed a command.
