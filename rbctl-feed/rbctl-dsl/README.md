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
rbctl-dsl [--config-iface <iface>] [--selftest] [--sniff]
```

| Flag | Description |
|------|-------------|
| `-i, --config-iface <iface>` | Management VLAN interface (default: `lan0.500`) |
| `--selftest` | Exercise socket + VLAN + board opcodes, capture frames, exit |
| `--sniff` | Passive listener for `0x88B5`/`0x88B6` frames |

### Selftest

Brings up the interface, creates/deletes a test VLAN, sends all board opcodes,
and captures TX/RX frames to `/tmp/rbctl-capture/`:

```sh
rbctl-dsl --selftest --config-iface lan0.500
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
