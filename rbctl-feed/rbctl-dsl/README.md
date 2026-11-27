# rbctl-dsl

OpenWrt package for the EcoNet xDSL board configuration daemon. Communicates
with the board over raw Ethernet (`0x88B5`) and manages transport VLAN
interfaces via netlink.

**Status:** validated on the VX830v with board-side service VLAN tagging and a working PPPoE session. This daemon handles xDSL; the same [OpenWrt feed](../README.md) separately provides the [two-port VoIP service](../rbctl-voip/README.md).

## Build

```sh
# In the OpenWrt buildroot:
make package/feeds/rbctl/rbctl-dsl/compile V=s
```

Output: `bin/packages/<arch>/rbctl/rbctl-dsl-<version>.apk`

## Runtime dependencies

The package recipe declares `+libuci +libnl-tiny`. The ubus stack is pure Rust
(`ubus-zero`) — no `libubus.so` needed.

## Usage

```
rbctl-dsl <command> [options]
```

The CLI is subcommand-based. Without a running daemon, `selftest` and `sniff`
are the useful modes; `status` / `reload` / `restart-line` / `stop` talk to a
running daemon over its IPC socket.

| Command | Description |
|---------|-------------|
| `daemon` | Start the configuration daemon (foreground) |
| `status` | Print live line state and metrics from the running daemon |
| `reload` | Tell the running daemon to reload UCI config |
| `restart-line` | Bounce the DSL line (down then up) on the running daemon |
| `stop` | Shut down the running daemon |
| `selftest` | Exercise socket + VLAN + board opcodes, then exit |
| `sniff` | Passive listener for `0x88B5` / `0x88B6` frames |

Common options for `daemon` / `selftest` / `sniff`:

| Option | Description |
|--------|-------------|
| `-i, --config-iface <iface>` | Management VLAN interface (default: `lan0.500`) |

`daemon` additionally accepts `--notify <path>`, `--syslog`, and UCI overrides
(`--annex`, `--line-mode`, `--tone`, `--xfer-mode`, `--vpi`, `--vci`,
`--encaps`, `--payload`, `--bitswap`, `--sra`, `-t/--transport-vlan`,
`--link-index`, `--link-vid`, `--link-pbit`). The optional
`--nosignal-watchdog` flag enables a one-per-episode warning when the line
remains continuously silent for 30 seconds; it is disabled by default and
never changes line state or sends board commands. The init script enables it
at boot when `network.dsl.nosignal_watchdog` is set to `1`:

```
config dsl 'dsl'
    option nosignal_watchdog '1'
```

### Data links (UCI)

Data links are named `network.<name>=ptm|atm` sections — one per link,
up to 8, each with a stable explicit `index` (transport VLAN =
`2000 + index`), an optional service `vid`/`pbit` **applied by the
board** on the PTM bearer, and ATM PVC params for `atm` sections:

```
config ptm
  option index '0'
  option vid  '100'   # service VLAN tagged board-side; absent = untagged
```

With no sections, a single implicit untagged PTM link at index 0 is
provisioned. Links are queued while the line is down and materialized
by the board at showtime (the OEM flow — validated end-to-end by the
E2E harness and on-target with a working PPPoE session). Host-side
transport VLANs remain down until the line reaches showtime.

### Selftest

Brings up the interface, creates/deletes a test VLAN, sends all board opcodes,
and logs the responses:

```sh
rbctl-dsl selftest --config-iface lan0.500
```

### Sniff

Live frame inspector: opens the interface with `ETH_P_ALL` and prints a
hex+ascii dump of every `0x88B5` / `0x88B6` frame, tagged `IN`/`OUT` by
direction. Run until interrupted with Ctrl+C:

```sh
rbctl-dsl sniff --config-iface lan0.500
```

Optionally write captured frames to a pcap file (`LINKTYPE_ETHERNET`,
readable directly by Wireshark / tcpdump):

```sh
rbctl-dsl sniff -i lan0.500 --dump /tmp/rbctl.pcap
```

## Workspace layout

```
crates/
├── rbctl_proto/     Protocol core (no_std, pure Rust)
├── af_packet/       AF_PACKET raw Ethernet socket (libc-only)
├── tinyln_rs_sys/   bindgen FFI for OpenWrt libnl-tiny
├── tinyln_rs/       Safe wrapper for libnl-tiny (socket, msg, attr, rtnl, genl)
└── rbctl_dsl/       Daemon binary (board controller, selftest, transport)
```
