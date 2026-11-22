#!/usr/bin/env bash
# Boot finished Alpine aarch64 Hello rootfs as QEMU virt, no developer socket.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
OUT="$ROOT/.alcard-local/qemu-aarch64"
for item in vmlinuz-virt initramfs-virt alcard-hello-aarch64.ext4; do
  [[ -s "$OUT/$item" ]] || { echo "Missing QEMU image: $item; build first" >&2; exit 1; }
done
command -v qemu-system-aarch64 >/dev/null || { echo "Install qemu-system-aarch64" >&2; exit 1; }
echo "Booting installed Alcard Hello aarch64 (virt machine, serial console)"
echo "Use Ctrl+A then X to quit QEMU."
exec qemu-system-aarch64 -machine virt -cpu cortex-a72 \
  -m 1024 -smp 2 -nographic \
  -kernel "$OUT/vmlinuz-virt" -initrd "$OUT/initramfs-virt" \
  -append "console=ttyAMA0 root=/dev/vda rw rootfstype=ext4 modules=virtio_pci,virtio_blk,ext4" \
  -drive "file=$OUT/alcard-hello-aarch64.ext4,format=raw,if=none,id=rootdisk" \
  -device virtio-blk-device,drive=rootdisk \
  -netdev user,id=network0 -device virtio-net-device,netdev=network0
