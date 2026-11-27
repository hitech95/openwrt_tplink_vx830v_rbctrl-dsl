# VX830v OpenWrt VoIP: what works today

**As of 2026-11-27:** both analog ports support SIP calling. On baresip 4.9.0 (service r6), the operator used **R on PHONE2** to hold and resume a PHONE1→PHONE2 call: recall dial tone was audible and speech returned **in both directions without redial**. The router recorded `connected → held → connected` on the same PCMA call. **Only the clean-image install test (T5.1) remains for M5/M5.1; it is deferred.** The r6 service and r7 LuCI editor were tested as upgrades. [Acceptance evidence](test-report.md#m5-review-verification)

**LuCI account settings** include the dial plan and both inter-digit timeouts, registration-dependent dial tone, and one Alert-Info ring mapping. They apply to every assigned port. Hunt rings the first free assigned port in physical order (PHONE1, then PHONE2). Port settings `first_digit_timeout`, `offhook_warning_timeout`, `howler_duration` and `cw_cid` remain UCI-only.

| Feature | Status | What we know |
|---|---|---|
| Handsets | Working | Both ports detect pickup, hangup and all 12 keypad keys. Ringing and dial tone were verified on both. |
| Caller ID | Partial | Number and private indication displayed. The same handset shows timestamps on POTS but not on ordinary VX830v calls; ordinary name display is unconfirmed. Unavailable indication has fixture evidence only. Waiting-call CID did not show the number; type II is opt-in and off by default. |
| Fax passthrough | Mode switch verified | A 2100 Hz fax answer tone on PHONE2 switched the active G.711 call into passthrough. No fax page transfer tested. |
| Per-port gain | Raw controls verified | Each port has independent microphone (TX) and earpiece (RX) settings, −6…+6, default 0. One step does not mean one dB. [Measurements and deferred electrical check](test-report.md#t418-live-raw-per-port-gain-evaluation). |
| Audio | Two-line board-clock drift passed | Over ten simultaneous minutes, delay grew by 2.2 ms per line. This measures router downlink drift; absolute end-to-end latency and conversational echo are separate. |
| SIP registration | Working on the router | baresip registered with Asterisk while the stock voice board reported ready. |
| Incoming and outgoing SIP calls | Working on PHONE2 | Dialling, ringback, incoming CID, answer, hangup from either end and redial worked. Asterisk received in-call digits in both RFC 4733 and SIP INFO modes. |

Both handsets can carry **simultaneous independent SIP calls** on separate accounts. Hanging up one left the other connected with working speech. On PHONE2, a waiting beep and the R key let the operator switch between two callers; hanging up recalled the held caller. PHONE2 also completed 31 consecutive calls. Conversational echo carries into M6; it has not been signed off.

**Also verified on hardware:** both ports send in-band digits; PHONE2 plays distinct busy, unobtainable and congestion tones. Exact matches, `#`, digit timeouts and on-hook cancellation worked. The shared front-panel FXS indicator follows both lines. PHONE2 completed mandatory SDES-SRTP and G.722 SIP calls. G.722 also runs on the board link, but showed no usable wideband benefit on the tested handsets, so PCMA remains the board default. Voicemail stutter and shared-account first-answer ringing were verified. [Full bench queue](m4-bench.md).

## Stock firmware (OEM) vs OpenWrt

The **OEM** column describes what is visible in the extracted [TP-Link telephony UI](../../vx830v/squashfs-root/web/main/) and its [English labels](../../vx830v/squashfs-root/web/locale/en_US/str.js), or in the [stock voice-board commands](peer.md#ata-feature-reachability-m4-evaluation-basis). A bundled page or command is evidence of a feature path, not proof it was enabled on every ISP build or tested on this unit. **OpenWrt** means the local `rbctl-voip` service; features a separate PBX might provide are not counted.

| Feature | OEM evidence | OpenWrt support | What is still missing or unverified |
|---|---|---|---|
| Analog calling and call waiting | SIP accounts, analog line control and call-waiting settings | **Supported** — two simultaneous calls; R holds/resumes a single call or swaps waiting callers | No known gap in these tested operations. R does **not** transfer the call to another number. |
| Transfer and three-way calling | Peer has conference start/stop commands (15/16); OEM transfer workflow not verified | **Missing** — no local transfer or three-way conference | Handing a caller off or joining three parties. Hold/swap keeps callers separate. |
| Dial plans and outgoing line selection | Digit-map editor; call rules select the outgoing number by call type or prefix | **Partial** — account dial plan, `#` to send, short/long timers; one account per port | Selecting a different outgoing account by prefix or call type. |
| Caller ID and distinctive ring | Peer accepts caller-ID data, waiting-call CID and ring cadence selection | **Partial** — number/private display and one provider Alert-Info mapping per account | Timestamp and waiting caller's number did not display on the tested handset; ordinary name remains unconfirmed. |
| Call forwarding and call-through | Forwarding rules (including unconditional and no-answer); PIN-controlled call-through page | **Missing** | No local forwarding rules or incoming-to-outgoing call-through service. |
| Do Not Disturb and number blocking | Scheduled DND, incoming/outgoing blocks and number allowlist | **Missing** | No schedules or caller-number lists. The registrar-IP check filters SIP sources, not callers. |
| Phonebook, speed dial and automatic dialing | Contact list, speed-dial entries and a configurable number to dial after off-hook inactivity | **Missing** | No local address book, speed-dial table or off-hook autodial. The latter is distinct from dialing an emergency number normally. |
| Call history and missed-call notifications | Call log and missed-call email option | **Missing** | Live ubus call events are available, but no persistent user-facing history or missed-call email. |
| Answering machine and voicemail indication | USB voicemail pages; stock peer supports stutter dial tone | **Partial** — SIP mailbox notifications drive stutter tone | No local answering machine or USB message storage/playback. No visual mailbox-lamp command found on the stock peer. |
| Fax and media | T.38 setting; board selectors for G.726-32, G.729 and T.38 as well as G.711 and G.722 | **Partial** — G.711 and G.722 SIP calls; G.711 fax-tone fallback verified | No T.38, G.726 or G.729 in the managed profiles. No end-to-end fax page tested. |
| IPv6 voice | IPv6 SIP settings and IPv6 media paths in stock code | **Missing** in the managed service (IPv4 only) | End-to-end IPv6 signaling and media. |
| Integrated cordless handsets | Bundled DECT registration, PIN and radio-settings pages; hardware availability on this unit not established | **Outside current scope** — supports the two analog jacks | No router-side DECT pairing or management. A cordless base plugged into an analog jack uses that jack like any other phone. |

These are **parity gaps, not M5 release gates**. For the planned scope see [ATA features and deferred services](../../plans/voip-baresip-plan.md#7-v1-feature-set); for measured behavior see the [bench report](test-report.md).
