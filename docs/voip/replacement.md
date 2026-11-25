# Open-source replacement: first-pass plan

[VoIP index](index.md) · [Architecture](architecture.md) · [Proxy](master-proxy.md) · [CMSIP](cmsip.md)

## Goal and boundaries

Replace the closed **AArch64 master-side** VoIP components with an open-source
solution that can operate the existing voice hardware. Focus on the
observable host interfaces rather than assuming a replacement is simply a
stock SIP application. The external voice device is a compatibility endpoint;
its firmware is outside this analysis scope.

Potential boundaries, to validate before choosing implementation details:

1. **Hardware bridge:** replace `voip_server_proxy`; implement the host-side
   raw Ethernet control, DSP events and bidirectional media relay.
2. **Call controller:** replace `voip_client`; handle two phone lines,
   configuration, call/line state, DTMF and media lifecycle. Two FXS channels
   are configured in `etc/config.bba`; observed proxy code has four media
   slots, which is not the same thing as four physical phone lines.
3. **SIP stack:** integrate a maintained open-source SIP implementation.
    If the vendor controller is retained temporarily, provide CMSIP-compatible
    IPC. If controller and SIP stack are replaced together, use a cleaner
    internal interface instead of requiring legacy CMSIP forever. A QEMU attempt
    to drive the original `0x40fd08` CMSIP call-status/media routine from the
    fixture ended in a guest fault whose cause was not established. The direct DSP media serializers `0x4165f8`
    (activate) and `0x416be8` (deactivate) remain the verified boundary.
4. **Platform integration:** identify which `cos`, vendor data model, cmm,
   firewall, `/dev/voip`, logging and voicemail facilities are genuinely
   needed on the target deployment.

These are architectural options, not proven drop-in replacements. Validate
source availability and licenses separately for all dependencies.

## Prioritized investigation

1. **Build reproducibility.** The [imported ELF identities](map.md) are verified.
   Check the vendor PJSIP tree's build options and establish whether
   it can reproduce the shipped `pjsua`. Inventory source and license notices.
2. **Host DSP ABI.** Map every 132-byte record: command IDs, offsets, signedness,
   byte order, request/event direction, error and startup semantics. Cross-check
   client sender, proxy forwarding and real captures.
3. **Ethernet contract.** Recover filter rules, exact packet layout, checksum
   polynomial and known vectors, sequence/retry behavior, VLAN and peer-MAC
   discovery. Independently validate `0xDD01`/`0xDD02`/`0xDD03` traffic.
4. **Media.** Map activate/update/deactivate structures and RTP/RTCP relay
   payloads, UDP address/port selection, IPv4/IPv6 behavior, packet size,
   reconfiguration and failure handling.
5. **SIP/controller contract.** Map CMSIP records and call state transitions;
   capture the generated PJSUA configuration safely without publishing account
   credentials. Determine the minimum call lifecycle and feature set.
6. **Integration and test.** Determine whether the device ioctls and vendor
   management APIs are essential, and design process startup/restart and
   configuration interfaces for the intended host environment.

## Suggested validation milestones

Build repeatable offline fixtures from legally obtained captures and redacted
configuration, then use an isolated test setup for live checks:

- Encoder/decoder tests for Ethernet checksum, framing and DSP/CMSIP records,
  including partial Unix-stream reads and reconnects.
- Host-side handshake and remote readiness; idle on-hook/off-hook events and
  ring control, without relying on process timing alone.
- SIP registration, inbound and outbound single calls, bidirectional RTP and
  RTCP, hangup from both ends, DTMF and correct resource teardown.
- Both configured FXS lines, concurrent calls, hold/resume, codec negotiation,
  restart/recovery and IPv6 if those are target requirements.

Voicemail, fax, caller ID, call transfer, supplementary services, emergency
calling, management/UI integration and NAT behavior remain separate parity
targets; none is demonstrated by this first pass. Hardware testing is
necessary before declaring compatibility.
