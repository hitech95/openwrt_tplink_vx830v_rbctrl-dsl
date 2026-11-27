#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
# Run inside the extracted VX830v tester bundle. Default: simulation only.
set -eu
cd "$(dirname "$0")"
base=$(pwd -P)
mode=${1:-check}
case "$mode" in check|install) ;; *) echo 'Usage: sh tester-apk.sh [check|install]' >&2; exit 2;; esac
[ "$(cat /tmp/sysinfo/board_name)" = tplink,vx830v ] && [ "$(cat /etc/apk/arch)" = aarch64_cortex-a53 ] || { echo 'Different target; rebuild for this firmware.' >&2; exit 1; }
for abi in libubox20260721 libubus20260628; do apk info -e "$abi" >/dev/null || { echo "Missing firmware ABI: $abi" >&2; exit 1; }; done
sha256sum -c SHA256SUMS >/dev/null
tmp=$(mktemp -d /tmp/voip-apk.XXXXXX); trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/keys" "$tmp/cache"
cp /etc/apk/keys/*.pem keys/rbctl-test.pem "$tmp/keys/"
bundle_apk() {
    apk --no-network --cache-dir "$tmp/cache" --keys-dir "$tmp/keys" --repositories-file /dev/null \
        -X "$base/local/packages.adb" -X "$base/openwrt/base/packages.adb" \
        -X "$base/openwrt/packages/packages.adb" -X "$base/openwrt/telephony/packages.adb" "$@"
}
bundle_apk add --simulate rbctl-voip-full=0.1.0-r4
df -h /overlay
[ "$mode" = install ] || exit 0
# Full set is ~10 MiB before filesystem overhead. Avoid exhausting rootfs
# even on a fresh tester device; small-overlay boards need extroot or an image.
free=$(df -Pk /overlay | awk 'END {print $4}')
[ "$free" -ge 12288 ] || { echo 'Install skipped: need 12 MiB free on /overlay for this bundle.' >&2; exit 1; }
bundle_apk add rbctl-voip-full=0.1.0-r4
ubus -t 15 wait_for voip && ubus call voip status
