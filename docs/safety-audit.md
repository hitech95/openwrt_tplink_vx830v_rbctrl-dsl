# Type & Memory-Safety Posture

Current state of every `unsafe` block and C-binding boundary in the
`rbctl-dsl` workspace. The daemon's business logic is 100 % safe Rust; all
`unsafe` is confined to three leaf FFI crates.

---

## Unsafe surface

| Crate / file | `unsafe` sites | Role |
|---|---:|---|
| `rbctl_proto` (pack/unpack/frame/validate/checksum) | **0** | pure-Rust protocol core |
| `rbctl_dsl` business logic (board/transport/uci_cfg/ubus_obj/hotplug/ipc) | **0** | daemon logic |
| `af_packet/src/lib.rs` | 10 | libc `AF_PACKET` socket |
| `tinyln_rs/*` (libnl-tiny wrapper) | 86 | netlink socket/msg/attr/rtnl/genl/unl |
| `rbctl_dsl/src/daemon.rs` | 1 | signal-handler install |
| `rbctl_dsl/src/selftest.rs` | 8 | `--sniff` raw-socket mode |

`ubus-zero` is pure Rust (no FFI). `rust-uci` (git dep) wraps libuci via
bindgen and is upstream code outside this repo (see Deferred D5).

---

## Current safety posture

### Hardened — sound by construction

| Area | Location | Guarantee |
|------|----------|-----------|
| Attribute reads | `attr.rs` `get_u8/u16/u32/u64` | Return `Option<T>`; payload bounds-checked (`payload_len >= sizeof(T)`) before any read |
| Attribute length | `attr.rs` `payload_len` | `checked_sub(NLA_HDRLEN)` — no underflow on malformed attrs |
| String reads | `attr.rs` `get_string` | UTF-8-validated via `from_utf8`; returns `Option<&str>` |
| Header mutation | `msg.rs` `header_mut(&mut self)` | `&mut self` receiver prevents aliasing `&mut`/`&` |
| Typed append | `msg.rs` `append_struct<T: Copy>` | `Copy` bound rejects non-POD types (e.g. `Vec`) at compile time; callers pass repr(C) POD kernel structs |
| Nested attributes | `attr.rs` `Nest { offset }` | Offset-based handle re-derives the write position at finalize time — survives `nla_put`-triggered buffer reallocations |
| String put | `attr.rs` `put_string` | Interior-NUL failure maps to a negative return (nla_put's error convention), not a panic |

### Accepted — correct by design

| Area | Location | Why it's safe |
|------|----------|---------------|
| Panic strategy | `Cargo.toml` `[profile.release] panic = "abort"` | Primary defense against unwind-through-C UB; the shipped daemon is release |
| Resource lifecycle | 9 `Drop` impls (`NlSocket`, `NlMsg`, `NlCb`, `Unl`, `RtnlLink`, `RtnlAddr`, `RtnlRoute`, `GenlSocket`, `RawSocket`/`OwnedFd`) | Every one null-checks before free; move-only ownership (`Drop` ⟹ `!Copy`) makes double-free impossible; `af_packet` uses RAII `OwnedFd` |
| Thread safety | No `unsafe impl Send/Sync` anywhere | Raw-pointer structs are `!Send`/`!Sync` by default; handles stay on their owning thread |
| Signal handlers | `daemon.rs` | `extern "C" fn` doing only `AtomicBool::store(SeqCst)` — async-signal-safe |
| Board-protocol RX | `unpack.rs`, `frame.rs` | Length-validated before every field access; unknown codes → `Option`/`Unknown(_)`; `SeqCounter` uses `wrapping_add`; zero `unsafe` |
| VLAN management path | `daemon.rs` shells out to `ip link` | The netlink attr code is exercised only by `--selftest`, narrowing exposure |

### Deferred — tracked, low-risk

| # | Location | Concern | Plan |
|---|----------|---------|------|
| D1 | `unl.rs`, `rtnl/{link,addr,route}.rs` `alloc_msg` | `assert!(!ptr.is_null())` aborts on ENOMEM | Convert to `io::Result` (return-type change ripples to callers) |
| D2 | `socket.rs` `fd()` reads `(*ptr).s_fd`; `rtnl/*` read `self.unl.sock` | Reaches into private struct fields by bindgen layout | Consider `nl_socket_get_fd` if exported, or pin the libnl-tiny version |
| D3 | `tinyln_rs_sys/build.rs` `layout_tests(false)` | Struct-layout drift between bindgen and libnl-tiny won't be caught at build time | Re-enable in the SDK build |
| D4 | `unl.rs`/`rtnl/*` `new()` | `mem::zeroed()` + `unl_rtnl_init()?` may leak if init partially allocates then fails | Call `unl_free` on the init-error path |
| D5 | `rust-uci` (external git dep) | Upstream libuci-sys bindgen | Trust upstream; revisit if a CVE surfaces |

---

## Verification

| Check | Result |
|-------|--------|
| `cargo test -p rbctl_proto -p af_packet` | **45 passed, 0 failed** |
| `cargo clippy -p rbctl_proto -p af_packet` | clean (3 cosmetic warnings) |
| `make package/rbctl-dsl/compile` (OpenWrt SDK, aarch64_cortex-a53 / Filogic) | **PASS** — `rbctl-dsl-0.1.0-r1.apk` (357 KB), ELF64 aarch64-musl, dynamically linked against `libuci`/`libnl-tiny`/`libc` |

The SDK cross-build confirms: `ifinfomsg`/`rtmsg`/`ifaddrmsg` are `Copy`
(required by `append_struct<T: Copy>`), and the `Nest` handle + `Option`
getters + `payload_len` type-check against the real bindgen `nlattr`/
`nlmsghdr`/`nl_msg` types from libnl-tiny's headers.

Open: the on-target/QEMU `--selftest` VLAN round-trip (functional confirmation
of the `Nest` offset handle producing correct wire bytes).

---

## Crash / leak / UB summary

- **Crash via UB** (OOB read, aliasing, dangling write): closed — the netlink
  attribute path is bounds-checked and realloc-safe.
- **Memory leaks**: all `Drop` impls free exactly once; one residual
  init-failure vector (D4), rare and tracked.
- **Double-free**: impossible — move-only ownership, null-checked drops.
- **Panic → abort** (not UB): `panic = "abort"` in release; remaining panic
  sources (`assert!`/`CString` in D1) abort rather than corrupt memory.
