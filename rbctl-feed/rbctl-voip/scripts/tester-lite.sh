#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
set -eu
cd "$(dirname "$0")"
base=$(pwd -P)
case "${1:-check}" in check|install) ;; *) exit 2;; esac
[ "$(cat /tmp/sysinfo/board_name)" = tplink,vx830v ] && [ "$(cat /etc/apk/arch)" = aarch64_cortex-a53 ] || { echo 'Wrong board or ABI'; exit 1; }
if apk info -e rbctl-voip >/dev/null; then echo 'Full voice package installed; do not mix editions.'; exit 1; fi
sha256sum -c SHA256SUMS >/dev/null
run_apk() {
    apk --no-network --keys-dir "$base/keys" --repositories-file /dev/null -X "$base/packages.adb" "$@"
}
run_apk add --simulate rbctl-voip-lite=0.1.0-r2
df -h /overlay
[ "${1:-check}" = install ] || exit 0
[ "$(df -Pk /overlay | awk 'END {print $4}')" -ge 4096 ] || { echo 'Need 4 MiB free overlay for lite.'; exit 1; }
run_apk add rbctl-voip-lite=0.1.0-r2
ubus -t 15 wait_for voip && ubus call voip status
