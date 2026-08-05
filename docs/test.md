# On-Device Testing Guide

This document describes how to deploy and test `rbctl-dsl` on real OpenWrt
hardware (the MediaTek MT7986 / Filogic board with the EcoNet xDSL daughter
card).

## Prerequisites

### Build the binary

```bash
cd ~/Documenti/Progetti/openwrt
make package/rbctl-dsl/compile V=s
```

The binary and `.apk` package are produced at:

```
bin/packages/aarch64_cortex-a53/rbctl/rbctl-dsl-0.1.0-r1.apk
```

The raw ELF binary (for `scp` transfer) is at:

```
build_dir/target-aarch64_cortex-a53_musl/rbctl-dsl-0.1.0/ipkg-aarch64_cortex-a53/rbctl-dsl/usr/bin/rbctl-dsl
```

### Identify the target

The real hardware runs an **OpenWrt snapshot from a few weeks ago**. Our binary
is built against **today's snapshot** staging dir. The key shared libraries the
binary links are:

| Library | SONAME | Notes |
|---------|--------|-------|
| `libc.so` | musl ld-musl-aarch64 | ABI-stable across snapshot versions |
| `libuci.so` | `libuci.so.20250120` | **Version-dated** — mismatch possible |
| `libubox.so` | `libubox.so.20260708` | **Version-dated** — mismatch possible |
| `libnl-tiny.so` | `libnl-tiny.so.1` | SOVER 1, generally compatible |

> **Library mismatch risk:** If the target's snapshot is older, the dated
> SONAMEs (`libuci.so.20250120`, `libubox.so.20260708`) may not exist on it.
> The binary will fail to start with `error while loading shared libraries`.
>
> **Workaround:** Either (a) `scp` the staging-dir `.so` files to `/tmp` on the
> target and set `LD_LIBRARY_PATH=/tmp`, or (b) build `rbctl-dsl` against the
> **exact same snapshot** the target is running (check `cat /etc/openwrt_release`
> on the target, match the commit in the SDK).
>
> Option (b) is strongly preferred for a clean test.

---

## Step 1: Copy the binary to the target

### Option A: SCP (recommended — requires network access)

```bash
# From the host, assuming the target is reachable at 192.168.1.1
scp build_dir/target-aarch64_cortex-a53_musl/rbctl-dsl-0.1.0/ipkg-aarch64_cortex-a53/rbctl-dsl/usr/bin/rbctl-dsl root@192.168.1.1:/tmp/rbctl-dsl
```

On the target:

```sh
chmod +x /tmp/rbctl-dsl
```

### Option B: APK install (requires matching snapshot)

If the target runs the same snapshot:

```bash
scp bin/packages/aarch64_cortex-a53/rbctl/rbctl-dsl-0.1.0-r1.apk root@192.168.1.1:/tmp/
```

On the target:

```sh
apk add --allow-untrusted /tmp/rbctl-dsl-0.1.0-r1.apk
# Binary installed to /usr/bin/rbctl-dsl
```

### Option C: Staging .so files (if library mismatch)

If the target has older library SONAMEs:

```bash
# Copy the staging-dir libraries alongside the binary
scp staging_dir/target-aarch64_cortex-a53_musl/usr/lib/libuci.so.20250120 root@192.168.1.1:/tmp/
scp staging_dir/target-aarch64_cortex-a53_musl/usr/lib/libubox.so.20260708 root@192.168.1.1:/tmp/
scp staging_dir/target-aarch64_cortex-a53_musl/usr/lib/libnl-tiny.so.1 root@192.168.1.1:/tmp/
```

On the target, run with:

```sh
LD_LIBRARY_PATH=/tmp /tmp/rbctl-dsl --selftest --config-iface lan0.500
```

---

## Step 2: Prepare the management VLAN interface

The real hardware should already have `lan0.500` (or equivalent — check with
`ip link show`). If it doesn't exist:

```sh
# Find the parent interface (usually lan0 or eth0)
ip link show lan0 2>/dev/null || ip link show eth0

# Create the management VLAN if missing
ip link add link lan0 name lan0.500 type vlan id 500
ip link set lan0.500 up
```

Verify:

```sh
ip addr show lan0.500
cat /sys/class/net/lan0.500/address    # Should show the host MAC
```

---

## Step 3: Run the selftest

```sh
/tmp/rbctl-dsl --selftest --config-iface lan0.500
```

Expected output when the xDSL board is **connected and powered**:

```
[selftest] config interface: lan0.500
[selftest] socket:  PASS bound lan0.500, MAC [...], BPF 0x88B5, 500ms timeout
[selftest] vlan:    PASS create→up→down→del lan0.2001 (parent idx N, all OK)
[selftest] board:   PASS board responded! status=Up; N frames captured to /tmp/rbctl-capture/
[selftest] uci:     PASS context OK; ...
[selftest] uloop:   PASS uloop_init OK
[selftest] 5 passed, 0 failed
```

Expected output when the xDSL board is **not connected**:

```
[selftest] board:   PASS no response (expected); 11 frames captured to /tmp/rbctl-capture/
```

> The selftest always exits 0 on full pass, 1 on any failure.

### With the real board: increase the timeout

The default selftest timeout is 500 ms per retry with 1 retry. For the real
board (which may take longer to respond), use the probe mode instead:

```sh
/tmp/rbctl-dsl --config-iface lan0.500
```

This uses 2 s timeout with 3 retries — matching the original `remote_board`
binary's behaviour.

---

## Step 4: Copy captured frames back for analysis

The selftest writes raw frame captures to `/tmp/rbctl-capture/` on the target.
Copy them back to the host:

```bash
# On the host:
mkdir -p ~/projects/openwrt_tplink_vx830v_rbctrl-dsl/captures/
scp 'root@192.168.1.1:/tmp/rbctl-capture/*' ~/projects/openwrt_tplink_vx830v_rbctrl-dsl/captures/
```

### Files to look for

| File | Content | RE value |
|------|---------|----------|
| `tx-02-*.bin` | Opcode 2 (get_line_obj) request frame | Verify payload_type, seq, checksum |
| `rx-02-*.bin` | Opcode 2 **response** from real board | **Critical:** compare against `docs/unpack.md` field layout |
| `tx-05-*.bin` | Opcode 5 (atm_link_add) request frame | Verify ATM link descriptor encoding |
| `rx-05-*.bin` | Opcode 5 **response** | **Critical:** VLAN id at payload offset `0x12` |
| `tx-01-*.bin` | Opcode 1 (line_config_up) request | Verify modulation/annex/profile encoding |
| `rx-01-*.bin` | Opcode 1 **response** | Status byte |
| `hexdump.txt` | Human-readable hex+ASCII dump of all frames | Quick visual inspection |

### If the board responds

The `rx-*.bin` files contain real board responses. These are **gold** for
validating the RE documentation:

1. **Compare field offsets** in `rx-02-*.bin` against `docs/unpack.rs` /
   `examples/unpack.py`. If the board's response doesn't match, the RE has
   errors and the parser needs updating.

2. **Extract the transport VLAN id** from `rx-05-*.bin`:
   ```bash
   # VLAN id is at offset 0x18 (HEADER_LEN) + 1 (payload_type) + 0x12 = 0x2B
   xxd -s 0x2B -l 2 rx-05-seq*-try0.bin
   ```
   This should be a 2-byte big-endian value (e.g. `07d1` = 2001).

3. **Verify checksum algorithm** by running `python3 examples/checksum.py`
   against the captured frame bytes.

### If the board does NOT respond

- Check physical connectivity (Ethernet cable between host and xDSL board)
- Check that the management VLAN (500) is correct for this hardware
- Try `tcpdump -i lan0.500 -e -X` to see if the board sends any frames at all
- The `tx-*.bin` files are still valid — compare them against the original
  `remote_board` binary's frames (capture with tcpdump while running the
  original binary for comparison)

---

## Important notes about real hardware

### Snapshot version mismatch

The binary is built against today's `staging_dir`. The target hardware may be
running a snapshot from weeks ago. Symptoms of mismatch:

| Symptom | Cause | Fix |
|---------|-------|-----|
| `cannot open shared object file: No such file or directory` | Dated SONAME not found | Copy staging `.so` to `/tmp`, use `LD_LIBRARY_PATH` |
| `version `GLIBC_2.x' not found` | musl version skew | Rebuild against target's SDK commit |
| `bus error` / segfault on startup | ABI mismatch in libuci or libubox | Rebuild against matching snapshot |
| Socket/VLAN operations return unexpected errno | Kernel API change | Extremely unlikely for stable rtnetlink UAPI |

### Management VLAN ID

The default management VLAN is **500** (`lan0.500`). Some hardware revisions
may use a different VLAN. Check the original `remote_board` configuration:

```sh
# On the target, check the existing config
ps | grep remote_board          # See what interface it uses
cat /etc/config/network | grep -A5 dsl   # Check UCI dsl section
```

### CAP_NET_RAW and CAP_NET_ADMIN

The daemon requires:
- **CAP_NET_RAW** — to open AF_PACKET sockets
- **CAP_NET_ADMIN** — to create/delete VLAN interfaces via netlink

Running as root satisfies both. If running as a non-root user, ensure the
binary has these capabilities:

```sh
setcap cap_net_raw,cap_net_admin+ep /tmp/rbctl-dsl
```

### Existing remote_board process

If the proprietary `remote_board` daemon is still running, it will conflict
(both try to bind to `0x88B5` on the same interface). Stop it first:

```sh
/etc/init.d/remote_board stop    # or: killall remote_board
```

### LED behaviour

The selftest does **not** drive LEDs. On real hardware, the original
`remote_board` controls LEDs via GPIO. Our daemon will add LED support in
Phase 3 (hotplug emitter + `dsl_notify.sh`).

### Recovery

If the selftest creates a VLAN interface that fails to clean up:

```sh
ip link delete lan0.2001    # Manual cleanup
```

The selftest tries to delete stale interfaces from previous runs at the
start of the VLAN lifecycle test, so this should be rare.

---

## Quick reference

```bash
# Build
cd ~/Documenti/Progetti/openwrt && make package/rbctl-dsl/compile V=s

# Deploy
scp build_dir/target-aarch64_cortex-a53_musl/rbctl-dsl-0.1.0/ipkg-aarch64_cortex-a53/rbctl-dsl/usr/bin/rbctl-dsl root@TARGET_IP:/tmp/rbctl-dsl

# Test (on target, as root)
chmod +x /tmp/rbctl-dsl
/tmp/rbctl-dsl --selftest --config-iface lan0.500

# Retrieve captures
scp 'root@TARGET_IP:/tmp/rbctl-capture/*' ~/projects/openwrt_tplink_vx830v_rbctrl-dsl/captures/
```
