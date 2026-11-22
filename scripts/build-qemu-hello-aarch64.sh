#!/usr/bin/env bash
# Build a disposable Alpine aarch64 virt machine image from signed Hello+font APKs.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
source "$ROOT/scripts/package-metadata.sh"
LOCAL="$ROOT/.alcard-local"
WORK="$LOCAL/qemu-aarch64/work"
OUT="$LOCAL/qemu-aarch64"
REPO="$LOCAL/packages/alcard"
fail() { echo "ERROR: $*" >&2; exit 1; }

if [[ "${ALCARD_QEMU_CONTAINER_ROOT:-}" != 1 ]]; then
  command -v podman >/dev/null || fail "Podman required on host"
  box="${ALCARD_BUILD_BOX:-alcard-arm64}"
  exec podman exec --user 0 -e ALCARD_QEMU_CONTAINER_ROOT=1 \
    "$box" /bin/bash "$ROOT/scripts/build-qemu-hello-aarch64.sh" "${1:-check}"
fi

[[ "$(id -u)" == 0 && "$(apk --print-arch)" == aarch64 ]] ||
  fail "Expected root inside Alpine aarch64 rootless Podman"
for cmd in apk mkfs.ext4 mkinitfs depmod python3 chroot; do
  command -v "$cmd" >/dev/null || fail "Missing: $cmd (install in the build container)"
done
alcard_package_array USER_APKS filename --group user
alcard_package_array SESSION_PINS pin --group session-roots
for filename in "${USER_APKS[@]}"; do
    apk verify "$REPO/aarch64/$filename" || fail "Missing/invalid signed APK: $filename"
done
case "${1:-check}" in
 check) echo "==> QEMU aarch64 requirements OK"; exit 0 ;;
 build) ;;
 *) fail "Usage: $0 check|build" ;;
esac

[[ ! -L "$OUT" && ! -L "$WORK" ]] || fail "Refusing symlinked output directories"
rm -rf -- "$WORK"
mkdir -p "$WORK/rootfs/etc/apk/keys" "$OUT"
SYSROOT="$WORK/rootfs"
cp -L /etc/apk/keys/*.pub "$SYSROOT/etc/apk/keys/"
cat > "$WORK/repositories" <<EOF
https://dl-cdn.alpinelinux.org/alpine/v3.24/main
https://dl-cdn.alpinelinux.org/alpine/v3.24/community
$REPO
EOF
# Desktop pre-install account scripts need baselayout and BusyBox to have
# finished installing; a single transaction does not guarantee that order.
apk --root "$SYSROOT" --arch aarch64 \
  --keys-dir /etc/apk/keys --repositories-file "$WORK/repositories" \
  add --initdb alpine-base || fail "Base system APK install failed"
apk --root "$SYSROOT" --arch aarch64 \
  --keys-dir /etc/apk/keys --repositories-file "$WORK/repositories" \
  add linux-virt mkinitfs \
  "${SESSION_PINS[@]}" \
  e2fsprogs kmod util-linux greetd greetd-agreety eudev eudev-openrc udev-init-scripts-openrc || fail "Rootfs APK install failed"

python3 "$ROOT/tools/ensure-greetd-user.py" "$SYSROOT" ||
  fail "Could not provision isolated greetd greeter account"
[[ -s "$SYSROOT/etc/greetd/alcard.toml" ]] ||
  fail "Missing greetd session configuration"
mkdir -p "$SYSROOT/etc/runlevels"/{sysinit,boot,default,shutdown}
for pair in sysinit/devfs boot/modules boot/sysctl boot/hostname \
            boot/fsck boot/bootmisc shutdown/mount-ro shutdown/killprocs; do
  level="${pair%/*}"
  svc="${pair#*/}"
  [[ -f "$SYSROOT/etc/init.d/$svc" ]] &&
    ln -sfn "../../init.d/$svc" "$SYSROOT/etc/runlevels/$level/$svc"
done
# All three choices ship. Hello retains Shell, selects the Foot fallback, or
# removes the graphical stack only after the account setup is ready.
python3 "$ROOT/tools/configure-session-services.py" "$SYSROOT"
[[ -x "$SYSROOT/usr/libexec/alcard/wayland-session" &&
   -s "$SYSROOT/etc/alcard/foot.ini" ]] ||
  fail "Missing Wayland session or terminal config"
python3 "$ROOT/tools/verify-session-install.py" --root "$SYSROOT" --mode graphics
python3 - "$SYSROOT/etc/inittab" <<'PY'
from pathlib import Path
import sys
p=Path(sys.argv[1])
lines=[l for l in p.read_text().splitlines()
       if not l.startswith('ttyAMA0::') and not l.startswith('tty1::')]
lines.append('ttyAMA0::respawn:/usr/libexec/alcard/tty1-dispatch ttyAMA0')
p.write_text('\n'.join(lines)+'\n')
PY
printf 'alcard-qemu\n' > "$SYSROOT/etc/hostname"
cat > "$SYSROOT/etc/fstab" <<'EOF'
/dev/vda / ext4 rw,noatime 0 1
EOF

# Alpine linux-virt installs /lib/modules. Resolve its exact single ABI inside
# this native guest root, so guest absolute symlinks never reach the build host.
ABI="$(chroot "$SYSROOT" /bin/sh -eu -c '
    set -- /lib/modules/*
    [ "$#" -eq 1 ] && [ -d "$1" ]
    printf "%s\n" "${1##*/}"
')" || fail "Expected exactly one linux-virt module ABI"
chroot "$SYSROOT" /bin/sh -eu -c '
    PATH=/usr/sbin:/usr/bin:/sbin:/bin
    export PATH
    exec depmod -m /lib/modules -a "$1"
' sh "$ABI" || fail "depmod failed"
for index in modules.dep modules.dep.bin modules.alias.bin; do
    chroot "$SYSROOT" /bin/sh -eu -c '[ -s "$1" ]' sh "/lib/modules/$ABI/$index" ||
        fail "Missing linux-virt module index: $index"
done
mkinitfs -b "$SYSROOT" -o "$SYSROOT/boot/initramfs-virt" "$ABI" || fail "mkinitfs failed"
[[ -s "$SYSROOT/boot/vmlinuz-virt" && -s "$SYSROOT/boot/initramfs-virt" ]] ||
  fail "Missing kernel/initramfs"
cp "$SYSROOT/boot/vmlinuz-virt" "$OUT/vmlinuz-virt"
cp "$SYSROOT/boot/initramfs-virt" "$OUT/initramfs-virt"
IMG="$OUT/alcard-hello-aarch64.ext4"
[[ ! -L "$IMG" ]] || fail "Refusing symlinked disk output"
truncate -s 3G "$IMG"
mkfs.ext4 -F -L ALCARDQEMU -d "$SYSROOT" "$IMG" >/dev/null ||
  fail "Ext4 image creation failed"
(cd "$OUT" && sha256sum alcard-hello-aarch64.ext4 > alcard-hello-aarch64.ext4.sha256)
echo "==> QEMU image ready: $IMG"
echo "==> Boot using: scripts/qemu-hello-aarch64-run.sh"
