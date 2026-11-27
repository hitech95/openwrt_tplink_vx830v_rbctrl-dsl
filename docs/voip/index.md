# AArch64 VoIP — analysis index

The [OpenWrt voice service](../../rbctl-feed/rbctl-voip/README.md) replaces the VX830v master-side telephony stack and drives both analog ports on the stock board. For current behavior, start with the [feature status](feature-status.md) and [on-device test results](test-report.md). The OEM analysis below documents the protocol evidence behind the implementation.

## Documents

| Document | Contents |
|---|---|
| [Feature status and OEM comparison](feature-status.md#stock-firmware-oem-vs-openwrt) | What works on OpenWrt, what stock firmware exposes, and which features are still missing or unverified |
| [architecture.md](architecture.md) | Processes, startup, socket ownership, dependencies, and scope |
| [master-proxy.md](master-proxy.md) | Master proxy, local DSP records, raw Ethernet and RTP relay |
| [board-link.md](board-link.md) | Host-derived board protocol, checksum vectors, DSP command inventory and hardware conformance gate |
| [Offline QEMU checks](board-link.md#reproducible-offline-checks-no-oem-shell-needed) | Execute the original AArch64 client and proxy routines against local socket pairs |
| [Stock proxy worker transcript](worker-transcript.md) | Normal proxy startup, two-line control and media, RTCP observation, retries and local reattachment under QEMU |
| [DTMF](dtmf.md) | Handset key encoding, digit collection vs in-call signaling, and QEMU evidence for CMSIP SIP INFO |
| [Software-generated tones](software-tones.md) | Production ATA waveforms, call-state triggers, direction, and regional limitations |
| [Controller startup and outgoing calls](controller-startup.md) | DSP reply ordering, channel affinity and open record, dial tone/first digit, and SIP/media serializers |
| [cmsip.md](cmsip.md) | Modified PJSUA, local CMSIP protocol, and disclosed source |
| [map.md](map.md) | Binary inventory, Ghidra project paths, addresses and provenance |
| [replacement.md](replacement.md) | Replacement boundaries, research milestones and validation plan |

## Starting points

For the replacement implementation, start with the [Econet peer findings](peer.md), [library design and API](foss-design.md), and [milestone test results](test-report.md).

**Scope of the static review:** all ten documents listed here (including this
index, with the offline-checks row pointing into `board-link.md`) were checked;
for `replacement.md` the review covered factual statements, not design choices.
In `voip_client` it covered main, the select thread, the DSP and SIP dispatchers,
the 21 callers of `4134c8`, `424d24`, the `418c18` dispatch arms, relevant
regions of `42af24`, and the command-name and DTMF tables. In the proxy it
covered main, workers, request/reply/event paths, CRC, socket filters, relay,
name table and globals. Other evidence was `cos:406398`, `voip.ko:init_module`,
PJSUA strings and metadata, scripts, disclosed sources and existing captures.

The reviewer did not inspect or execute the QEMU fixtures. Their transcripts
are separate evidence. The module ioctl handlers, full PJSUA implementation,
vendor data-model/provisioning mappings and `remote_board` internals (beyond
its hash) were outside the review.

The host runs `voip_client`, `voip_server_proxy` and the vendor-modified
`pjsua`. The first two connect over a local Unix socket. `pjsua` connects to
`voip_client` over a different Unix socket; the proxy also communicates with
the external voice hardware over `lan0.500`. The local SIP control protocol and
the hardware-facing protocol are separate. See [architecture.md](architecture.md).

The checked-in vendor GPL tree contains PJSIP 1.10-based source with CMSIP
integration. It is an important reference, but has not yet been shown to
reproduce the shipped binary. The exact build configuration and licensing of
individual sources and dependencies still need verification.

Other project work: [main documentation index](../index.md),
[remote-board protocol](../protocol.md), and [DSL control architecture](../architecture.md).
