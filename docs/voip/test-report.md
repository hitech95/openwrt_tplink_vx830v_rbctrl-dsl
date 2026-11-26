# VX830v FOSS voice milestone results

## T4.18 live: raw per-port gain evaluation

Italy 10, one slot at a time, handsets off-ear. A Scarlett headphone driver
applied a fixed 1500 Hz mouthpiece stimulus, measured on decoded board uplink.
A separate fixed 1 kHz PCMA marker drove the earpiece, measured on the powered
Scarlett CH1 measurement mic. No microphone signal was echoed. Each nonzero
setting was bracketed by zero settings; all runs ended with deactivate/close.
The initial run without microphone power is excluded. Values below are relative
to the mean of the adjacent zero runs, not absolute levels or calibrated SPL.

| Changed board word (other zero) | PHONE1 uplink Δ dB | PHONE2 uplink Δ dB | PHONE2 earpiece Δ dB |
|---|---:|---:|---:|
| +04 = −6 | 0.00 | +0.09 | −3.54 |
| +04 = +6 | +0.01 | −0.01 | +2.47 |
| +08 = −6 | −3.02 | −3.96 | −0.23 |
| +08 = +6 | +3.01 | +2.96 | +0.64 |

PHONE1 acoustic zero brackets moved by up to 12.75 dB; its RX magnitudes
are not usable. PHONE2 acoustic zero brackets moved by up to 1.72 dB. TX
zero-to-zero drift was at most 0.11 dB. These results reverse the previous
handset-direction interpretation of the board words: TX (toward network) is
+08; RX (toward handset) is +04. Source levels did not approach digital full
scale at the tested settings; these low-level trials do not establish maximum
unclipped gain. Per operator decision expose independent **raw steps**, limited
to −6…+6, rather than claiming ±1 dB accuracy across handsets. Keep both at
zero by default. T4.18 closed as an evaluation, not a calibrated dB pass.

**Deferred electrical check (2026-11-26):** A repeatable tip-to-ring measurement
under a known off-hook load would separate line-level gain from handset/mic
placement. For RX, compare the differential line voltage at each setting to
zero with the same digital stimulus. For TX, hold an isolated line stimulus
constant and compare decoded uplink levels. Use a telephone-line-rated isolated
differential probe or telecom test set, not a grounded scope lead on either
wire. No suitable setup is available now; no electrical dB/step or clipping
measurement was made. This is a future line-level check, not a handset SPL test.

## T4.19 live: fax-answer-tone fallback

2026-11-26, PHONE2 off-ear, slot 1, Italy 10. A Scarlett headphone driver
coupled to the mouthpiece played two synthetic three-second signals. A LAN
endpoint accepted the PCMA-only call and counted uplink RTP; it returned no
audio. Baresip used fixed RTP and audio buffers with no PLC/VAD modules loaded.

| Stimulus | Observation |
|---|---|
| 1 kHz control | No fax request or RTP_UPDATE; zero frames meeting the CED detector criterion. |
| 2100 Hz answer tone | 147 consecutive qualifying 20 ms frames at the LAN endpoint. Module reported fax passthrough on; stock peer ACKed DD01 command 11 with status zero. Slot/direction/codec/PTs stayed `1/3/2/8/8`; DTMF relay and event PTs changed from `1/101/101` to `0/0/0`. No matching DD02 fax start/end event was seen. |
| Hangup | Same SIP dialog carried 3558 PT 8 packets over 71 s, ended with BYE; RTP_DEACTIVE and CHANNEL_CLOSE acknowledged. |

**Closed by the plan's no-fax-machine fallback:** acoustic CED detection and
in-call mode switching verified on hardware. No T.30 negotiation, fax page,
ReceiveFAX result, or automatic migration from a non-G.711 codec was tested.
Temporary endpoint, router runtime and control trace removed; no audio saved.

## T4.14 ordinary caller-ID follow-up: PHONE1 display

The operator reports that this same handset displays a caller-ID timestamp on
POTS. Its date/time fields stayed blank on the VX830v Italy profile. This is
an end-to-end timestamp interoperability failure on a timestamp-capable handset.
Ordinary caller-name capability is still unknown. The POTS signal itself was
not captured or decoded in this test.

2026-11-26, Italy locale 10, on-hook PHONE1 (line 0), waiting CID disabled.
One synthetic INVITE supplied different PAI, RPID and From identities. The
DD01 CID_PLAY record carried the PAI name `OrchidBench1234` and number
`123456789876543` (both capped at 15 bytes), plus `11262019` (`MMDDhhmm`).
Econet acknowledged CID_PLAY, RING_START and RING_STOP with status zero.
SIP trace showed 180 then 486; this was an unanswered display observation,
not a successful CANCEL assertion. Operator reported **number only, no caller
name or date/time**; did not separately transcribe the LCD digits this time.
No audio or type-II burst. Temporary runtime and trace removed.

Together with the earlier private/unavailable and waiting-name observations,
this characterizes this handset. It does not establish whether the peer omitted
name/date from the analog FSK or the handset chose not to display them. T4.14
and Q8 stay partial. For the timestamp, inspect the peer's CID serializer and,
if available, decode the router's on-hook analog CID message to check the date
parameter's presence, format and length. No second handset is required to
establish the display discrepancy.

## Registration loss, unanswered INVITE and unavailable CID

2026-11-26, stock Econet/Italy profile 10, temporary LAN registrar and SIP
endpoint. PHONE2 was kept off-ear, Scarlett CH2 at its earpiece. No echo,
waiting-CID signaling or recorded audio. Three independent observations:

| Gate | Evidence | Result |
|---|---|---|
| T4.17 | With a registered shared AOR, gated PHONE2/slot 1 used selector 4 (425 Hz; 200 on/200 off/600 on/1000 off ms). A rejected renewal (503) while lifted switched it to selector 2, measured 200/200 ms. During the same outage ungated PHONE1/slot 0 still opened with selector 4, then stopped and closed on-hook. After REGISTER 200, PHONE2 returned to measured normal cadence without a hook cycle or process restart. | Live pass. PHONE1 comparison verified by status-zero board commands, PHONE2 also acoustically. |
| T4.4 no-response follow-up | While registration kept renewing successfully, the endpoint deliberately ignored all INVITEs to the test extension. Baresip closed with ETIMEDOUT; dial-tone stop to congestion command was 32.78 s on the router. Selector 2 ACKed; CH2 measured 425 Hz, 200/200 ms; slot closed after hook-on. | Live pass, distinct from a SIP 503 final response. |
| T4.14 unavailable | Ordinary on-hook INVITE on PHONE1 with a nonnumeric From user and no PAI/RPID. Operator identified the display as unavailable/out of area (not a transcription of exact LCD wording). CID, ring start and ring stop ACKed. The 36-byte snapshot did not include the CID number field; `O` encoding is covered by the host test. | Unavailable-display subcase passed; extended name/date remains partial. |

Temporary runtime, endpoint, configs and control trace removed; 72 control
frames captured, zero kernel drops. This session did not exercise T4.16 or
repeat the prior loud-noise incident.

## T4.16 live: both-busy admission

2026-11-26, two ports on one SIP AOR, call waiting disabled on both. Off-ear,
no downlink RTP sent, uplink packets counted and discarded. Two incoming
dialogs each received 180 then 200. With both Up the third INVITE received
486 Busy Here directly (no 180, no media); the operator observed no new ring.
The original dialogs stayed Up and each delivered 187 additional PCMA packets
across the rejection observation window. Replacing both handsets generated a
BYE for each original dialog. Services and temporary files removed.

**T4.16 functional gate passed** on this result plus the earlier ring-all,
first-answer, free-port and off-ear isolation checks. The loud burst previously
reported with dual echo is an independent unresolved audio incident; this
signaling test neither reproduced nor explained it. No at-ear dual echo retest.

## T4.22 live MWI and interrupted T4.16 shared-account check

**Off-ear isolation follow-up, 2026-11-26.** Two incoming dialogs, same AOR,
PHONE2 answered first. Independent synthetic PCMA transmitters discarded all
microphone RTP; neither echoed it back. Only one marker at a time (700 Hz on
slot 1 or 1300 Hz on slot 0), with silence windows, handsets separated and CH2
mic moved between earpieces. Numbers below are digital levels, not calibrated
SPL. No listening or cough reproduction. No audio recordings.

| Output | Digital board-bound PT 8 | Scarlett measurement |
|---|---|---|
| PHONE2 / slot 1 | 272 exact 700 Hz packets, 13484 quiet, 0 foreign/other | Own bin −90.6 dBFS vs −108.6 idle; foreign bin −106.5 vs −107.4 idle. |
| PHONE1 / slot 0 | 272 exact 1300 Hz packets, 10813 quiet, 0 foreign/other | Own bin −76.6 dBFS vs −98.1 idle; foreign bin −99.2 vs −99.3 idle. |

All 24,841 captured downlink frames accounted for, zero kernel drops; maximum
decoded peak 328/32768 on either slot. Both SIP dialogs released with BYE.
A new host fixture also checks four distinct low-level AC markers and peaks
in both directions through two actual UAs and the shared account, including
the surviving call after release. 980/980 post-settling fixture frames matched
their expected PCM block, peak 504/32768. Neither run reproduced cross-feed
or a level excursion under these conditions. Neither rules out feedback in
the earlier live echo topology, upstream/analog coupling or an intermittent
fault. **Keep at-ear dual echo suspended.** Both-busy admission still pending.

2026-11-26. Two physical ports mapped to one UA. A LAN registrar/notifier
provided a synthetic mailbox, not a voice-recording service. **T4.22 passed
on PHONE2:** actual `SUBSCRIBE message-summary` and three acknowledged NOTIFYs
(`no`, `yes`, `no`). Operator heard normal dial tone, stutter on the next lift,
then normal dial tone again without hanging up when the mailbox was cleared.
DD01 tone selectors 4, 19 and 4 on slot 1 all returned status zero. No visual
indicator, mailbox login or audio recording needed.

**T4.16 partial — suspend at-ear dual echo tests.** A new INVITE rang both
ports; answering PHONE2 stopped PHONE1 and established two-way echo. A second
INVITE to the same AOR rang the free PHONE1 and established a separate dialog.
Operator reported possible faint shared audio (room coupling uncertain) and a
very loud noise on PHONE2 after a cough, requiring hangup. Audio independence
and the both-busy policy are not verified here. Logs show baresip still running
after the event, normal slot-1 SIP termination and status-zero deactivation/
close; no evidence identifies a DSP crash or the cause of the burst. Test
endpoint echoed each dialog's RTP to its own negotiated destination; both
test endpoints and temporary data were removed. Before resuming, use headsets
off-ear, independent low-level synthetic markers, per-slot RTP checks and peak
metering. T4.17 registration loss/recovery was not run in this session.

## T4.13–T4.15 live: hold, displayed identity and distinctive ring

2026-11-26, Italy locale 10, AArch64 baresip 3.16.0 and a temporary LAN SIP
endpoint with RTP echo. For this setup the operator identified **PHONE1 / line
0 as the display handset** and **PHONE2 / line 1 as the analog ringer**. Do not
use the melody on the display handset to infer a board cadence.

| Gate | Live evidence | Result |
|---|---|---|
| T4.13 | One established call, first R gave recall dial tone and SIP `sendonly`; second R restored `sendrecv` and fresh echo. The remote endpoint then offered `inactive`, resumed on a new UDP port with PCMU/PT 0 instead of PCMA/PT 8, and received matching 200/SDP. Both router encoder and decoder changed to PCMU; board RTP_UPDATE (11) acknowledged and fresh speech returned without redial. | Pass |
| T4.14 | With conflicting PAI/RPID/From, slot 0 received the 15-byte PAI number, PAI name and eight-byte date/time. Operator confirmed the number on the display but no date/time; ordinary caller name not explicitly confirmed. Privacy produced blank name and `P`, reported as private on the handset. For waiting CID, `CID_PLAY` followed by selector 31 both had status zero; operator saw **Internal only**, not the number, and heard a very loud signaling burst plus the waiting tone. Type-II remains opt-in/default off. Unavailable `O` has fixture evidence only. | Partial; no further at-ear type-II replay. Known-capable receiver/decoder needed for Q8. |
| T4.15 | On analog PHONE2, default entry 0 vs `Alert-Info` parameter `info=internal` → entry 2 produced clearly distinct rings. Both stopped: plain CANCEL and CANCEL with SIP cause 200/completed elsewhere each got 200 CANCEL / 487 INVITE and a status-zero RING_STOP. Earlier answered calls also stopped ringing. | Pass |

No audio recording. Temporary endpoint, router runtime and control trace removed.

## T4.11 and T4.12 live: early media and off-hook warning

2026-11-26. PHONE2, stock peer, Italy locale 10, PCMA to Asterisk 22.5.2.
**T4.11 pass on combined evidence:** extension 680 sent 180 without SDP;
operator heard ordinary local ringback. Extension 683 sent 183 with SDP;
operator heard only the PBX's 800 Hz, 200/200 ms pulses before answer.
Both switched to returned speech after 200 OK. Dial, busy, congestion, SIT,
waiting and disconnect had earlier live results; warning heard below.

**T4.12 pass:** used accelerated 6 s first-digit, 4 s post-disconnect and 3 s
warning timers. No-digit lift to warning RTP activation took 6.007 s; after
3.003 s the module deactivated media, stopped tone and closed slot 1 before
hook-on. A key during lockout did not restart dialling. Operator confirmed
fresh dial tone after replacing and lifting the handset. On extension 684 a
deliberate one-second 650 Hz **answered-call** marker preceded PBX hangup;
it was not part of the disconnect sequence. After release the selector-1 busy
tone lasted 4.003–4.004 s, followed by 3.002–3.003 s of warning and then
silence/slot close. Two full cycles and one early on-hook during busy were
observed. No active PBX channels remained. Temporary services, control traces,
test configuration and signaling logs removed. No new T4.10 ear labels claimed.

## T4.10 dual-port selector sweep

Italy profile 10: ran all 27 documented selectors on both off-hook ports independently, with stop/close after nonzero selectors. Reran PHONE2 on Scarlett CH2 after a known-tone reference (428 Hz, 500/500 ms). All CLI commands exited successfully. Aggregated earpiece frequency/cadence results are in the [peer selector map](peer.md). The earlier absolute RMS threshold missed known low-level tones; a per-frequency post-stop background reference recovered the dial, busy, ringback-like and stutter cadences. The sustained-tone detector does not rule out short transients. All 54 port/selector combinations have been exercised; semantic ear labels are partial, particularly 3/16/17. No group listening response was counted as an individual identification. No raw audio retained.

## T4.8 live: board-leg G.722 bandwidth evaluation

2026-11-26, stock Econet peer, Italy profile 10, both handsets off-hook. The
Scarlett left headphone driver excited one mouthpiece; CH1 measured the other
earpiece, then the fixtures were swapped. Direct source check at the headphone:
1/3/5/6 kHz detected at −56/−53/−58/−70 dBFS, no clipping. Each direction
used the same source level and placement across PCMA then G.722. No CH2 timing
reference is required for this spectral comparison. Each test frequency played
twice. Values below are the 95th percentile of the Hann-windowed FFT-bin
amplitude in 50 ms frames (dBFS, rounded), **not** calibrated acoustic SPL.

| Excited mouth → measured ear | Board codec | Earpiece 1 / 3 / 5 / 6 kHz | RTP packets forwarded each way in 10 s |
|---|---|---|---:|
| PHONE1 → PHONE2 | PCMA/PT 8 | −48 / −31 / −89 / ≤−90 | 500 |
| PHONE1 → PHONE2 | G.722/PT 9 | −56 / −37 / ≤−90 / ≤−90 | 499 |
| PHONE2 → PHONE1 | PCMA/PT 8 | −54 / −42 / ≤−90 / ≤−90 | 500 |
| PHONE2 → PHONE1 | G.722/PT 9 | −60 / −48 / ≤−90 / ≤−90 | 499 |

Separately decoded the excited port's PT 9 board uplink at 16 kHz: 1/3 kHz
at −29/−31 dBFS (PHONE1) and −31/−35 (PHONE2), but 5/6 kHz at the analysis
floor (−90.3 dBFS) for both. Both slots activated, deactivated and closed with
status zero in every run. Raw audio and packet captures were not saved.

**Evaluation decision:** G.722 board framing and narrowband audio work;
no usable upper-band benefit demonstrated on these handsets. Keep PCMA as
the default board leg. This does not isolate the limiting element among the
mouthpiece, analog path and DSP, or rule out a different handset. SIP-side
G.722 remains independently verified by T4.7.

## M4 live: mandatory SRTP and network G.722

2026-11-26, PHONE2/slot 1, stock Econet peer, AArch64 baresip 3.16.0,
LAN Asterisk 22.5.2. Two separate answering-extension calls; operator heard
their spoken phrase returned and hung up each time.

| Gate | Evidence | Result |
|---|---|---|
| T4.6 | Router account `mediaenc=srtp-mand`; PBX `media_encryption=sdes`, optimistic fallback disabled. Both SDP offer and answer used `RTP/SAVP`, PCMA/PT 8, `AES_CM_128_HMAC_SHA1_80`. Call established for 26 s with returned speech. | Pass |
| T4.7 | Separate G.722-only PBX account; offer and answer used PT 9. Router encoder and decoder both reported G722, 16 kHz mono, with `auresamp` on both paths. Board-facing slot-1 DD03 RTP/PT 8: 827 master→peer and 828 peer→master packets, zero kernel drops. Call established for 17 s with returned speech. | Pass |

SDP inspection retained only profile/PT and cipher suite, not inline keys.
Board monitoring counted matching frame headers only. No audio file or raw
capture was saved. Both calls ended; temporary PBX, credentials and router
runtime removed. T4.7 does **not** establish wideband analog response (T4.8).

## M4 live: in-band digits, progress tones, dial map and panel

2026-11-26, VX830v/stock Econet, locale 10, two registered PCMA accounts
and Asterisk 22.5.2. One router clock for DD02 key release and first LAN INVITE.
No call audio retained. Temporary accounts, PBX, traces, hotplug symlinks and
router runtime removed. Both panel devices returned to their initial off/none.

| Gate | Live evidence | Result |
|---|---|---|
| T4.1 | Both board legs set to in-band, PBX endpoints `dtmf_mode=inband`. With both calls up, Asterisk detected each port's four requested keys once in order, including `*`. The initial PHONE2 attempt produced only three board releases and three detected digits; a distinct fourth press supplied the missing `*`, not a duplicate. PHONE2 inbound answer/echo rechecked. Earlier live runs cover RFC 4733, INFO and BYE/cleanup. | Pass on combined evidence; both-port in-band gap closed. |
| T4.4 | PHONE2: genuine final responses 486 Busy Here → selector 1 regular busy, 404 Not Found → generated three-pitch SIT, 503 Service Unavailable → selector 2 fast busy. Operator distinguished all three and each on-hook cleared the indication. | Pass for the three named categories; other mapped status codes covered by host tests, not all replayed live. No-response/transport-loss bench case outstanding. |
| T4.5 | Exact `600` sent the first INVITE within 1 ms of the last key release on both ports. Ambiguous `603` sent after 3.005 s, explicit `#` sent in 0.81 ms, incomplete `60` gave local unobtainable after 6.006 s with no INVITE. Confirmed cancellation by hanging up ~1.18 s after entering `603`; no subsequent INVITE. | Pass. |
| T4.23 | Master OpenWrt FXS hotplug: idle registered solid green, ringing 200/200 ms green, active 500/500 ms green. Operator confirmed active indication persisted after the first of two calls ended and returned to idle only after the last. No registered mapped UA while idle → steady red (visual and sysfs); shutdown → both off (sysfs). | Pass. No peer LED command or GPIO changes. |

## M4 autonomous verification pass

2026-11-26, working tree on `d4ec01f`. **31/31 host module CTests**, including
the optional real baresip 3.16 SDES, G.722/resampler and MWI scenarios. **20/20
CLI emulator scenarios** (one Cargo integration test); `cargo fmt --check` and
CLI clippy with warnings denied passed. Built the router module and release
CLI for AArch64 musl, plus target `srtp`, `mwi`, `auresamp` and `g722` modules
in scratch staging (G.722 with SpanDSP 0.0.6 and TIFF 4.7.1). No new hardware
test, installed package, or handset observation in this pass.

The harness now checks frequency pairs and release for in-band keys at the
opposite SIP endpoint, immediate/short/long digit-map decisions, first-digit
and disconnect warning/lockout, 12 final SIP failure codes, 180 local ringback
versus 183 remote RTP, solo flash hold/resume, shared-account ringing and hunt,
identity precedence/privacy, mapped ring cadence and CANCEL, type-II CID request,
MWI set/clear and registration recovery, signed gain words, and event- and
2100-Hz-triggered fax profile updates. CLI checks include G.722 payload forwarding
and port LED command layout. This evidence does not measure physical gain in dB,
SLIC wideband response, display behaviour, fax transfer, visual LEDs, or long-call
drift. See the [M4 bench queue](m4-bench.md) for per-gate dependencies and limits.

## T4.3 live: PHONE2 call waiting, R-key swap and recall

**Pass, 2026-11-26.** On the stock peer (Italy, line 1), Asterisk answered
call A with Echo and originated B to that same busy SIP account. The operator
heard the short waiting beep and verified A still echoed. One R press held A
and answered B (distinct repeating tone); both PBX channels were Up. A second
R held B and restored fresh speech on the original A. Router hold/resume logs
and PBX SDP showed `sendonly` for hold and `sendrecv` for resume. Replacing
PHONE2 released A and physically re-rang the handset for held B; lifting
resumed B's tone. Final hangup released B. No PBX channels remained. No lamp
or display indication was used. Temporary test services and logs removed.

## T4.3 call waiting: host preflight

15/15 module CTests passed. Two caller accounts dialled the same occupied
fixture line: the wait beep was present on its playout, A continued to carry
fresh bidirectional payloads, and B did not trigger a second physical ring.
DD02 hook-flash (19) answered B and held A; a second flash restored A. New
payload signatures verified both changes, including absence of handset samples
on the held caller. Replacing the receiver with B held rang the same port;
lifting resumed B. The AArch64 module built. No lamp or CID display is needed.
Live beep audibility, physical R-key switching and PBX hold SDP remain to verify.

## T4.2 live: two simultaneous independent calls

**Pass, 2026-11-26.** One AArch64 router process registered distinct accounts
701 (slot 0) and 702 (slot 1) to LAN Asterisk. Both physical handsets reached
independent calls to the same PCMA echo extension. The operator heard each
handset's own speech; Asterisk showed both PJSIP channels Up simultaneously.
After hanging up the first call (702/slot 1), the original 701 channel remained
Up and returned a fresh phrase without redialling. After final on-hook neither
channel remained. Slot 1 sent 9494 packets, slot 0 sent 8844; neither player
missed a timer period. The operator's first handset was observed on slot 1;
account/slot identities above come from the logs, not the physical labels.
An earlier operator window expired before any call and is excluded. Temporary
PBX, credentials and router files removed.

14/14 host CTests passed. The dual fixture drives two accounts per UA and two
slots per board transport, checks distinct payloads in all four directions and
requires new payloads on the surviving dialog after hangup. This run is not
the ten-minute stability and conversational echo check (T4.20).

## T4.9 live: remote disconnect on both ports

2026-11-26, locale 10 (Italy). Asterisk extension 604 answered, played a short
confirmation tone, then hung up while the operator held the receiver. **Pass on
both physical ports with the other line idle.** PHONE2 completed two calls and
PHONE1 three. On both, the operator heard the repeating busy tone after remote
release, then fresh dial tone after replacing and lifting the receiver, and
successfully redialled. PHONE2 stayed quiet during the PHONE1 test.

Each call had status-zero replies to `RTP_DEACTIVE(slot) → TONE_PLAY(slot,1)`
and then `TONE_PLAY(slot,0) → CHANNEL_CLOSE(slot)` on handset replacement.
No tone commands targeted the opposite slot. The PBX ended with zero active
calls/channels. Ports were selected sequentially, with a module restart only
between port batches; continuity of an active second call remains T4.2. The
temporary router runtime, PBX instance, credentials and control traces were removed.

## M4 start: disconnect feedback and tone characterization

T4.10 partial: warm init at locale 10, four PHONE2 selectors measured, selector
1 independently heard as normal busy and stopped. [Measurements](peer.md)
are in the selector lookup section of peer.md. No audio files saved.
T4.9 host: remote established-call close stops media and requests selector 1
on the still-open local channel. On-hook stops the tone and closes the slot;
next off-hook opens it with dial tone. Both call directions tested in the
two-endpoint fixture; 13/13 module CTests passed. CLI fixture 18/18 scenarios,
including selector 19 on line 1; Rust clippy clean; both target artifacts built.
The selector listening check was a CLI command, not an end-to-end hangup test.
The subsequent live remote-release run on both ports is recorded above.

## M3 review follow-up (host)

13/13 module CTests passed, including rejection of a runtime tone with a successful subsequent call,
one dropped synchronous channel-open send followed by redial, timeout followed by warm re-attach in
the same process, and opt-in board-clock ringback/media with duplicate suppression. Host and AArch64
module builds used GNU C11 and `-Wall -Wextra -Wshadow`. FFI clippy, C smoke and generated-header
comparison passed. These results are host evidence for the review changes, not a new hardware run.

## M3 live: PHONE2 stability and latency

### Playout pacing and wired-LAN jitter-buffer comparison

Repeated the 60-chirp PHONE2 measurement after anchoring the board player to a
monotonic 20 ms schedule. Both runs used the same module, PC sender, Scarlett
reference and 20 ms PCMA frames. Matched the 60 successive chirp onsets on the
router between LAN ingress and slot-1 DD03 egress during each acoustic run.
No audio file was saved.

| baresip RTP jitter range (packets) | Router ingress→DD03 min / median / max | Above 150 ms | Earpiece minus CH2 median / max |
|---|---|---|---|
| Default 5–10 | 118.7 / 155.6 / 188.8 ms | 34/60 | 269.9 / 289.4 ms |
| Wired-LAN trial 2–8 | 54.4 / 91.4 / 128.0 ms | 0/60 | 204.4 / 244.3 ms |

Both calls: 60/60 acoustic matches, no clipped samples, zero kernel capture
drops, no missed playout periods, maximum callback lateness 1 ms. The second
run's router delay still rose from 58.4 to 128.0 ms. This comparison isolates
the change in buffer profile with the pacing change in both builds. The
downlink met the master budget over this minute on the wired LAN; sustained
delay growth, uplink latency and conversational echo remain open. Internal
queue occupancy was not sampled. Compare longer-duration source/board clock
rates and queue depth before generalizing.

### Acoustic follow-up: PHONE2 earpiece, Scarlett Solo

2026-11-26: a temporary PC SIP endpoint answered PHONE2 and sent PCMA/PT 8,
20 ms frames made from the PipeWire left output monitor. One 100 ms chirp
(650–2400 Hz) per second for 60 s. Scarlett CH1 mic at the earpiece; headphone
output into balanced mono CH2 as an electrical reference; Direct Monitor off.
Both inputs and the digital source were captured together at 48 kHz and matched
at 8 kHz. Operator confirmed chirps from the earpiece. All 60 matched, no
clipped samples, no audio file saved. In-call CH2 lagged the digital monitor
by 33.375–33.750 ms (median 33.625 ms).

| Difference in arrival time | First | Median | Max | Last |
|---|---:|---:|---:|---:|
| Earpiece mic minus CH2 electrical reference | 157.1 ms | 296.9 ms | 357.3 ms | 316.8 ms |
| Earpiece mic minus digital source | 190.9 ms | 330.5 ms | 390.6 ms | 350.5 ms |

The acoustic figures include the PC injection path, LAN, router, board and
handset; they do not isolate master-only delay. Delay often changed in units
of one 20 ms packet. No simultaneous router timestamps or conversational echo
check in this run. T3.6 remains open on the prior master-only measurement.

2026-11-26, stock peer, line/slot 1, warm Italy profile 10, PCMA/20 ms.

**T3.7 — pass.** The same router baresip process (PID 5915) completed 31
outgoing calls to an Asterisk answer/tone/echo extension. Both ends counted 31;
the PBX ended with zero active calls and channels. DD01 header monitoring found
32 acknowledged opens/closes on slot 1, 31 acknowledged activations and 31
deactivations. The extra open/close without media was not counted as a call;
the operator reported dial mistakes. 442 control frames, zero kernel drops.
No monotonic growth in measured idle RSS or descriptors. Slot ACKs substitute
for the not-yet-implemented M5 ubus status interface.

| Completed calls | Idle fds | Idle sockets | VmRSS (KiB) |
|---:|---:|---:|---:|
| 0 | 12 | 6 | 6552 |
| 5 | 12 | 6 | 6608 |
| 17 | 12 | 6 | 6584 |
| 31 | 12 | 6 | 6584 |

**T3.6 — current build misses the latency target.** On a separate 40-second
PBX tone-only call (no Echo application), matched 40 active-tone onsets of an
alternating 300/500 ms tone pattern. Compared first above-threshold 20 ms PCMA
frames from LAN RTP ingress with slot-1 DD03 RTP egress using the router's own
packet timestamps. Master downlink delay rose from 105.1 to 237.9 ms, median
165.7 ms; 26/40 onsets exceeded 150 ms. This isolates a master segment rather
than estimating one-way latency by halving a round trip. It excludes analog
and uplink delay. No audio retained. Operator heard the pattern clearly and no
noticeable delay on that call, but noted a large voice delay in the earlier
deliberate Echo test. This is not a two-person echo assessment. Investigate
20 ms playout scheduling and buffer growth, then repeat and check real speech.

## M3 live calls: PHONE2 and Asterisk

2026-11-26. One physical line (PHONE2/slot 1), Italy profile 10, router baresip
3.16.0, LAN Asterisk 22.5.2, PCMA/8000 mono. The PBX used a synthetic account
and an Echo application, not a recording. Temporary services and logs removed.

| Gate | Evidence | Result |
|---|---|---|
| T3.2 outgoing | Handset dialled `600#`; baresip connected to extension 600, received 180 Ringing, then answer. Operator heard dial tone, ringback and their own speech returned through the PBX. | Pass |
| T3.3 incoming | PBX originated a call, PHONE2 rang and displayed the expected synthetic number. Lifting answered; operator confirmed speech returned via the PBX. | Pass |
| T3.4 release | Handset hangup and explicit PBX channel hangup both terminated established calls. After remote hangup the same receiver redialled successfully without restarting baresip. On redials: router-originated BYE and status-zero DD01 acknowledgements for RTP_DEACTIVE and CHANNEL_CLOSE on slot 1; Asterisk ended at zero active channels. Remote release was observed via PBX/UA state and slot reuse, not a separate remote-BYE capture. | Pass |
| T3.5 in-call DTMF | RFC 4733 on outbound, SIP INFO on inbound. For each mode Asterisk logged the four requested test keys once each and in order on the PJSIP channel. | Pass |

**Follow-up:** after PBX hangup the off-hook receiver was silent; it currently has
no disconnect indication. This did not prevent release or redial. The tone is tracked in [M4, test T4.9](../../plans/voip-baresip-plan.md#m4--two-lines-and-complete-ata-feature-set). Baresip also
reported jitter-buffer drops around echoed DTMF. T3.6 needs a calibrated latency
measurement and normal (non-Echo) listening. T3.7's 30-call run was deferred.

## M3 call integration (host)

2026-11-26: **9/9 CTests passed**, including two calls between separate baresip
3.16.0 processes with Unix-datagram board fixtures. Caller fixture on line 0
lifted, received channel open and dial-tone selector 4, and dialled `202#`.
The called extension on line 1 received CID number `101` with selection mask 2,
and ring start; the caller received nonconstant ringback audio. Answer by hook
event stopped ringing, and each fixture received the other's distinct 160-byte
PCMA payload over the SIP/RTP bridge. Caller-initiated and callee-initiated
hangup each released both slots. The non-fixture AArch64 module also built.
No physical/PBX call or acoustic check in this run. [Live procedure](../../rbctl-feed/rbctl-voip/modules/rbctl_voip/README.md#live-call-checks-t32t34).

## M3 live: T3.1 board ready and PBX registered

**Pass, 2026-11-25.** The AArch64 OpenWrt master ran baresip/libre 3.16.0 and
the non-fixture `rbctl_voip.so` from a private temporary directory. The module
attached to the stock Econet peer over `lan0.500` (`DSP running`) and logged
`rbctl_voip: board ready (warm, locale 10)`. In the same 10-second run, a
synthetic SIP account received `200 OK` over UDP from a LAN Asterisk 22.5.2
PBX with one binding. Asterisk's `pjsip show contacts` independently showed
the router contact before baresip shut down normally. This covers warm startup
and registration, not a call or an on-target cold boot.

Built with the OpenWrt Cortex-A53 GCC 14.4 musl toolchain and the target Rust
FFI archive. No package or service was installed. The temporary PBX was stopped
and the temporary router runtime/configuration removed; no handset actions or
packet captures. SDK packaging remains T5.1.

## M3 host preflight

**Current startup implementation: 7/7 host CTests passed.** Real baresip/libre
3.16.0 registered with a loopback registrar while a Unix datagram fixture checked
the ordered DSP commands and event acknowledgements. Cold (locale 10): SLIC
completion, DSP defaults, fresh DSP start completion and four slot defaults.
Warm (locale 7): RTP deactivate and channel close on all four slots, SLIC/DSP
defaults and four slot defaults, with no DSP start. Both reported board ready.
Missing attach state, missing SLIC completion, missing fresh DSP start completion,
and rejection of the final slot default each prevented readiness while SIP still
registered. The seventh test checks board-link ownership and transport errors.
Target/PBX verification is recorded in the live T3.1 result above.

**Historical scaffold run:**

2026-11-25, Linux x86-64: **2/2 CTests passed** with upstream baresip/libre
3.16.0 and the Rust FFI static archive. `board_lifecycle` exercised bind, duplicate
bind, a failed DD01 send through the real link and callback, double close and
rebind. `baresip_registration_smoke` ran the actual baresip executable: a synthetic
account registered with a loopback UDP registrar (200 OK, one binding), while
the loader separately reported `ENOSYS` for `rbctl_voip.so`, as expected from
the scaffold. Baresip's own “ready” message does not imply board readiness.
No handset, router media or provider credentials were used in that scaffold run.

## T4.21 — rotary dialling (early evaluation)

2026-11-26, PHONE2 (line 1), operator's rotary handset, locale 10. Temporary `rbctl-voicectl` build (SHA-256 `3119b49a…4a18`, same protocol code as T2.16) in router tmpfs; DD02-only tcpdump; all files removed afterwards. Digit values were decoded from the capture (the CLI does not print them).

| Run | Master | Observed DD02 | Note |
|---|---|---|---|
| 1 | Monitor had exited; events unacknowledged | HOOK_OFF, three DTMF_START/END pairs all with value 35 (`#`), HOOK_ON; each frame sent 3× | Queue stalled without ACKs. Values do not correspond to the dialled digits; origin unexplained, not counted. |
| 2 | Acknowledging monitor | HOOK_OFF, HOOK_ON 20.5 s later; no other event | Operator dialled 1–0 |
| 3 | Acknowledging monitor | HOOK_OFF, HOOK_ON 7.3 s later; no other event | Operator dialled 5, 0 |

**Result: unsupported by the stock peer.** Pulse breaks produce neither digit nor hook/flash events. The peer never sets the driver's `pulseEnable` and has no DD01 field for it ([peer.md](peer.md#ata-feature-reachability-m4-evaluation-basis)); the master cannot decode pulses from debounced hook state.

## M2.1 closure

2026-11-25. **T2.17 passed** on the stock Econet peer via `lan0.500`, with the same Rust CLI SHA-256 as T2.16. Operator confirmed the handset observations. M2.1 closed with the operator-approved T2.16 exception: the tested CLI uses dynamic musl; the full OpenWrt SDK package build remains a T5.1 gate.

| Check | Observation |
|---|---|
| Cold start | Shared switch power-cycled both boards. Locale 10; `1a`/`1b` status 0, stopped state 1, SLIC result 10, fresh DSP start result 0, four channel defaults acknowledged. Ready in 126 ms. |
| Both phones | Rust event monitor observed OFF/ON on line 0, then line 1. Keypad verifier passed 12 press/release pairs on each. Operator heard six-second dial tone followed by silence on each line and intelligible speech both ways on the repeated default-profile loop. |
| Media | Timed 20 s default run: RTP 1000/999, RTCP 4/4. Separate listening repeat: RTP 895/895. Five-second direction controls: A = 0/0 RTP (one RTCP; expected no-data exit after teardown), B = 250/250 RTP. |
| Capture | `voip_trace.py -v`: 4008/4008 captured frames classified as voice and CRC-valid; tcpdump reported zero kernel drops. Raw capture deleted. |
| Warm recovery | Three inits ready in 55, 55, 57 ms; third after killing an active two-slot bridge. All four slots reclaimed and defaulted without another DSP start; both slots subsequently reused. Additional owner-death check: 59 ms, then zero DD03 frames in a three-second idle window. |

Retired the C/CMake feed after parity. The Rust workspace's `scripts/check.sh` passed again without it, including the retained golden vectors, FFI smoke/header check, and QEMU differential (19 comparisons / 2528 bytes). Long-running fuzz results are from the earlier host run below.

## M2.1 — Rust port: host checks

2026-11-25. Workspace `rbctl-feed/rbctl-voip` (uncommitted), base `c96803a` (af_packet MIT relicence `becbc9e`, `pkt_type` fix `c96803a`). Host x86_64, Rust 1.97.1; nightly for fuzzing. No hardware in this run.

| Test | Command | Result |
|---|---|---|
| T2.10 | `cargo build --workspace --locked && cargo test --workspace --features rbctl_voicectl/fixture` (via `scripts/check.sh`) | **Pass.** 8 unit, 1 golden, 4 proptest (4096 cases each), 1 CLI (17 scenarios), 2 FFI tests. |
| T2.11 | `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings` with default and all features; `cargo deny check licenses bans sources` | **Pass.** No findings. Graph licences: MIT, Apache-2.0, Unicode-3.0, GPL-2.0-or-later; `af_packet` MIT. |
| T2.12 | `crates/rbctl_voip_proto/tests/golden.rs` against `tests/vectors/c_prototype.txt` (SHA-256 `4ffee044fe05eb896355ea24bcee57bd33850aad8ea9c0f2bca9ecf0471bfd7e`) | **Pass.** 163/163 vectors byte-identical: semantic records, every command × direction normalization, `1a`/retry/`1b`/`16` frames, peer status, ACK ok/duplicate/short/full, DD03 broadcast and learned. Vectors generated by `tests/vectors/gen_c.c` linked to the C prototype. |
| T2.13 | `python3 -B tests/qemu/diff.py target/debug/examples/oracle_vectors` | **Pass.** 19 records/frames, 2528 bytes vs OEM AArch64 routines (final ring word replayed, as T1.5). |
| T2.14 | proptest (above) + `cargo +nightly fuzz run <t> -- -max_total_time=600` | **Pass.** `frame` 166,270,061 execs, `record` 111,058,506, `link` 64,061,381; no crash/panic. `frame_valid` (CRC repaired, deeper paths): see below. |
| T2.15 | `crates/rbctl_voip_ffi/tests/c_smoke.rs` | **Pass.** `tests/ffi/smoke.c` built with `-fsanitize=address,undefined` against `rbctl_voip.h` + staticlib: init/retry/reply, record codec, peer status, event ACK/dedupe/pop, media view and send, error codes. `cbindgen` output equals committed header. |
| T2.16 | Replay of `Build/Prepare` into a scratch tree, then `cargo build --locked --release` for `aarch64-unknown-linux-musl` with the OpenWrt GCC 14.4 musl linker, `-crt-static` off, `panic=abort` | **Partial.** `rbctl-voicectl` 529,144 bytes, stripped, dynamically linked (`libc.so`, `libgcc_s.so.1`; OpenWrt rust-package model, not static as the C CLI), SHA-256 `abcc62e26f2b3143bc1b6a8316309558c2b53ab325f00edd517de5b016da8f65`; runs `--help` under `qemu-aarch64 -L <toolchain>`. `librbctl_voip_ffi.a` 23 MB (unstripped archive; linked size is what matters in M3). The OpenWrt Rust host toolchain is not built in the local tree, so `make package/rbctl-voip/compile` is deferred to T5.1 (as T1.13). |
| T2.17 | Hardware parity | **Pass** in the subsequent [live run](#m21-closure). |
| T2.18 | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` | **Pass.** `#![deny(missing_docs)]` in proto, net and FFI crates. |

**Fuzz, CRC-repaired frames (`frame_valid`):** 77,678,581 executions in 600 s, exit 0, no findings. Coverage plateaued at 60 edges: random destination MACs rarely match the link's local address, so this target mainly exercises the post-CRC decode paths; link dispatch is covered by `link` and the unit tests.

**T2.17 instructions for the operator** (copy the aarch64 `rbctl-voicectl` to the router; record each result here):
1. `rbctl-voicectl init --locale 10` on a cold board (T2.1), then `rbctl-voicectl events` and lift/replace both handsets (T2.2).
2. `rbctl-voicectl keypad` — 12 keys on each line (T2.3).
3. `rbctl-voicectl tone --line 0` **and** `--line 1` — dial tone ~6 s then silence (T2.5, both lines).
4. Both handsets off-hook: `rbctl-voicectl loop --line 0 --to 1 --duration-ms 20000` — two-way audio, ~50 RTP/s each way (T2.6); repeat with `--profile A` (expect 0 RTP) and `--profile B` (expect RTP).
5. During step 4 capture `tcpdump -p -i lan0.500 -s 0 -w /tmp/m21.pcap 'ether proto 0xdd01 or ether proto 0xdd02 or ether proto 0xdd03'`, then `python3 examples/voip_trace.py -v m21.pcap` offline (T2.7). Delete the raw capture afterwards.
6. Three warm restarts: `rbctl-voicectl init` ×3, the last one after killing a running `loop` (T2.8).

## M2 closure

**2026-11-25 — PASS, T2.1–T2.8, against the stock Econet peer.** Uncommitted working tree on `5670764`. Closure uses the recorded live runs below; the entire hardware sequence was not repeated today. T2.4 passes its written criterion: the phone rang, the CID-capable handset displayed the synthetic caller **number**, and ringing stopped. Both ports were heard to ring and stop. Caller **name** is unconfirmed; **date/time** did not display. Keep these as v1 follow-ups with a known-capable receiver, not as positive M2 findings. No change to the acceptance criteria.

| Passed | Hardware evidence |
|---|---|
| T2.1–T2.3 | Ready in 115 ms; lift/replacement and all twelve ASCII key pairs verified on both ports. |
| T2.4–T2.5 | Ring/CID number/stop as above; dial tone started and stopped on lifted PHONE1. |
| T2.6–T2.7 | Two-way audibility confirmed across the working-profile runs. All 1,232 captured voice frames classified, all CRCs valid. |
| T2.8 | 10/10 warm restarts ready in 51–58 ms, including reclaim and reuse of previously active slots. |

**Closure regressions:** `cmake --build build -j4 && ctest --test-dir build --output-on-failure` and the same commands for `build-san`: **4/4 each**, including the CLI fixture, packet read, unit tests and QEMU differential. ASan/UBSan reported no findings.

**Live field check:** both handsets off-hook, no keys pressed, no capture or audio retained. Built the normal CLI with `tests/live/media-profile.c` linked using `-Wl,--wrap=vx_dsp_encode` against the AArch64 static library. Only the activation tuple is overridden (`VX_TEST_PROFILE=A|B`); no changes to the production default. Each run started a fresh process, opened both slots and obtained both RTP_OFF and CHANNEL_CLOSE ACKs at teardown. Counts below are forwarded datagrams, not samples or judgments of speech quality.

| Run | Direction | DTMF mode, TX/RX event PT | Seconds | RTP 0→1 / 1→0 | RTCP 0→1 / 1→0 |
|---|---:|---|---:|---|---|
| A1 | 0 | 0, 0/0 | 5 | 0 / 0 | 0 / 0 |
| B1 | 3 | 0, 0/0 | 20 | 999 / 999 | 4 / 5 |
| A2 | 0 | 0, 0/0 | 5 | 0 / 0 | 0 / 1 |
| B2 | 3 | 0, 0/0 | 20 | 999 / 999 | 4 / 5 |

Both A runs exited nonzero as expected for no RTP; both B runs exited 0. Direction was the only changed activation word; the longer B windows allowed a listening check. The operator reported the acoustic comparison as unclear. The packet result is repeatable; the independent T2.6 speech result still applies to the working profile with mode 1 and PTs 101/101.

## M2 live — 2026-11-25: T2.8 master restart pass

Base `5670764` + WT, stock peer kept powered, `lan0.500`, Italy locale 10. **Before fix:** warm attach reported DSP state 0; SLIC returned 10; repeated DSP_START received a zero DD01 ACK but no DD02 completion by the 5000 ms deadline. Not a successful start.

**After fix:** 10 independent `voicectl init --locale 10` processes reached ready in **55, 51, 53, 57, 56, 57, 56, 57, 57, 58 ms**. Each matched `1a/1b`, observed running state 0 at attach, reclaimed all four slots, received a fresh SLIC event 1 (value 10), set DSP defaults, **skipped DSP_START**, and received four channel-default ACKs. This is warm recovery, not ten DSP restarts. Offline header checks found ten distinct initial sequences; all 16 DD01 request/reply subtype/sequence pairs per run matched. A deliberately reused sequence was not tested live.

**Active-slot case (run 10):** started the two-slot PCMA loop, then killed only that master CLI before teardown. In two-second inbound DD03 observations, tcpdump reported **196 packets received by filter** after owner death and **0** after warm recovery (zero kernel drops). A subsequent two-second loop reused both slots: 99 RTP forwarded per direction and 3 RTCP total, followed by acknowledged deactivation/close. Packet flow, not another acoustic check. No audio saved. Capture references `T2.8-idle-headers` and `T2.8-recovery-headers` were DD01-only, 24-byte snapshots in tmpfs, removed after validation. The first active-case setup was interrupted by BusyBox `sleep` rejecting a fractional interval; it was not counted as an init run.

Host `cli_startup` passed including stopped/cold attach, missing fresh cold completion, warm recovery with no START, and rejected cleanup. Verified deployed SHA-256 `c9670dc8c2e7436e07054a593bf3385c3cea3ba5f9878fdd94554ae417ac7384`. No board reboot, service installation or physical-interface change.

## M2 live — 2026-11-25: T2.7 frame classification and CRC pass

Uncommitted working tree on `5670764`. Capture reference `T2.7-idle-loop-6s` (ephemeral; raw files deleted). Both handsets remained on-hook; no dialing or CID. Used the same verified `voicectl-long` as the successful T2.6 run, with a 6000 ms loop and unchanged media/locale settings. Captured before attach through after teardown on `lan0.500`: `tcpdump -p -i lan0.500 -s 0 -U -w m2.pcap 'ether proto 0xdd01 or ether proto 0xdd02 or ether proto 0xdd03'`. Tcpdump reported 1,232 captured, 1,232 received by filter, 0 kernel drops. No pcap committed or retained; both private tmpfs copies removed after offline validation.

`python3 -B examples/voip_trace.py -v m2.pcap`: **1,232 frames, 1,232 voice, 1,232 CRC OK**. Also checked captured vs original lengths, markers, slots/selectors and matched each DD01/DD02 sequence and subtype to a zero-status reply. **No unclassified, truncated, or bad-CRC frame** in this run.

| EtherType | Frames | Classification |
|---|---:|---|
| DD01 | 20 | One `1a`, one `1b`, eight `16` requests, each with a matching reply |
| DD02 | 2 | Attach-state event `18` and its ACK |
| DD03 | 1,210 | `19`: 1,200 RTP (300 per slot per direction); 10 RTCP (3 peer / 2 host per slot) |

The CLI's narrower bridge window saw 604/604 media frames accepted, forwarded 300 RTP and 2 RTCP each way, then acknowledged both deactivations and closes. Packet classification here does not isolate the T2.6 activation fields; see the separate [static direction review and proposed hardware comparison](peer.md#t26-follow-up-media-direction-vs-telephone-event-settings).

## M2 live — 2026-11-25: T2.6 two-way voice pass

Uncommitted tree on `5670764`, OpenWrt master, stock peer, two off-hook handsets. `loop --line 0 --to 1` opened slots 0/1, activated PCMA/PT 8 at 20 ms, and forwarded each incoming DD03 datagram back on the opposite slot with the same RTP/RTCP selector and bytes. No SIP/UDP provider, audio logging, pcap, or locale change. Both slots received deactivate and close ACKs after every live run.

| Profile and duration | Counts (PHONE1→2 / PHONE2→1) | Acoustic result |
|---|---|---|
| Direction 0, DTMF mode/PTs zero, 20 s | 0/0 RTP and RTCP | Neither direction audible despite successful DD01 ACKs. |
| Same profile, 6 s | 0/0 forwarded; independent inbound tcpdump counter saw 2 DD03 frames around the test, 0 kernel drops (24-byte snaplen; no packets saved) | No continuous media stream established. |
| OEM fixture fields: direction 3, DTMF mode 1, TX/RX event PT 101, 12 s | 600/600 RTP, 3/3 RTCP, 1206/1206 DD03 accepted | PHONE1→PHONE2 confirmed, reverse initially uncertain. |
| Same working profile, 45 s | 2250/2250 RTP, 9/9 RTCP, 4518/4518 DD03 accepted in bridge window | Operator clearly heard PHONE2→PHONE1. Both directions now confirmed on the same profile. |

Working rate: 50 RTP packets/s per slot. The profile changed multiple fields together, so this does **not** isolate the cause of the initial silence. This is not a latency, loss-rate or audio-quality measurement. Offline CLI socket-pair regression passed for both profiles tested during development: RTP/RTCP crossed slots intact; rejected second activation still triggered teardown. T2.7 capture analysis and T2.8 restart testing remain separate.

## M2 live — 2026-11-25: T2.5 dial tone on PHONE1

**Pass on PHONE1 (line 0).** Base `5670764` + WT, no capture. The operator lifted the simple analog handset before the test and kept it off-hook without pressing keys. On the running DSP, `tone --line 0 --sel 4 --duration-ms 6000` sent CHANNEL_OPEN for slot 0, command 13 with selector 4, then command 13 with selector 0. All received DD01 status 0. The channel was deliberately left open for another three seconds before CHANNEL_CLOSE (status 0). The operator confirmed dial tone for about six seconds followed by silence while still off-hook. No profile change, SIP dialog or RTP stream; no frequency measurement or second-port tone claim.

Offline `cli_startup` passed with byte checks for open/play/stop/close and standalone selector 0. AArch64 CLI SHA-256 verified on the master: `d8bd6aa979bf809a7ccc639b65660727b14409e48cd763ce8d75f088a22df937`.

## M2 live — 2026-11-25: PHONE1 analog ring cadence

Base `5670764` + WT, no capture. Tested the simple analog handset on PHONE1 (line 0), on-hook throughout. No locale change or CID_PLAY. `ring --line 0 --duration-ms 20000` reattached to state 0; DD01 start and stop both returned status 0. Relative to the first audible onset, the operator reported approximate start/stop times (seconds): **0/1, 5/6, 10/11, 15/16, 20/21**. This supports mode 0's nominal **1 s on, 4 s off, 5 s period** acoustically on PHONE1. The stopwatch and command timer were not synchronized; the last burst is not a measurement of stop latency. PHONE2 extended CID and electrical impedance remain unverified. The bounded CLI window was extended from 10 s to 20 s; the default remains 6 s. Host `cli_startup` passed and the cross-built AArch64 binary's checksum matched on the master.

## M2 live — 2026-11-25: PHONE2 cadence observation

Base `5670764` + WT; no capture. With the CID-capable handset on-hook on PHONE2, ran `ring --line 1 --duration-ms 10000` using the same verified CLI as the Italy test below. The DSP reported attach state 0; ring start and stop each received status 0. No locale or CID_PLAY command was sent. The operator heard a very short initial ring followed by a normal ring, with the whole sequence lasting about ten seconds. No usable burst width or onset-to-onset interval was obtained. This does not verify the nominal 1 s on / 4 s off cadence or establish that on-hook data transmission caused the initial alert.

## M2 live — 2026-11-25: Italian profile request and PHONE2 caller ID

Base `5670764` + WT, no capture. Operator also confirmed PHONE1 stopped on its own in the preceding six-second test. For the Italian CID-capable handset on PHONE2 (line 1), reattached to state 0, requested **DSP_DEFAULT 10** (Italy profile ID 19), then sent CID_PLAY with synthetic number/name and eight digits of master-local `MMDDhhmm`, mode-0 RING_START, and same-line RING_STOP after ten seconds. Each command received a zero transport status. No subsequent profile change was requested. A transport ACK is not SLIC profile readback.

**T2.4:** audible ringing and stop confirmed on both ports; synthetic number displayed on PHONE2. Date/time not set on handset; name not independently confirmed. Operator could not judge cadence timings. Ring table S:40e1f0 entry 0 contains 1000/4000 (nominal ms on/off), independent of locale selection. Neither electrical cadence accuracy nor actual impedance/FSK modulation was measured. Open: extended CID and timed cadence; use a receiver with known field support or instrumented line observation. Offline `cli_startup` passed including profile-before-CID ordering and CID fields; verified AArch64 SHA-256 `09576b82e5e8f951909f381fd67cb319bcb9e0268e9e776c0b74195c5bb325a2` before deployment.

## M2 live — 2026-11-25: audible ring on PHONE1 (T2.4 partial)

Uncommitted working tree on `5670764`; no capture. Host `cli_startup` passed including a six-word ring record with zero final word, explicit same-line stop, and stop after exhausted start retries. Deployed AArch64 CLI SHA-256 `e2be9d5124c469da8d0ed30e894dd2390cb59df794caac7e708cdbfa75414057`. With the handset on-hook on PHONE1, `ring --line 0 --duration-ms 6000` reattached to state 0 and received zero statuses for both start and stop. Operator confirmed audible ringing; cessation without lifting was not explicitly confirmed. No CID_PLAY was sent. PHONE2 ring/CID deferred by operator for profile investigation. The locale last requested at initialization was 0; the test did not read back or change electrical settings. See [driver evidence](peer.md#electrical-profile-cross-check-ring-investigation).

## M2 live — 2026-11-25: physical FLASH key

Base `5670764` + working tree; no capture. `voicectl monitor` reattached to the initialized DSP (attach state 0); no SLIC/DSP reconfiguration. With one analog handset and one FLASH press per lift, the operator confirmed **PHONE2 first, then PHONE1**. DD02 decoded as `HOOK_OFF → HOOK_FLASH → HOOK_ON` on line 1, then the same sequence on line 0. Each press produced one command 19, not an on-hook/off-hook pair. Monitor stopped. This verifies recognition with this handset/profile, not the flash timing boundaries or SIP call swapping (T4.3). T2.4 caller-ID display verification is deferred until a CID-capable device is available.

Offline `cli_startup` passed including attach-only event monitoring and flash on both lines. Deployed AArch64 monitor SHA-256: `492c84b04a6dc12ea01e9740ae820b3201e3291b08648f0aa274baee519ab40f`.

## M2 live — 2026-11-25: T2.3 pass

Base `5670764` + working tree; no capture. Ran `vx830v-voicectl keypad` on the same OpenWrt master. The tool reattached to the already running DSP (attach state 0) without repeating SLIC/DSP initialization. The operator moved one handset from PHONE1 to PHONE2 and entered the fixed test sequence on each. Between each off-hook/on-hook pair the monitor matched **12 ordered DD02 START/END pairs**, including the full BE32 ASCII value on both edges and the expected physical line. Both lines reported PASS, exit 0. No observed digits or audio were logged. This confirms local keypad reporting, not SIP INFO or RTP telephone-event delivery.

CLI SHA-256 verified after transfer: `9b6cce9a78d9ea0a30fb9caebe6994cfa8a1e8cded111ed5bdd1293b2efba330`. Before deployment `ctest --test-dir build -R cli_startup` passed, including successful keypad simulation and rejection of a release word with the expected low byte but incorrect high bits.

## M2 live — 2026-11-25: T2.1 and T2.2 pass

Base `5670764` + working tree. TP-Link VX830V master running OpenWrt SNAPSHOT `r35484+502-11a859cca7` (AArch64, kernel 6.18.44); `lan0.500` UP/LOWER_UP, MTU 1500. Verified the deployed CLI SHA-256 against the M2 preparation fingerprint below. Ran `vx830v-voicectl events --device lan0.500 --locale 0` once. Capture: none; evidence is live CLI output, not a pcap.

| Test | Observed | Result |
|---|---|---|
| T2.1 | DD01 `1a` and `1b` status 0. At attach: DD02 command 2 state **1**. SLIC locale 0: DD02 command 1 result 0. DSP defaults accepted; after DSP_START: **new** DD02 command 2 result **0**. Four separately acknowledged channel defaults for slots 0,1,2,3. `ready: elapsed_ms=115`. | **Pass** (limit 5000 ms) |
| T2.2 | Operator lifted/replaced PHONE1, then PHONE2. Same-session CLI reported `HOOK_OFF line=0`, `HOOK_ON line=0`, `HOOK_OFF line=1`, `HOOK_ON line=1`, in that order. | **Pass** (both ports) |

DD02 command IDs 24/20 also appeared between the hook transitions. The monitor reports only the command IDs, so the digit values required by T2.3 were not verified. Stopped the temporary test monitor after reading the results.

## M2 preparation — 2026-11-25: init and events ready for live test

Uncommitted working tree on `5670764`; no capture. Normal and ASan/UBSan CTest runs with `VX_BUILD_TESTS=ON` and `VX_BUILD_TOOLS=ON` each passed 4/4. `cli_startup` exercised the executable over a private Unix datagram pair: successful boot (including SLIC result before its ACK), same-session hook events on both lines, cached attach state without a fresh start completion, negative SLIC result, and rejected DD01 command. Only the first two simulations reached ready.

Cross-compiled `build-aarch64/vx830v-voicectl` with the OpenWrt GCC 14.4 musl toolchain, `MinSizeRel` and `-static`. `file` identified AArch64, `readelf -d` found no dynamic section, and `qemu-aarch64 ... --help` exited 0. SHA-256 `8cd334eca24c35ed2446aa9a3b70be2cb646c5e59f2fe5bbfba77c2c9cd884f8`. **T2.1 and T2.2 have not run on hardware.**

## M1.1 — 2026-11-25: library polish complete

**Review follow-up (T1.7/T1.9/T1.10):** On this host a fresh default configure found checkpatch but not clang-format, clang-tidy or cppcheck; `lint` ran checkpatch and explicitly skipped the other three checks. Strict configuration on the same tree failed as expected with the missing tool and its cache override. With `VX_REQUIRE_LINT_TOOLS=ON` and explicit paths to tools unpacked under `/tmp/opencode`, full lint passed locally, and CTest passed 3/3. Reviewed the formatted array declarations and expanded return-value comments; no API or wire changes.

Uncommitted working tree, base `5670764`. Capture reference: none. Commands run from `voip-feed/vx830v-voip` unless stated otherwise.

| Test | Result | Evidence |
|---|---|---|
| T1.7 | Pass | GCC 15.2, CMake 4.2.3, `VX_BUILD_TESTS=ON`: static/shared and test targets built with `-Wall -Wextra -Werror`; CTest `packet_read`, `unit`, `qemu_diff` 3/3. Separately hid QEMU from PATH: explicit skip message, code 77. |
| T1.8 | Pass | Clang 21 `VX_SANITIZE=ON` build: `packet_read`, `unit_sanitized`, `qemu_diff` 3/3, no ASan/UBSan report. |
| T1.9 | Pass | `cmake --build build --target lint`: format clean, strict checkpatch 0 errors/warnings/checks on **14 files**, clang-tidy and cppcheck clean on project code. [Exceptions and tool discovery](foss-design.md#offline-verification). |
| T1.10 | Pass (source review) | See checklist below. |
| T1.11 | Pass | Fake clock and MAC exercise immediate `1c` rejection, seed/wrap, and a queue-full retry after drain with single delivery. Mock `recvfrom`: identical lengths, `PACKET_OUTGOING` rejected, `PACKET_HOST` accepted. |
| T1.12 | Pass | Standalone `python3 -B tests/qemu/diff.py`: **19 comparisons, 2,528 bytes**. CMake `VX_FUZZ=ON`, `fuzz` target: **96,953,265 runs in 601 seconds**, no finding. |
| T1.13 | Package staging passed; standalone SDK invocation deferred to T5.1 | No standalone SDK found. Used the configured OpenWrt AArch64 Cortex-A53 musl source build instead. Top-level package discovery stopped before compile on an unrelated `scanner` group-ID collision. Invoked the same package recipe directly with its staged host/target toolchain on PATH: compile/install-dev succeeded. Verified AArch64 ELF SONAME `.so.0`, archive, `usr/include/vx/{voice,wire,dsp,link,packet}.h` and sysrooted `pkg-config --cflags --libs`. No device install. |

**T1.10 review checklist**

- [x] `wire.h`: Ethernet/header offsets, marker direction, integer order, byte lengths and error codes; decoded views explicitly borrow the input.
- [x] `dsp.h`: per-command constants and direction, body-relative offsets for media/CID/tone/words, opaque layouts, array ownership and encode/decode sizes. Checked against board-link.md's inventory and media table.
- [x] `link.h`: private state vs caller observations, single owner and non-reentrant send callback, timer units, error vs completion, DSP readiness and event-loop example; `packet.h`: fd ownership, Linux interface and lifetime, partial read/write errors.
- [x] Implementation: comments explain CRC coverage and padding, field descriptors and zeroed reserves, sequence/retry budgets, independently learned MACs, negative ACKs and cached successes, kernel filter and `MSG_TRUNC`. Public headers install independently of internal helpers.

## M1 — 2026-11-25: protocol library complete

**Review follow-up (uncommitted):** `make -C tests/unit check` and `make -C tests/unit sanitize` passed after removing `1c` from the request engine, randomizing session sequences, and caching only successful event enqueues. Tests include sequence wrap and a rejected event retried after queue drain. `python3 -B tests/qemu/diff.py` again matched 19 vectors (2,528 bytes) with a deterministic replay seed. The updated packet-type filter compiled; no hardware or real-interface test was run. The original M1 results below describe the pre-review baseline.

Base `5670764` + working tree for all rows; capture reference **none**. Commands run from `voip-feed/vx830v-voip`. [As-built API and reproduction](foss-design.md).

| Test | Command / result | Evidence |
|---|---|---|
| T1.1 | `make -C tests/unit check` — pass | `277e`, `591f`, `bb3d`; full header round-trip, padding excluded, bad CRC and truncation rejected. |
| T1.2 | Same unit run — pass | All four request subtypes, 120 or 2 identical retries, final deadline, busy, wrong subtype/seq/source/destination/length, peer status and initial/retry transport error. |
| T1.3 | Same unit run — pass | Successful enqueue despite failed first ACK send, identical cached resend, short status 1, full FIFO nonzero ACK cached even after draining. Separate DD03 learning and slot/selector checks. |
| T1.4 | Same unit run — pass | All 34 inventory IDs in both directions, known body widths, BE fields, reserved padding, opaque preservation, family 2/10 and non-command rejection. |
| T1.5 | `python3 -B tests/qemu/diff.py` — pass | **19** full comparisons, **2,528 bytes**: both lines' CID/ring, stop-ring, open, RTP active/off; four channel defaults; init, forwarded stop-ring and DD03 datagram frames. Only the unspecified final ring word (4 bytes per line, different between identical OEM runs) is supplied as an opaque input. The comparison does not claim independent generation of stack residue. |
| T1.6 | `make -C tests/unit sanitize` and Clang libFuzzer with ASan/UBSan, `-max_total_time=600 -max_len=1520 -print_final_stats=1` — pass | Unit suite clean. `fuzz.c` with `wire.c`/`dsp.c`: **107,820,452 inputs in 601 s**, exit 0, no findings. Raw malformed frames, media prefixes, and structured frame/record decode paths. Reproduce with `make -C tests/unit fuzz`. |

Both host library formats built with warnings as errors. AF_PACKET compiled but was never opened; M2 is the first hardware interop gate. No assertions or scripts in the OEM oracle were changed for M1.

## M0 — 2026-11-25: complete after T0.3 repair

**T0.3 rerun: pass, exit 0.** `python3 -B examples/voip_qemu/controller.py`, base `5670764` + working tree, offline QEMU, no capture. The other five baseline results below still apply. All six M0 criteria now pass; M1 is unlocked.

| Controller scenario | Passed evidence | Transcript bytes |
|---|---|---:|
| default | Startup, channel/open and tone records, native endpoint samples, two CMSIP make-call requests, PCMA/PT8/20 ms and telephone-event/PT101 media serialization | 4164 |
| bad_digits | No request within the invalid-line observation window; valid second line originates and its sequence resolves to its slot | 2556 |
| teardown | Two provisional call-status inputs bind call indexes 2 and 3; on-hook frees both slots and generates CMSIP 1020 for each index | 6456 |
| concurrent | Second line retains its slot and call index after the first is released | 6456 |
| startup_failure | Guest checks for silence during a 100 ms withheld ready reply; then checks the complete recovered call transcript | 4164 |
| peer_interrupt | Local DSP send reports failure with no descriptor; after socket-pair replacement the allocated lines tear down correctly | 6456 |

`controller.py` now consumes each DSP record, then the little-endian sample for that line; it checks the entire transcript through EOF. In the guest, OEM stdout diagnostics are separated from binary records. Teardown initializes the OEM call-index table and supplies a simulated CMSIP 100 Trying response. The fixture stubs management-service call-history persistence. The observed release is a **local CMSIP request by call index**, not a network BYE. Media serializer calls remain independent of negotiation; DSP descriptor replacement does not simulate a physical board reboot.

Passing SHA-256: `controller.py` `4220983985744d6bdcddbe36ffe9069a6c3fa074bdb88e6c73e573083dc62b77`; `controller.c` `bf5e332ed30142761061a0394ed3689573c505e23c9eadbb53316a30b93077e7`.

### Original baseline (before repair)

Base commit for **each row**: `5670764d49c0305fe76d5b454c24a9189f29bea1`, plus the existing uncommitted plan, findings and QEMU fixtures. The commit alone does not identify the fixtures. Capture reference for all rows: **none** (offline QEMU or static review). Commands below ran from the repository root, normal Python with assertions enabled. QEMU 10.2.1, Clang with AArch64 support and `readelf` were present; `run.py`'s existing Rust-sysroot `ld.lld` fallback supplied the linker. No oracle edits.

| Test | Command or review | Result | Evidence |
|---|---|---|---|
| T0.1 | `python3 -B examples/voip_qemu/run.py` | **Pass**, exit 0 | Six DSP records matched. DD01 init/record CRCs `277e`/`da0a`; identical duplicate DD02 ACK and one delivery; 172-byte RTP datagram preserved on DD03. |
| T0.2 | `python3 -B examples/voip_qemu/workers.py` | **Pass**, exit 0 | 17 distinct DD01 transactions including reattach, one byte-identical retry, 8 DD02 events; proxy still running after two line lifecycles. |
| T0.3 | `python3 -B examples/voip_qemu/controller.py` | **Fail**, exit 1 | Seven default startup records matched. `controller.py:79`: `AssertionError: line 0 sample: (8, 0, 0)`. Remaining checks did not run. |
| T0.4 | `python3 -B examples/voip_qemu/dtmf.py` | **Pass**, exit 0 | 72 DSP event assertions and 24 independent SIP INFO formatter writes, digit and call index verified. |
| T0.5 | Review [peer.md](peer.md) against plan §3.3 | **Pass** (static review) | DD01 statuses/unsupported subtype, DD02 generation, CID/ring/locale/tone, media address substitution, codec/ptime, wideband and restart have peer addresses. Physical or unresolved behaviour is assigned to T2.1–T2.8. |
| T0.6 | Review [protobuf.md, EtherType safety](../../plans/protobuf.md#ethertype-safety--no-voip-conflict) against [master proxy](master-proxy.md) and [peer](peer.md) | **Pass** | DD01/DD02 requests and same-type replies, bidirectional DD03, and master Unix stream path `/var/tmp/sock_CM_DSP` documented. |

**T0.3 diagnosis:** After the seven startup records, `controller.c:130–138` writes command 8, command 13, another command 13, then one native AArch64 three-word sample **per line**. `output()` also writes each whole 132-byte DSP record to stdout. `controller.py:76–79` instead reads the first 12 bytes immediately as big-endian sample words and expects two adjacent samples. `(8, 0, 0)` is the prefix of the first CHANNEL_OPEN record. The failing assertion does not establish an off-hook or slot-allocation fault. Reconcile the producer/reader transcript and native sample byte order in a separately authorized oracle change, then rerun the unchanged milestone command through exit. Do not count startup alone as a pass.

Failing fixture fingerprints (SHA-256): `controller.py` = `dcc933e79dcb500dcc868c8469ed442d4fba24f918a58564b9e6ff5caeac9976`; `controller.c` = `ea11b9ff7b5691d0be1a021e094358e3c9cf8611395db59478c1968c9ae02230`.

**Gate at original baseline:** 5/6 M0 tests passed. Superseded by the T0.3 rerun above.
