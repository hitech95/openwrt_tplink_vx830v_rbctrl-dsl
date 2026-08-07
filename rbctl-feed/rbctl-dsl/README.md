# rbctl-dsl

OpenWrt package for the EcoNet xDSL board configuration daemon. Communicates
with the board over raw Ethernet (`0x88B5`) and manages transport VLAN
interfaces via netlink.

## Build

```sh
# In the OpenWrt buildroot:
make package/rbctl-dsl/compile V=s
```

Output: `bin/packages/<arch>/rbctl/rbctl-dsl-<version>.apk`

## Runtime dependencies

The `.apk` declares `+libuci +libubox +libnl-tiny`. The ubus stack is pure Rust
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
`--encaps`, `--payload`, `--bitswap`, `--sra`, `-t/--transport-vlan`).

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

Optionally dump each captured frame as a raw `.bin` file (created under the
given directory, named `<n>-<in|out>-<ethertype>.bin`):

```sh
rbctl-dsl sniff -i lan0.500 --dump /tmp/rbctl-capture
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
