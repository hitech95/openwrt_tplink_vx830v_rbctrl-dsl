#!/bin/bash
# inner.sh — runs INSIDE the harness namespace (entered by run.sh).
# Sets up: fake /proc (tmpfs with tc3162 files), private sysfs+run, the
# veth + stacked-VLAN wire, shell stubs, the aarch64 daemon sysroot.
# Then execs the scenario driver (e2e.py).
#
# Full documentation: docs/e2e.md
# Required env (see e2e.md §2): ECONET_ROOTFS, OPENWRT_ROOT; optional
# DAEMON_BIN. No personal paths are hardcoded — set them or use env.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"

: "${ECONET_ROOTFS:?ECONET_ROOTFS must point to the extracted OEM rootfs (see e2e.md §2)}"
: "${OPENWRT_ROOT:?OPENWRT_ROOT must point to an OpenWrt SDK/build tree (see e2e.md §2)}"
OPENWRT_STAGING="$OPENWRT_ROOT/staging_dir/target-aarch64_cortex-a53_musl"
if [ -z "${DAEMON_BIN:-}" ]; then
  # newest built rbctl-dsl ipkg binary in the SDK build tree
  DAEMON_BIN="$(ls -1d "$OPENWRT_ROOT"/build_dir/target-aarch64_cortex-a53_musl/rbctl-dsl-*/ipkg-aarch64_cortex-a53/rbctl-dsl/usr/sbin/rbctl-dsl 2>/dev/null | sort -V | tail -1)"
fi
[ -x "$DAEMON_BIN" ] || { echo "daemon binary not found/executable: $DAEMON_BIN (set DAEMON_BIN; see e2e.md §2)" >&2; exit 1; }
[ -d "$ECONET_ROOTFS/userfs/bin" ] || { echo "ECONET_ROOTFS does not look like the OEM rootfs (no userfs/bin)" >&2; exit 1; }

LOGDIR="$(mktemp -d /tmp/rbctl-e2e.XXXXXX)"
AROOT="$LOGDIR/aroot"
STUBBIN="$LOGDIR/stubs"

# ── filesystems: private sysfs (netns-aware /sys/class/net), fake /proc ──
mount -t sysfs sysfs /sys
mount -t tmpfs tmpfs /proc
mount -t tmpfs tmpfs /run
# Fake DSL-driver proc interface (the real one comes from tc3162 kernel
# modules probing EN7516 hardware qemu cannot model). Writes to these
# files are captured losslessly via strace (see e2e.py); reads are state
# injection — the harness rewrites the file, the board's next fopen sees it.
mkdir -p /proc/tc3162
: > /proc/tc3162/tcci_cmd
: > /proc/tc3162/vlan_tag_sw
: > /proc/tc3162/xdsl_stats
printf 'ADSL link status: down\nOpmode: Wrong opmode\nAdsl Type: \n' > /proc/tc3162/adsl_stats
printf 'mt7510ptm 12345 0 - Live 0x00000000\n' > /proc/modules
printf 'CurrentProfiles: 17a\n' > /proc/tc3162/vdsl_interface_config

# ── the wire: veth + stacked VLANs (board binds the literal eth0.1.500) ──
ip link add board0 type veth peer name lan0
ip link set board0 name eth0
ip link set eth0 address 00:aa:bb:01:23:48
ip link add link eth0 name eth0.1   type vlan id 1
ip link add link eth0.1 name eth0.1.500 type vlan id 500
ip link add link lan0 name lan0.1   type vlan id 1
ip link add link lan0.1 name lan0.1.500 type vlan id 500
for i in eth0 eth0.1 eth0.1.500 lan0 lan0.1 lan0.1.500; do ip link set "$i" up; done

# Dummy PTM interface: the board's vdsl_linkAdd gates the smuxctl/vconfig
# materialization on if_nametoindex("nas8") != 0 (on real hw the mt7510ptm
# driver creates nas8 when the line trains). Without it the op15 handler
# bails with "interface nas8 is not ready" and board_exec.log never sees
# the data-plane commands.
ip link add nas8 type dummy 2>/dev/null || true
ip link set nas8 up

# Pre-create every interface ptmCreateVLAN(pvc) polls for (50 x 5 ms via
# waitIntfExists): "nas8_<pvc>", "eth0.1.<2000+pvc>", "br<pvc+2>" for
# pvc 0..7. On real hw the board's own vconfig/brctl/smuxctl create them;
# under the harness those are ENOENT absolute paths or no-op stubs, so
# nothing would materialize — ptmCreateVLAN would return 1, vdsl_linkAdd
# with it, the handled flag would never be set and op16 would silently
# skip vdsl_linkDel (no smuxctl rem, l_vidInfo never freed). Verified
# against ptmCreateVLAN @ 0x00406094 + ptm_build_ifnames @ 0x00405de8.
for p in 0 1 2 3 4 5 6 7; do
  ip link add "nas8_$p" type dummy 2>/dev/null || true
  ip link set "nas8_$p" up
  ip link add link eth0.1 name "eth0.1.$((2000+p))" type vlan id $((2000+p)) 2>/dev/null || true
  ip link set "eth0.1.$((2000+p))" up
  ip link add "br$((p+2))" type bridge 2>/dev/null || true
  ip link set "br$((p+2))" up
done

# ── shell stubs: satisfy the binaries' system() calls, record them ──
mkdir -p "$STUBBIN"
for t in ifconfig brctl vconfig smuxctl route; do
  printf '#!/bin/sh\necho "%s $*" >> "%s/board_sys.log"\nexit 0\n' "$t" "$LOGDIR" > "$STUBBIN/$t"
  chmod +x "$STUBBIN/$t"
done

# ── aarch64 daemon sysroot (musl loader + uci/ubox/nl-tiny + the binary) ──
mkdir -p "$AROOT/lib" "$AROOT/usr/lib" "$AROOT/usr/sbin" "$AROOT/etc/config"
cp -a "$OPENWRT_ROOT/staging_dir/target-aarch64_cortex-a53_musl/root-mediatek/lib/." "$AROOT/lib/"
cp -a "$OPENWRT_STAGING/usr/lib/"*.so* "$AROOT/usr/lib/" 2>/dev/null || true
cp -a "$OPENWRT_STAGING/usr/lib/libgcc_s.so"* "$AROOT/usr/lib/" 2>/dev/null || true
cp "$DAEMON_BIN" "$AROOT/usr/sbin/rbctl-dsl"
# minimal UCI fixture: default dsl behaviour is fine; file may stay absent.

export ECONET_ROOTFS AROOT LOGDIR STUBBIN
echo "[e2e] logdir: $LOGDIR"
exec python3 "$HERE/e2e.py"
