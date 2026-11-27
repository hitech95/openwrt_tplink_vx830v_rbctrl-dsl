# AArch64 call controller: startup through outgoing media

[VoIP index](index.md) · [Board link](board-link.md) · [DTMF](dtmf.md) · [Proxy worker transcript](worker-transcript.md)

This analysis covers the stock `voip_client` on the AArch64 master. Function addresses are Ghidra VAs for the [pinned image](map.md). Static findings describe branches present in that image; QEMU findings use a simulated DSP and seeded account state. No SIP call or physical board was used.

## DSP initialization and recovery

`voip_client_main` (`413954`) connects to the proxy, starts its socket thread, waits for `/var/tmp/remote_init` to disappear and then initializes voice state (`420ebc` → `41f5a0`). This is separate from the proxy's `1a`/`1b` Ethernet handshake.

| Step | Host action and dependency |
|---|---|
| Configure SLIC | If provisioned locale differs from cached locale, `415070` sends DSP command **1** with one BE32 country selector (0–20) and waits on a dedicated semaphore. The DSP dispatcher `415e2c` wakes it on **DSP event 1**. It logs status `-1` (DSP module load failure) and `-2` (SLIC initialization timeout); the waiter itself does not return the status word and no local timed wait is visible in this routine. |
| Load host device and set defaults | The init path invokes `insmod /lib/modules/voip.ko`, then sends command **0** with one BE32 locale word (`412e3c`). This host module operation is not an Ethernet message. |
| Start DSP | `412e84` sends command **2** with BE32 `1` for the argument used at boot, or `0x101` when its argument is nonzero. Its guard requires previous state word `1`; it changes that word to `0` and waits on the controller semaphore. DSP event **2** updates the state word and wakes the waiter (`413438`). Result `1` is the stopped state in this code; a result other than `1` enables the startup flag and triggers command **7** for each slot `0..3`. QEMU used simulated result `0`; the complete peer status enum is unverified. |
| Initialize local slot table | `41f5a0` calls `415170` for each slot. This resets *host-local* entries independently of the DSP command-7 burst. Enabled lines and profile parameters are then read from the vendor data model. |
| Restart | `420fb0` re-enters `41f5a0(0)`, but this does not guarantee a second **DSP_START**. The guard word `460068` starts at `1`; `412e84` clears it before sending and `413438` replaces it with the state from event 2. Unless it is back at `1`, the next start helper skips both send and wait. The stop helper `412f08` would set it to `1` and send command **3**, but no direct caller was identified. On the recovered path, event 2 reporting `1` restores eligibility. No autonomous peer-reboot recovery was demonstrated. |

**Do not use the transport ACK as a ready signal.** DD01 ACK acknowledges the command transfer, whereas DD02 carries the SLIC/DSP replies. Also do not copy the stock shared-semaphore design as a protocol requirement: the production code reuses that semaphore for other events. The fixture isolates the DSP start routine and proves that, with no other wakeups, the command-7 burst follows the injected reply.

## From off-hook to the first completed digit

The verified path is the normal idle-to-dialing branch, with a registered account and no special call/profile overrides:

1. **Off-hook:** DSP event 18 flows through `415e2c` → `413728` (queue) → `418c18` → `433f60` → the endpoint state machine `42af24`. Without a registered account and outside manufacturing mode, the stock code can take an alternate tone/state path instead of opening a channel (`42b6e8–42b734`).
2. **Allocate and open a DSP channel:** `4155fc` picks the first free slot matching the line's affinity. `415170` sets affinity `slot & 1`: slots **0/2** for line 0, **1/3** for line 1 in the stock allocator. The proxy relay does not enforce this policy. `414f0c` sends command **8** immediately; the idle branch does not wait for a separate channel-open completion before requesting dial tone.
3. **Dial tone:** Normal `cmTone` request selects **4**, with the initial duration from the tone profile (`42b854–42b8bc`). `437268` queues the action. Inside `voip_parse_msg_in_queue` (`FUN_00418c18`), the case at `418dbc` dispatches it to the DSP tone serializer. The fixture saw command **13** with `(line, 4, 0, 0)` in the first four body words and controller state **1** (digit collection). Duration zero is the fixture profile value, not a universal stock setting. DND and other profiles can choose alternate selectors.
4. **Digit edges:** Supplying command **24** (`DTMF_START`, ASCII `'5'`) did not append a digit or stop tone in the tested branch. Supplying command **20** (`DTMF_END`) appended exactly one `'5'` and emitted **command 13 with selector 0** to stop dial tone. Do not substitute command 14 solely because its name is `TONE_STOP`. Command 71's dispatch to the same completed-digit event is covered separately in [DTMF](dtmf.md).

### `CHANNEL_OPEN` body (`0x2c` meaningful bytes)

Offsets below start **after** the four-byte command number. All entries in this table are BE32 except that a cleared field has the same representation in either order. The record pads to 132 bytes.

| Offset(s), hex | Value constructed by `414f0c` |
|---|---|
| `00` | Allocated DSP slot. |
| `04`, `08` | Endpoint profile words at `ep+8`, `ep+0xc`, from provisioning IDs `0xa` and `0xb`. Meaning not independently confirmed. |
| `0c`, `10`, `14` | `1`, `0`, cleared. |
| `18`, `1c` | Byte-valued configuration getter `421568` widened to BE32, then `0`. |
| `20`, `24`, `28` | Endpoint Boolean derived from provisioning ID `0xd`, followed by `1`, then cleared. |

With zeroed profile fields the OEM fixture emitted body words `[slot, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0]`. This byte vector is not a recommendation to zero provisioned profile values on hardware.

## Dial decision and media

`cm` does not send every key to SIP. `434294` appends/evaluates the digit buffer and consults matching rules and feature codes. Depending on provisioning ID `0x66`, `#` with an existing buffer can be treated as the same evaluation trigger as the internal timeout token `'T'`. That token is not a physical key. `41d0c4` evaluates the digit map; the controller also checks internal extensions and registration/account selection before `cmOriginate` (`425544`). This is static analysis of the routing branch, not a complete digit-map grammar or a proof that a particular default dial string selects a provider.

Once origination is selected, `425544` builds the SIP destination URI, records the channel sequence (`415424`) and queues internal event `0x400003ee`. The corresponding case inside `FUN_00418c18`, at `418fe0`, looks up the active slot by sequence and emits CMSIP **1006** (`0x800003ee`, native little-endian), with call kind, sequence, account index and URI. This is the boundary a replacement can expose as a typed call request; legacy CMSIP is not required if the replacement owns both sides.

Media is a later transaction. For negotiated media handed back to CM, `424d24` maps the call state and SDP-derived values into the DSP profile. `4165f8` emits command **10**, `415908` handles **11** updates, and `416be8` emits **12** followed by **91** on teardown (request for statistics on that slot). The QEMU fixture verified the original activation bytes for **PCMA/PT 8**, **20 ms** both ways and **telephone-event/PT 101** on both lines, followed by deactivation and statistics request. PT 101 is a fixture negotiation value, not an OEM fixed value. The [board-link field table](board-link.md#media-configuration-vs-media-packets) separates codec, RTP payload type and relay mode.

## Reproduce and interpret

```sh
python3 -B examples/voip_qemu/controller.py
```

The [guest fixture](../../examples/voip_qemu/controller.c) calls original startup routines with a seeded locale, previous state and registered account. It provisions the literal digit map `5550100`, feeds completed digits and reads the resulting **local CMSIP call request** for `<sip:5550100@example.invalid:5060>`. It supplies post-negotiation media state separately to the serializers. This is not a SIP dialog with a provider. The expanded two-line QEMU driver is work in progress: its transcript parser must distinguish 132-byte DSP records from native endpoint samples before its concurrent-call assertions can be relied on.

**Remaining boundaries:** the controller now has executable evidence from startup through first digit, plus independently verified call/media serialization. Dormant/optional dial-plan branches, timeouts under real provisioning, concurrent calls, peer reset while media is active, board-generated telephone-event timing and real analog behavior remain open. No choice of SIP implementation is implied by these results.

## Approaches attempted and outcomes

### CMSIP call-status injection via `0x40fd08`

A previous fixture supplied synthetic call-status and media messages to `40fd08` after the local CMSIP originate request. That experiment ended with a guest SIGSEGV; it did not isolate the cause or demonstrate an additional SIP socket requirement. Direct serialization (`4165f8`, `416be8`) is separate evidence from activating media through the full call state machine.

### On-hook teardown via `ingest(17,line,0)`

An earlier on-hook experiment appended release records to the transcript while the Python reader still used the old fixed offset for line 1. The unexpected command at that offset is **not evidence of cross-line state corruption**. Independent channel release and a surviving second call still need an end-to-end assertion.

### Mocked inputs in the fixture

The following are not provided by the real system in the QEMU fixture:
- `/var/tmp/remote_init` disappearance (startup waits for it; the fixture bypasses the wait)
- Registered-account data model (`0x469978`, `0x4699fc`, provisioning IDs `0x2726`/`0x2752`) is seeded directly in BSS
- DSP replies to commands 1 and 2 are simulated; command 7 records are host output
- SIP peer: no SIP stack or registrar; the fixture reads the local originate request generated from DTMF_END events
- Media peer: no actual RTP endpoint; the fixture supplies local loopback addresses/ports to the serializers
- Management IPC (`cmm` lock calls return `errno=22` noise but do not block the fixture)
- LED/module ioctls, status-file writes, and voicemail are stubbed or intercepted
