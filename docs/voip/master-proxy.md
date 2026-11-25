# AArch64 master VoIP proxy

[VoIP index](index.md) · [Architecture](architecture.md) · [Symbol map](map.md) · [Replacement plan](replacement.md)

## Local DSP side

See the updated [board-link specification](board-link.md) for the numeric DSP command table, payload fields and conformance status.

`voip_server_proxy` listens on the Unix stream socket
`/var/tmp/sock_CM_DSP`; `voip_client` connects to it. Proxy `main` accepts one
client, starts a message-processing thread, and closes additional connections.
The client select loop reads **0x84 (132) bytes** per DSP record, and the proxy
reads the same size from its client. The proxy forwards such records in a
request with subtype `0x16` and payload length `0x84` using its outbound raw
control path. Client `voip_process_dsp_msg` uses `ntohl` on the message type
and selected words, so at least those fields use network byte order.

The binary contains command names including `VOIP_DSP_START`,
`VOIP_SLIC_LOCALE`, `VOIP_RING_START`, `VOIP_RING_STOP`, `VOIP_CHANNEL_OPEN`,
`VOIP_RTP_ACTIVE`, `VOIP_RTP_UPDATE`, `VOIP_RTP_DEACTIVE`, `VOIP_HOOK_ON`,
`VOIP_HOOK_OFF`, `VOIP_DTMF_START` and `VOIP_DTMF_END`. The numeric mappings
in the board-link specification come from integer/pointer pairs, not string order.

## Raw Ethernet endpoints

Proxy code creates `PF_PACKET` / `SOCK_RAW` sockets, attaches a BPF filter,
and binds them to the literal interface name `lan0.500`. `htons()` is used on
the protocol passed to `socket()`:

| EtherType | Observed host-side use | Evidence |
|---|---|---|
| `0xDD01` | Outbound request/response path | request initializer at `0x00402028`; subtype `0x1a` for initialization and `0x16` for forwarded DSP records |
| `0xDD02` | Control receive loop | socket initializer at `0x00402cb8` |
| `0xDD03` | RTP/RTCP relay path | relay initializer at `0x00405da0` and frame sender at `0x00403a78` |

These are distinct from the `0x88B5`/`0x88B6` documented for general
`remote_board`/cmm. They must be captured and validated on the actual device;
the values above come from host code, not packet captures.

### Observed frame fields

The host's sender builds an Ethernet header followed by a 10-byte custom
header. The following offsets are relative to the Ethernet frame as the host
constructs it, without an 802.1Q header inserted by the VLAN device:

| Offset | Bytes | Observation |
|---:|---:|---|
| `0x00` | 6 | Destination MAC; media receiver learns a peer MAC when initialized to broadcast. |
| `0x06` | 6 | Source MAC from the bound interface. |
| `0x0c` | 2 | EtherType, network byte order. |
| `0x0e` | 1 | Value `0x11` written by request/media senders; meaning not established. |
| `0x0f` | 1 | Subtype: for example `0x16`, `0x1a`, or media `0x19`. |
| `0x10` | 4 | Request initializer writes an incremented sequence in network order; media sender writes zero. |
| `0x14` | 2 | Payload length, network byte order. |
| `0x16` | 2 | Checksum in network byte order. |
| `0x18` | variable | Payload. |

The checksum routine clears the checksum field and runs a nibble-table
algorithm over the custom header and payload starting at `0x0e`, length
`payload_length + 10`. The table matches CRC-16/ARC; see the offline vectors
in the board-link specification. The outbound request path pads frames shorter than 60
bytes; padding is outside the indicated payload. Verify VLAN/offload effects,
checksums and MAC learning with captures before using this as a wire spec.

The initialization thread sends subtype `0x1a`, with a four-byte payload, and
uses the response-wait routine with a 1000 ms interval and retry count 120.
This is a static reading of arguments, not a measured boot-time guarantee.
DSP forwarding uses a 300 ms interval and retry count 2. The local forwarding
thread does not return transaction failures to the client. See the [board-link
contract](board-link.md) for the stock reply checks, queue overflow behavior
and distinct init failure states.

## RTP relay observations

`vsp_rtp_relay_init` prepares four channel slots and starts two threads. The
downstream thread reads UDP sockets and writes `0xDD03` frames. The media
payload begins with two 16-bit network-order values: channel index and a
selector. In this code selector `3` corresponds to the RTP socket and `4` to
the RTCP socket; the remaining payload is the received packet. The upstream
thread receives these frames and forwards payloads to the configured UDP
destinations. It bounds-checks the channel index against four slots.

The downstream loop reacts to 132-byte internal control records with decoded
command values 10, 11 and 12, managing per-channel RTP/RTCP sockets. Their
full struct layouts, semantic names, IPv6 details, port policy and behavior
on errors need verification before implementing a replacement. UDP media
handling is distinct from SIP signaling.
