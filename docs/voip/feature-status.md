# VX830v OpenWrt VoIP: what works today

The VX830v can make and receive SIP calls on a physical analog handset. PHONE2 was tested against a LAN Asterisk PBX, including caller number display, speech in both directions and in-call keypad tones.

| Feature | Status | What we know |
|---|---|---|
| Handsets | Working | Both ports detect pickup, hangup and all 12 keypad keys. Ringing and dial tone were verified on both. |
| Caller ID | Number works; timestamp missing | The same handset displays CID timestamps on POTS but only the number on ordinary VX830v calls. The master sent the timestamp to the board. Private/unavailable indications work; ordinary caller-name support is unknown. |
| Fax passthrough | Mode switch verified | A 2100 Hz fax answer tone on PHONE2 switched the active G.711 call into passthrough. No fax page transfer tested. |
| Per-port gain | Raw controls verified | Each port has independent microphone (TX) and earpiece (RX) settings, −6…+6, default 0. One step does not mean one dB. [Measurements and deferred electrical check](test-report.md#t418-live-raw-per-port-gain-evaluation). |
| Audio | Working over SIP; quality testing continues | With wired-LAN tuning, all 60 master downlink measurements were under 150 ms (median 91 ms, max 128 ms). Delay still grew during the minute; longer calls, uplink delay and conversational echo need checking. |
| SIP registration | Working on the router | baresip registered with Asterisk while the stock voice board reported ready. |
| Incoming and outgoing SIP calls | Working on PHONE2 | Dialling, ringback, incoming CID, answer, hangup from either end and redial worked. Asterisk received in-call digits in both RFC 4733 and SIP INFO modes. |

Both handsets can carry **simultaneous independent SIP calls** on separate accounts. Hanging up one left the other connected with working speech. On PHONE2, a waiting beep and the R key let the operator switch between two callers; hanging up recalled the held caller. No indicator light was needed. PHONE2 also completed 31 consecutive calls. Long-call latency growth and conversational echo remain M4 checks.

**Also verified on hardware:** both ports send in-band digits; PHONE2 plays distinct busy, unobtainable and congestion tones. Exact matches, `#`, digit timeouts and on-hook cancellation worked. The shared front-panel FXS indicator follows both lines. PHONE2 completed mandatory SDES-SRTP and G.722 SIP calls. G.722 also runs on the board link, but showed no usable wideband benefit on the tested handsets, so PCMA remains the board default. Voicemail stutter and shared-account first-answer ringing were verified. [Full bench queue](m4-bench.md).
