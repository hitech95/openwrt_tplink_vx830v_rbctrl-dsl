# Stock master proxy: two-line worker transcript

[VoIP index](index.md) · [Wire format and command inventory](board-link.md)

**What this proves:** the unmodified AArch64 proxy executable runs its normal `main`, initialization, control, queue, local DSP socket and media workers through the exchange below. Both ports were exercised sequentially during one process lifetime. **Who chose the exchange:** a simulated controller supplies the command sequence; a simulated board acknowledges and supplies events. The ring records themselves come from the OEM AArch64 client routine. This is a host compatibility fixture, not a physical-board capture or an inferred requirement to use this exact call order.

## Reproduce

```sh
python3 -B examples/voip_qemu/run.py      # isolated OEM routine vectors
python3 -B examples/voip_qemu/workers.py  # process/worker transcript
```

Use normal Python (assertions enabled). Both use the same [hash-pinned preparation code](../../examples/voip_qemu/run.py). The [worker shim](../../examples/voip_qemu/worker_io.c) substitutes three private datagram socket pairs for the three raw interfaces, supplies the master MAC, redirects the DSP socket and readiness marker into scratch space, suppresses shell side effects, and uses ordinary thread scheduling. Datagram filtering, VLAN tagging, CPU affinity and real-time scheduling are consequently not under test. The RTP/RTCP data path uses actual UDP binds and sends on IPv4 loopback. No privileges, router access, SIP account, or remote firmware are needed.

The [scenario](../../examples/voip_qemu/workers.py) compares emitted headers, sequence, payload, CRC, destination learning, returned DSP records and UDP payloads. Before proxy startup it invokes the original client's `voip_start_ring` at `412f6c` with two fixture endpoint profiles, fixed date `01020304`, number `5550100` and name `Fixture`. For **each** line that produces `4 CID_PLAY` followed by `5 RING_START`, with the respective zero-based line ID. In this mode, body offsets 29 and 69 of CID_PLAY contain name and number respectively. The harness forwards those exact 132-byte records; it doesn't generate caller-ID or ring payloads from a guessed struct.

## Observed conversation

Each DD01 and DD02 control frame below was answered on its own EtherType. A successful transport reply is not a DSP state transition. All DSP IDs are decimal; envelope subtypes are hexadecimal.

| Step | Input supplied by fixture | What the running OEM proxy did |
|---|---|---|
| Boot | Provide a functioning link; acknowledge init | Sent broadcast DD01/1a seq 1. Removed init marker after success. |
| Attach | Connect to proxy's Unix listener | Sent DD01/1b seq 2 to the learned peer MAC. |
| Alert | Submit client-generated CID and ring records | Emitted separate DD01/16 requests with unmodified records. When the first line's ring ACK was withheld once, retransmitted the **identical frame**, same sequence. |
| Answer | Board sends DD02/18 with DSP 18 (off-hook); controller submits DSP 6 | ACKed the event and delivered it intact to the Unix client. Forwarded the separate stop-ring request. The hook event does not itself prove which component decides to stop ringing. |
| Start media | Submit DSP 10 with a slot, PCMA and loopback endpoints | Forwarded the record and established local RTP/RTCP sockets. UDP RTP became DD03/19 with slot and selector 3. DD03 RTP became the original UDP datagram sent from the configured local port. |
| Control media | Supply an eight-byte RTCP receiver report | UDP RTCP became DD03 selector 4. For the **reverse** direction see the observation below. |
| Update | Submit DSP 11 with a different local RTP port | Relay accepted UDP on the new port; the old RTP and RTCP ports were available to bind again. |
| Digits and release | Board sends DSP 24, 20 (both with fixture digit word 5), then 17 (on-hook); controller submits DSP 12 and 31 | Three ACKs and three distinct, exact event records delivered on the local stream. Deactivation and channel close were forwarded. Both UDP ports were released. This tests event carriage, not how the physical keypad encodes a digit. |
| Second port | First run uses line 0 with slot 3; second uses line 1 with slot 2 | Each line's control/event ID and its independently supplied media slot are preserved. The relay handles both mappings. The fixture does not model simultaneous calls or the OEM controller's slot-allocation policy. |
| Local recovery | Close and reconnect the controller stream | Sent another DD01/1b without repeating 1a. Sequence continued; proxy remained running. |

**Run totals:** 17 distinct DD01 transactions (1 init, 2 attach notifications, 7 forwarded DSP records per line), one extra identical retransmission, 8 DD02 events. Each line passed RTP in both directions and released its sockets. The harness terminates the proxy after the checks; that is not an OEM shutdown sequence.

### An IPv4 RTCP distinction that matters

In the activation setup used here, the remote control port and the local RTP+1 port were different. Injecting DD03 selector 4 from the board **did not deliver a datagram to the configured remote UDP receiver**. Instead, the same RTCP bytes came back out on DD03 selector 4. The IPv4 branch of proxy `4040e4` stores the **local** IPv4 address and local RTP+1 in its RTCP destination. This explains the observed loop through the proxy's own local RTCP socket. Treat this as observed stock behavior on this path, not a universal RTCP routing rule. IPv6, other media modes and the peer's use of this path remain unverified.

The IPv4-only fixture did not exercise IPv6. Static analysis separately establishes why a stock client IPv6 profile still selects IPv4 in this proxy: the two ends interpret the [address-family word](board-link.md#media-configuration-vs-media-packets) in different byte orders.

## What still separates this from a complete ATA specification

* **Controller policy:** only the ring builder is OEM client code in this process-level scenario. A separate [controller fixture](controller-startup.md) now exercises startup routines, channel allocation, dial tone and the first completed digit. SIP negotiation, hook-flash policy and concurrent calls remain unverified. The mock peer accepting a command cannot demonstrate how the hardware interprets it.
* **Recovery:** a delayed control ACK, duplicate inbound events (routine fixture) and local controller reattachment have coverage. Missing peer at startup, peer reboot during a call, exhaustion of retries and restoring an active media slot do not.
* **Media and optional features:** the fixture exercises IPv4 RTP forwarding, local-port changes and teardown. For destination-only renegotiation there is a separate **static** result: in `4040e4`, command 11 rewrites the proxy's cached RTP and RTCP destinations only inside their respective local-port-change branches (RTCP uses local RTP+1). With unchanged local ports, the host relay keeps its old destinations even if the forwarded DSP record specifies new ones. This does not establish what the board does with that record. IPv6 operation, negotiated DTMF, fax, supplementary features, `1c` and unnamed DSP IDs remain open.
