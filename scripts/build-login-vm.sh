#!/bin/sh
# Build a private real-VT CI guest from already verified signed APKs.
# This script intentionally cannot build on the host or export a credential disk.
set -eu
[ "${ALCARD_LOGIN_VM_BUILD:-}" = 1 ] && [ -f /.dockerenv ] && \
    [ "$(id -u)" = 0 ] && [ "$(apk --print-arch)" = aarch64 ] || {
    echo 'Refusing login VM construction outside disposable Alpine aarch64 CI' >&2
    exit 1
}
case "$(cat /etc/alpine-release)" in 3.24.*) ;; *) exit 1 ;; esac
ROOT=/work
INPUT=$ROOT/.alcard-local/login-input
OUT=$ROOT/.alcard-local/ci-login
SOURCE=/tmp/alcard-login-source
[ -d "$INPUT/packages/aarch64" ] && [ ! -L "$OUT" ] && \
    [ ! -L "$ROOT/.alcard-local" ] || exit 1
mkdir -p "$OUT/private" "$OUT/evidence"
apk add --no-cache bash python3 e2fsprogs kmod mkinitfs
(
    cd "$INPUT"
    sha256sum -c SHA256SUMS
) > "$OUT/evidence/artifact-hashes.log"
test "$(find "$INPUT/packages/aarch64" -name '*.apk' | wc -l)" -eq 9
test "$(cat "$INPUT/status.txt")" = exit_status=0
for key in "$INPUT"/*.rsa.pub; do
    [ -f "$key" ] || { echo 'Missing CI public signing key' >&2; exit 1; }
    install -m 644 "$key" /etc/apk/keys/
done
mkdir -p "$SOURCE/.alcard-local/packages/alcard"
tar -C "$ROOT" --exclude=.git --exclude=.alcard-local --exclude=target \
    --exclude=__pycache__ -cf - . | tar -C "$SOURCE" -xf -
cp -R "$INPUT/packages/aarch64" "$SOURCE/.alcard-local/packages/alcard/"
ALCARD_SESSION_CONTAINER_ROOT=1 bash "$SOURCE/scripts/verify-session-install.sh" install \
    > "$OUT/evidence/install.log" 2>&1 || {
    cat "$OUT/evidence/install.log"
    exit 1
}
SYSROOT=$SOURCE/.alcard-local/session-install/rootfs
REPOS=$SOURCE/.alcard-local/session-install/repositories
# These are test platform/observation tools, not replacements for the production
# login, launcher, compositor, polkit agent, PAM policy, or locker executables.
apk --root "$SYSROOT" --arch aarch64 --keys-dir /etc/apk/keys \
    --repositories-file "$REPOS" add linux-virt mkinitfs eudev eudev-openrc \
    udev-init-scripts-openrc python3 busctl util-linux kmod \
    >> "$OUT/evidence/install.log" 2>&1 || {
    tail -100 "$OUT/evidence/install.log"
    exit 1
}
python3 "$SOURCE/tools/verify-session-install.py" --root "$SYSROOT" --mode graphics \
    >> "$OUT/evidence/install.log"
# The ordinary polkit probe targets Alpine's installed BusyBox true applet.
# Resolve its links inside the guest; the build host may have a different layout.
chroot "$SYSROOT" /bin/sh -eu -c '[ -x /bin/true ] && /bin/true'
install -d -m 755 "$SYSROOT/usr/libexec/alcard-ci"
install -m 755 "$SOURCE/scripts/fixtures/login-vm/boot" \
    "$SYSROOT/usr/libexec/alcard-ci/boot"
install -m 755 "$SOURCE/scripts/fixtures/login-vm/observer.py" \
    "$SYSROOT/usr/libexec/alcard-ci/observer.py"
install -m 755 "$SOURCE/scripts/fixtures/login-vm/policy-probe.py" \
    "$SYSROOT/usr/libexec/alcard-ci/policy-probe"
install -m 755 "$SOURCE/scripts/ci/hello/measure-idle-resources.py" \
    "$SYSROOT/usr/libexec/alcard-ci/measure-idle-resources.py"
printf '%s\n' 'Alcard isolated CI guest; never a distributable system image.' \
    > "$SYSROOT/etc/alcard-ci-vm"
python3 "$SOURCE/tools/configure-session-services.py" "$SYSROOT" \
    >> "$OUT/evidence/install.log"
mkdir -p "$SYSROOT/etc/runlevels/shutdown"
rm -f "$SYSROOT/etc/runlevels/default/greetd"
for pair in boot/sysctl boot/hostname boot/fsck boot/bootmisc \
    shutdown/mount-ro shutdown/killprocs; do
    level=${pair%/*}
    service=${pair#*/}
    [ -x "$SYSROOT/etc/init.d/$service" ] || {
        echo "Required real OpenRC service missing: $service" >&2; exit 1;
    }
    ln -sfn "../../init.d/$service" "$SYSROOT/etc/runlevels/$level/$service"
done
cat > "$SYSROOT/etc/modules" <<'MODULES'
virtio_gpu
evdev
xhci_pci
usbhid
hid_generic
MODULES
printf 'alcard-login-ci\n' > "$SYSROOT/etc/hostname"
printf '/dev/vda / ext4 rw,noatime 0 1\n' > "$SYSROOT/etc/fstab"
cat > "$SYSROOT/etc/apk/repositories" <<'REPOSITORIES'
https://dl-cdn.alpinelinux.org/alpine/v3.24/main
https://dl-cdn.alpinelinux.org/alpine/v3.24/community
REPOSITORIES
python3 - "$SYSROOT/etc/inittab" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
lines = [line for line in path.read_text().splitlines()
         if not line.startswith(('tty', 'hvc'))]
assert lines.count('::wait:/sbin/openrc default') == 1, 'Expected Alpine default runlevel action'
# BusyBox init finishes wait actions before starting respawn actions. The
# private observer applies real Hello once, then releases the normal dispatcher.
lines += ['::wait:/usr/libexec/alcard-ci/boot',
          'tty1::respawn:/usr/libexec/alcard/tty1-dispatch tty1']
path.write_text('\n'.join(lines) + '\n')
PY
chroot "$SYSROOT" /bin/sh -eu -c '
    cat /etc/inittab
    ls -l /etc/runlevels/sysinit /etc/runlevels/boot /etc/runlevels/default
    [ ! -e /etc/runlevels/default/polkit ]
' > "$OUT/evidence/boot-config.log"
# The Alpine linux-virt APK installs /lib/modules. Resolve inside the guest,
# including any guest links, without following absolute links on the build host.
chroot "$SYSROOT" /bin/sh -c \
    'ls -ld /lib /usr/lib /lib/modules /usr/lib/modules 2>&1 || :' \
    > "$OUT/evidence/kernel-layout.log"
ABI=$(chroot "$SYSROOT" /bin/sh -eu -c '
    set -- /lib/modules/*
    [ "$#" -eq 1 ] && [ -d "$1" ]
    printf "%s\n" "${1##*/}"
')
chroot "$SYSROOT" /bin/sh -eu -c '
    PATH=/usr/sbin:/usr/bin:/sbin:/bin
    export PATH
    exec depmod -m /lib/modules -a "$1"
' sh "$ABI"
for index in modules.dep modules.dep.bin modules.alias.bin; do
    chroot "$SYSROOT" /bin/sh -eu -c '[ -s "$1" ]' sh "/lib/modules/$ABI/$index"
done
mkinitfs -b "$SYSROOT" -o "$OUT/private/initramfs-virt" "$ABI"
cp "$SYSROOT/boot/vmlinuz-virt" "$OUT/private/vmlinuz-virt"
truncate -s 2G "$OUT/private/rootfs.ext4"
mkfs.ext4 -F -L ALCARDLOGIN -d "$SYSROOT" "$OUT/private/rootfs.ext4" \
    > "$OUT/evidence/ext4.log" 2>&1
{
    printf '%s\n' 'Fresh Alpine 3.24/aarch64 real-VT QEMU guest.'
    printf '%s\n' 'Production packages are the verified signed installed-session input artifact; see artifact-hashes.log.'
    printf '%s\n' 'Guest-only observer and ordinary policy probe are CI fixtures; no PAM/auth overrides.'
    printf '%s\n' 'No VM disk, shadow file, fixture password, or private signing key is exported.'
    apk --root "$SYSROOT" info -v
    sha256sum "$OUT/private/vmlinuz-virt" "$OUT/private/initramfs-virt"
} > "$OUT/evidence/platform.txt"
chmod -R a+rX "$OUT/evidence"
# Host QEMU needs read-only access; -snapshot keeps secrets out of this base disk.
chmod 644 "$OUT/private/rootfs.ext4" "$OUT/private/vmlinuz-virt" \
    "$OUT/private/initramfs-virt"
chown -R "$(stat -c %u:%g "$ROOT")" "$OUT"
echo 'PASS: signed APK guest built; actual login acceptance runs after boot.'
