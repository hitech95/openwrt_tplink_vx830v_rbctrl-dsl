# VX830v OpenWrt VoIP: what works today

The VX830v can make and receive SIP calls on a physical analog handset. PHONE2 was tested against a LAN Asterisk PBX, including caller number display, speech in both directions and in-call keypad tones.

| Feature | Status | What we know |
|---|---|---|
| Handsets | Working | Both ports detect pickup, hangup and all 12 keypad keys. Ringing and dial tone were verified on both. |
| Caller ID | Partly verified | PHONE2 displayed the caller number in a board test. Name is unconfirmed; date/time did not appear. |
| Audio | Working over SIP; quality testing continues | With wired-LAN tuning, all 60 master downlink measurements were under 150 ms (median 91 ms, max 128 ms). Delay still grew during the minute; longer calls, uplink delay and conversational echo need checking. |
| SIP registration | Working on the router | baresip registered with Asterisk while the stock voice board reported ready. |
| Incoming and outgoing SIP calls | Working on PHONE2 | Dialling, ringback, incoming CID, answer, hangup from either end and redial worked. Asterisk received in-call digits in both RFC 4733 and SIP INFO modes. |

PHONE2 completed 31 consecutive calls with stable socket and memory use. M3 is complete; latency growth and echo are checked in M4 (T4.20) and the latency target in M6 (T6.8). If the remote end hangs up first, the lifted handset currently goes silent. The disconnect tone is planned for [M4, test T4.9](../../plans/voip-baresip-plan.md#m4--two-lines-and-complete-ata-feature-set).
