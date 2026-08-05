# rbctl-dsl

Rust daemon that replaces the proprietary `remote_board` + `libcmm.so` stack
for managing an EcoNet xDSL board over a proprietary `0x88B5` Ethernet
protocol on mainline OpenWrt.

## Status

| Phase | Status | Description |
|-------|--------|-------------|
| 0 — Toolchain gate | **PASSED** | UCI + uloop + ubus link/load on aarch64-musl (QEMU verified) |
| 1 — Protocol core | **PASSED** | CRC-16/ARC, frame builder, pack/unpack for all opcodes (18/18 tests) |
| 2 — Board control | **PASSED** | AF_PACKET socket, netlink VLAN mgmt, Board struct with 8 opcodes (QEMU selftest 5/5) |
| 3 — OpenWrt integration | Pending | UCI config loader, ubus object, procd init, hotplug emitter |

## Repository layout

```
rbctl-feed/rbctl-dsl/          Cargo workspace (the OpenWrt package)
├── crates/
│   ├── rbctl_proto/           Protocol core (no_std, pure Rust)
│   ├── af_packet/             AF_PACKET raw Ethernet socket (libc-only)
│   ├── tinyln_rs_sys/         bindgen FFI for OpenWrt libnl-tiny
│   ├── tinyln_rs/             Safe Rust wrapper for libnl-tiny (9 modules)
│   └── rbctl_dsl/             Daemon binary (board.rs, main.rs, transport.rs)
├── Cargo.toml                 Workspace root
└── Makefile                   OpenWrt package definition (rust-package.mk)
docs/                          Reverse-engineering documentation
examples/                      Reference implementations (Python: checksum, pack, unpack)
plans/                         Implementation plan (phases, gates, architecture)
```

## Crates

### `rbctl_proto` — Protocol core

Pure Rust, `no_std`, zero C dependencies. Implements the `0x88B5` wire
protocol:

- CRC-16/ARC checksum (set + verify)
- 24-byte header frame builder (`build_command_frame`)
- TX encoders: opcodes 1 (line config up), 5 (ATM link add), 15 (PTM link
  add), 6/16 (link delete)
- RX decoders: opcodes 2 (line object — status, rates, SNR, attenuation), 4
  (channel stats)

### `af_packet` — Raw Ethernet socket

libc-only crate for `AF_PACKET` `SOCK_RAW` sockets:

- Bind to a specific interface (e.g. `lan0.500`)
- Classic BPF filter for ethertype matching (`0x88B5`)
- TX/RV with configurable timeout
- MAC address from `/sys/class/net/<iface>/address`

### `tinyln-rs` — libnl-tiny wrapper

Safe Rust wrapper around OpenWrt's `libnl-tiny` C library:

- `NlSocket` — netlink socket (alloc, connect, send, recv, ACK)
- `NlMsg` — message builder (alloc, append, reserve)
- `NlAttr` — typed attribute put/get, nesting, iteration
- `NlCb` — callback dispatch
- `RtnlLink` — interface management (VLAN create/delete, up/down)
- `RtnlAddr` / `RtnlRoute` — IP address and route management
- `GenlSocket` — generic netlink (family resolution, multicast)
- `Unl` — high-level micro-netlink helpers

### `rbctl_dsl` — Daemon

The binary that ties everything together:

- `Board<T: Transport>` — high-level controller with all 8 opcodes, sequence
  numbering, retransmission (3 retries × 2 s), checksum verification
- `--selftest` mode — exercises socket + VLAN lifecycle + board probe,
  captures all TX/RX frames to `/tmp/rbctl-capture/`
- Mock transport for unit testing (15 board tests)

## Building

Requires the [OpenWrt SDK](https://openwrt.org/docs/guide-developer/toolchain)
with a `aarch64_cortex-a53` (Filogic) or `aarch64_generic` (armsr) target.

```bash
# Clone the OpenWrt SDK
git clone https://git.openwrt.org/openwrt/openwrt.git
cd openwrt

# Configure for your target
./scripts/feeds update -a && ./scripts/feeds install -a
make menuconfig    # select Target (e.g. MediaTek Filogic) + rust/host

# Add the rbctl feed
echo 'src-link rbctl /path/to/openwrt_tplink_vx830v_rbctrl-dsl/rbctl-feed' >> feeds.conf
./scripts/feeds update rbctl && ./scripts/feeds install rbctl-dsl

# Build
make package/rbctl-dsl/compile V=s
```

Output: `bin/packages/<arch>/rbctl/rbctl-dsl-0.1.0-r1.apk`

## Testing

### Host-side tests (no SDK needed)

```bash
cd rbctl-feed/rbctl-dsl
cargo test -p rbctl_proto    # 18 tests — CRC, frame, pack/unpack
cargo test -p af_packet      # 10 tests — BPF, MAC parsing, ethertype
```

### QEMU selftest (on-device validation)

```bash
# Build the binary
make package/rbctl-dsl/compile V=s

# Run in QEMU (see rbctl-feed/rbctl-dsl/boot-qemu-test.sh for automation script)
# Or see docs/test.md for manual on-device testing instructions
```

The `--selftest` mode validates:
1. AF_PACKET socket open + BPF filter + interface bind
2. VLAN lifecycle via netlink (create → up → down → delete)
3. Board opcode round-trip (expects timeout without hardware)
4. UCI context and uloop initialization

All TX/RX frames are captured to `/tmp/rbctl-capture/` as `.bin` files with
a human-readable `hexdump.txt`.

### On real hardware

See [docs/test.md](docs/test.md) for the complete on-device testing guide,
including binary deployment, library mismatch handling, frame capture
retrieval, and RE validation workflow.

## Protocol

The daemon communicates with the EcoNet xDSL board using raw Ethernet frames
with a proprietary ethertype (`0x88B5`). See:

- [docs/protocol.md](docs/protocol.md) — frame layout, field offsets, BPF filter
- [docs/network.md](docs/network.md) — socket setup, VLAN handling, QinQ model
- [docs/checksum.md](docs/checksum.md) — CRC-16/ARC algorithm and covered region
- [docs/commands/dispatch.md](docs/commands/dispatch.md) — opcode dispatch table
- [docs/xdsl/](docs/xdsl/) — data plane, payload formats, ATM vs PTM

## License

GPL-2.0-only
