#!/bin/bash
# run.sh — E2E harness entry point.
#
# Full documentation: docs/e2e.md
#
# Prerequisites (no exotic deps): qemu-user (qemu-mips, qemu-aarch64),
# iproute2, python3, strace, and unprivileged user+net+mount namespaces.
# Required env: ECONET_ROOTFS (extracted OEM rootfs), OPENWRT_ROOT (SDK
# build tree) — or put them in tests/e2e/env (sourced, gitignored).
#
# Usage:
#   tests/e2e/run.sh                  # default scenarios: E1 E2 E3 E5 E6 E7 E8
#   SCENARIOS='E1 E2' tests/e2e/run.sh   # subset
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"

# Personal environment (not committed) — see e2e.md §2.
# shellcheck source=/dev/null
[ -f "$HERE/env" ] && . "$HERE/env"

command -v qemu-mips    >/dev/null || { echo "qemu-mips missing (apt install qemu-user)"; exit 1; }
command -v qemu-aarch64 >/dev/null || { echo "qemu-aarch64 missing (apt install qemu-user)"; exit 1; }
command -v unshare >/dev/null || { echo "unshare missing"; exit 1; }
command -v strace  >/dev/null || { echo "strace missing"; exit 1; }
[ -n "${ECONET_ROOTFS:-}" ] || { echo "ECONET_ROOTFS not set (see e2e.md §2)"; exit 1; }
[ -n "${OPENWRT_ROOT:-}" ]  || { echo "OPENWRT_ROOT not set (see e2e.md §2)"; exit 1; }

unshare -Urnm --fork --kill-child bash "$HERE/inner.sh" "$@"
