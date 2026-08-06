#!/bin/bash
# Boot the armsr/armv8 OpenWrt image in QEMU and run the rbctl-dsl Phase 0 gate test.
# Usage: ./boot-qemu-test.sh
set -euo pipefail

SDK="${OPENWRT_SDK:-/path/to/openwrt}"
IMG_DIR="$SDK/bin/targets/armsr/armv8"
AAVMF_CODE="/usr/share/AAVMF/AAVMF_CODE.fd"
AAVMF_VARS="/usr/share/AAVMF/AAVMF_VARS.snakeoil.fd"

# Find the combined EFI image (full disk: ESP + rootfs)
EFI_IMG=$(ls "$IMG_DIR"/openwrt-*armsr*armv8*combined-efi*.img 2>/dev/null | head -1)
if [ -z "$EFI_IMG" ]; then
    echo "ERROR: no combined-efi image found in $IMG_DIR"
    echo "Has the build finished? Looking for: openwrt-*armsr*armv8*combined-efi*.img"
    ls "$IMG_DIR"/*.img 2>/dev/null || echo "(no .img files at all)"
    exit 1
fi

echo "=== Booting OpenWrt armsr/armv8 in QEMU ==="
echo "Image: $EFI_IMG"
echo "Serial console on stdin/stdout. Login as root (no password)."
echo "To test rbctl-dsl: /usr/bin/rbctl-dsl"
echo "To exit QEMU: Ctrl-A then X"
echo ""

# Make a writable copy of VARS (AAVMF requires a writable vars store)
VARS_COPY=$(mktemp --suffix=.fd)
cp "$AAVMF_VARS" "$VARS_COPY"

exec qemu-system-aarch64 \
    -M virt -cpu cortex-a53 -m 512 \
    -drive if=pflash,format=raw,readonly=on,file="$AAVMF_CODE" \
    -drive if=pflash,format=raw,file="$VARS_COPY" \
    -drive file="$EFI_IMG",format=raw,if=virtio \
    -nographic \
    -netdev user,id=net0,hostfwd=tcp::2222-:22 \
    -device virtio-net-pci,netdev=net0
