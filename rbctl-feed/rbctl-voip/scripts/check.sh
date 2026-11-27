#!/bin/sh
# SPDX-License-Identifier: GPL-2.0-or-later
# Host checks for M2.1 (T2.10–T2.15, T2.18). Run from the workspace root.
# Optional tools are skipped with a message: cargo-deny, cbindgen,
# qemu-aarch64 + extracted OEM rootfs (T2.13).
set -eu
cd "$(dirname "$0")/.."

echo "== T2.10 build + tests (incl. golden vectors, proptest, CLI fixture, C smoke)"
cargo build --workspace --locked
cargo test --workspace --locked --features rbctl_voicectl/fixture

echo "== T2.11 format, lint, licences"
# Plain `cargo fmt` (no --all): --all also walks local path dependencies,
# i.e. af_packet and the whole rbctl-dsl workspace, which uses its own style.
cargo fmt -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
if command -v cargo-deny >/dev/null; then
	cargo deny check licenses bans sources
else
	echo "SKIP cargo deny: not installed (cargo install --locked cargo-deny)"
fi

echo "== T2.13 QEMU differential against OEM AArch64 serializers"
cargo build -p rbctl_voip_proto --example oracle_vectors --locked
rc=0
python3 -B tests/qemu/diff.py target/debug/examples/oracle_vectors || rc=$?
[ "$rc" = 0 ] || [ "$rc" = 77 ] || exit "$rc"

echo "== T2.18 rustdoc (missing docs are errors)"
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked

if [ -n "${BARESIP_PREFIX:-}" ]; then
  prefix=$(realpath "$BARESIP_PREFIX")
  for mod in account menu g711 g722 auresamp srtp mwi; do
    test -f "$prefix/lib/baresip/modules/$mod.so" || { echo "Missing host module: $mod" >&2; exit 1; }
  done
  cmake -S modules/rbctl_voip -B target/host-4.9 -DRBCTL_HARNESS=ON -DBUILD_TESTING=ON \
    -DBARESIP_INCLUDE_DIR="$prefix/include" -DRE_INCLUDE_DIR="$prefix/include/re" \
    -DBARESIP_EXE="$prefix/bin/baresip" -DBARESIP_MODULE_DIR="$prefix/lib/baresip/modules" \
    -DVOICE_ARCHIVE="$PWD/target/debug/librbctl_voip_ffi.a"
  cmake --build target/host-4.9 --parallel
  LD_LIBRARY_PATH="$prefix/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ctest --test-dir target/host-4.9 --output-on-failure
else
  echo 'SKIP module tests: set BARESIP_PREFIX to a full baresip/libre 4.9.0 host install'
fi
echo "All host checks passed."
