#!/usr/bin/env bash
# Offline Alpine image builder, called by build.sh (which is only a dispatcher).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
source "$ROOT/scripts/package-metadata.sh"
LOCAL="$ROOT/.alcard-local"
WORK="$LOCAL/image-stage"
DEST="$LOCAL/images"
REPO="$LOCAL/packages/alcard"
ABI="$(alcard_package kernel-abi)"
KERNEL_VERSION="$(alcard_package kernel-version)"
die() { echo "ERROR: $*" >&2; exit 1; }
case "$2" in
  v5) board=alcard-board-zero-v5 ;;
  v5-lite) board=alcard-board-zero-v5-lite ;;
  *) die 'Usage: build-image-zero.sh check|build v5|v5-lite' ;;
esac
case "$1" in check|build) ;; *) die 'Unknown operation' ;; esac

# Root is confined to the existing rootless Podman container. No host sudo.
if [[ "$(printenv ALCARD_IMAGE_CONTAINER_ROOT || :)" != 1 ]]; then
    command -v podman >/dev/null || die 'Podman required on host'
    box="$(printenv ALCARD_BUILD_BOX || echo alcard-arm64)"
    [[ -n "$box" ]] || die "Empty ALCARD_BUILD_BOX"
    exec podman exec --user 0 -e ALCARD_IMAGE_CONTAINER_ROOT=1 \
        -e "ALCARD_DEV_ROOT=${ALCARD_DEV_ROOT:-0}" \
        -e "ALCARD_ROOT_MIB=${ALCARD_ROOT_MIB:-2048}" \
        -e "ALCARD_SELECT_MIB=${ALCARD_SELECT_MIB:-64}" \
        -e "ALCARD_BOOT_MIB=${ALCARD_BOOT_MIB:-128}" \
        "$box" /bin/bash "$ROOT/scripts/build-image-zero.sh" "$1" "$2"
fi
[[ "$(id -u)" == 0 && "$(apk --print-arch)" == aarch64 ]] ||
    die 'Requires root within Alpine aarch64 rootless Podman container'
for cmd in apk mkfs.vfat mkfs.ext4 mcopy mtype python3 sha256sum depmod mkinitfs du awk; do
    command -v "$cmd" >/dev/null || die "Missing build dependency: $cmd"
done
alcard_package_array USER_PACKAGES group user
alcard_package_array ALL_APKS filename alcard-kmods-rpi alcard-firmware-zero \
    alcard-boot-zero alcard-bsp-zero "$board" "${USER_PACKAGES[@]}"
alcard_package_array SESSION_PINS pin --group session-roots
BOARD_PIN="$(alcard_package pin "$board")"
IMAGE_VERSION="$(alcard_package pkgver "$board")"
for filename in "${ALL_APKS[@]}"; do
    apk verify "$REPO/aarch64/$filename" || die "Missing/invalid signed APK: $filename"
done
[[ -s "$REPO/aarch64/APKINDEX.tar.gz" || -s "$REPO/aarch64/Packages.adb" ]] ||
    die 'Missing signed local APK index'
echo "==> Image prerequisites verified: $2"
[[ "$1" == build ]] || exit 0

[[ ! -L "$WORK" && ! -L "$DEST" ]] || die 'Refusing symlinked work/output directories'
mkdir -p "$DEST"
rm -rf -- "$WORK"
mkdir -p "$WORK"/{rootfs,select,boot-a,boot-b}
SYSROOT="$WORK/rootfs"
mkdir -p "$SYSROOT/etc/apk/keys"
cp -L /etc/apk/keys/*.pub "$SYSROOT/etc/apk/keys/" || die 'No trusted signing keys'
cat > "$WORK/repositories" <<EOF
https://dl-cdn.alpinelinux.org/alpine/v3.24/main
https://dl-cdn.alpinelinux.org/alpine/v3.24/community
$REPO
EOF

# Full installation including scripts, unlike dependency-only install-verify.
# Complete baselayout and BusyBox first: desktop pre-install scripts need the
# installed account databases and applet links to create their service users.
apk --root "$SYSROOT" --arch aarch64 \
  --keys-dir /etc/apk/keys --repositories-file "$WORK/repositories" \
  add --initdb alpine-base || die 'Base system APK install failed'
apk --root "$SYSROOT" --arch aarch64 \
  --keys-dir /etc/apk/keys --repositories-file "$WORK/repositories" \
  add "linux-rpi=$KERNEL_VERSION" raspberrypi-bootloader \
  "$BOARD_PIN" "${SESSION_PINS[@]}" \
  e2fsprogs kmod util-linux eudev eudev-openrc udev-init-scripts-openrc \
  greetd greetd-agreety ||
  die 'Real sys-mode apk installation failed'
python3 "$ROOT/tools/ensure-greetd-user.py" "$SYSROOT" ||
  die 'Could not provision isolated greetd greeter account'
[[ -s "$SYSROOT/etc/passwd" ]] || die 'Target passwd missing'
[[ -d "$SYSROOT/usr/lib/modules/$ABI" ]] || die 'Kernel modules missing'
# Alpine 3.24 is /usr-merged: the APK files live under /usr/lib/modules.
# Explicit -m avoids dependence on the container kmod default (/lib/modules).
# A depmod exit status alone does NOT prove it found any modules.
depmod -b "$SYSROOT" -m /usr/lib/modules -a "$ABI" ||
  die 'depmod failed for target /usr/lib/modules'
MODDIR="$SYSROOT/usr/lib/modules/$ABI"
for index in modules.dep modules.dep.bin modules.alias.bin; do
  [[ -s "$MODDIR/$index" ]] ||
    die "Missing $MODDIR/$index after depmod; check Alpine /usr merge and kmod -m"
done
for module in m5ioe1 gpio-forwarder tca8418_keypad_m5stack \
              pwm_bl_m5stack es8389_m5stack cardputerzero-audio \
              simple-amplifier bq27xxx_battery bq27xxx_battery_i2c \
              bmi270_core bmi270_i2c bmc150_magn_bmi270; do
  [[ -s "$MODDIR/extra/alcard/$module.ko" ]] ||
    die "Target vendor kernel module absent: $module"
  grep -Eq "(^|/)extra/alcard/$module[.]ko:" "$MODDIR/modules.dep" ||
    die "modules.dep lacks entry for $module"
done
echo "==> Verified depmod: 12 vendor modules and dependency indexes at /usr/lib/modules/$ABI"
# The full image now ships labwc, Foot, greetd, fonts and runtime services.
# Fail before image packing if the root staging tree exceeds available ext4
# capacity. The estimate conservatively leaves 25% free for filesystem
# metadata, logs, firstboot and apk operations.
ROOT_MIB="$(printenv ALCARD_ROOT_MIB || echo 2048)"
[[ "$ROOT_MIB" =~ ^[0-9]+$ && "$ROOT_MIB" -ge 1024 ]] ||
    die "ALCARD_ROOT_MIB must be an integer >=1024 for full images"
USED_KIB="$(du -sk "$SYSROOT" | awk '{print $1}')"
CAPACITY_KIB=$(( ROOT_MIB * 1024 * 75 / 100 ))
[[ "$USED_KIB" -le "$CAPACITY_KIB" ]] ||
    die "Rootfs too large: $USED_KIB KiB, 75% of root partition is $CAPACITY_KIB KiB; increase ALCARD_ROOT_MIB"
echo "==> Rootfs staging size $USED_KIB KiB, ext4 target $ROOT_MIB MiB"

cat > "$SYSROOT/etc/apk/repositories" <<'EOF'
https://dl-cdn.alpinelinux.org/alpine/v3.24/main
https://dl-cdn.alpinelinux.org/alpine/v3.24/community
# Alcard public signed repo will be enabled after publication.
EOF
cat > "$SYSROOT/etc/apk/world" <<EOF
alpine-base
linux-rpi=$KERNEL_VERSION
raspberrypi-bootloader
$board
greetd
greetd-agreety
e2fsprogs
kmod
util-linux
eudev
eudev-openrc
udev-init-scripts-openrc
EOF
printf '%s\n' "${SESSION_PINS[@]}" >> "$SYSROOT/etc/apk/world"
printf 'alcard\n' > "$SYSROOT/etc/hostname"
cat > "$SYSROOT/etc/fstab" <<'EOF'
LABEL=ALCARDROOT / ext4 rw,noatime 0 1
EOF
cat > "$SYSROOT/etc/modules" <<'EOF'
spi_bcm2835
m5ioe1
pwm_bl_m5stack
panel_mipi_dbi
tca8418_keypad_m5stack
EOF
# Exactly one owner of tty1: firstboot dispatcher, then greetd/agreety.
# Do not enable the greetd OpenRC service on the same VT.
python3 - "$SYSROOT/etc/inittab" <<'PY'
from pathlib import Path
import sys
path=Path(sys.argv[1])
rows=path.read_text().splitlines()
rows=[row for row in rows if not row.startswith("tty1::")]
rows.append("tty1::respawn:/usr/libexec/alcard/tty1-dispatch")
path.write_text("\n".join(rows)+"\n")
PY
[[ -x "$SYSROOT/usr/libexec/alcard/tty1-dispatch" ]] ||
  die 'Missing Hello tty1 dispatcher'
[[ -s "$SYSROOT/etc/greetd/alcard.toml" &&
   -x "$SYSROOT/usr/libexec/alcard/start-session" ]] ||
  die 'Missing Alcard greetd configuration/session launcher'
[[ -x "$SYSROOT/usr/sbin/greetd" || -x "$SYSROOT/usr/bin/greetd" ]] ||
  die 'Missing greetd executable'
[[ -x "$SYSROOT/usr/bin/agreety" || -x "$SYSROOT/usr/sbin/agreety" ]] ||
  die 'Missing agreety executable'
[[ -s "$SYSROOT/usr/share/kbd/consolefonts/AlcardPixel-Regular-core-8x8.psf" ]] ||
  die 'Missing PSF2 Alcard console font'



# A fresh apk --root install provides init scripts but does not populate
# runlevels as Alpine setup scripts normally would. Configure the TARGET
# OpenRC runlevels, without starting any service or touching host OpenRC.
python3 - "$SYSROOT" <<'PY'
from pathlib import Path
import posixpath
import sys

root = Path(sys.argv[1])
# Only the board's essential boot services are mandatory; Alpine releases
# can relocate or omit other init scripts. Preserve whichever optional
# services are actually installed.
required = {
    "sysinit": ("devfs",),
    "boot": ("modules",),
}
optional = {
    "sysinit": ("dmesg",),
    "boot": ("sysctl", "hostname", "bootmisc", "fsck"),
    "shutdown": ("mount-ro", "killprocs"),
}
services = {level: [(name, True) for name in names]
            for level, names in required.items()}
for level, names in optional.items():
    services.setdefault(level, []).extend((name, False) for name in names)
for level, names in services.items():
    level_dir = root / "etc/runlevels" / level
    level_dir.mkdir(parents=True, exist_ok=True)
    for name, mandatory in names:
        service = root / "etc/init.d" / name
        if not service.is_file():
            if mandatory:
                raise SystemExit(f"Required OpenRC init script missing: {service}")
            continue
        link = level_dir / name
        target = "../../init.d/" + name
        if link.is_symlink():
            actual = posixpath.normpath(
                posixpath.join("/etc/runlevels/" + level, link.readlink())
            )
            if actual != "/etc/init.d/" + name:
                raise SystemExit(f"Unexpected OpenRC service link: {link} -> {link.readlink()}")
        elif link.exists():
            raise SystemExit(f"Refusing to overwrite non-symlink OpenRC service: {link}")
        else:
            link.symlink_to(target)
        print(f"==> Enable OpenRC: {name} in {level}")
PY
# Device management belongs to the common image base and survives Console.
# Graphical logins use this same eudev/cgroups/elogind setup as the VM gate.
python3 "$ROOT/tools/configure-session-services.py" "$SYSROOT"
[[ -x "$SYSROOT/usr/libexec/alcard/wayland-session" &&
   -s "$SYSROOT/etc/alcard/foot.ini" ]] ||
    die "Missing Wayland session or Foot configuration"
python3 "$ROOT/tools/verify-session-install.py" --root "$SYSROOT" --mode graphics
# Ignore leftover local credentials unless this build explicitly enables them.
case ${ALCARD_DEV_ROOT:-0} in 0|1) ;; *) die 'ALCARD_DEV_ROOT must be 0 or 1' ;; esac
if [[ ${ALCARD_DEV_ROOT:-0} == 1 ]]; then
    [[ -f "$LOCAL/dev-root-password.hash" ]] || die 'Missing developer root password hash'
    echo 'WARNING: local dev-root-password.hash unlocks root and prevents automatic Hello on first boot' >&2
    [[ ! -L "$LOCAL/dev-root-password.hash" ]] || die 'Unsafe password hash symlink'
    [[ "$(stat -c %a "$LOCAL/dev-root-password.hash")" == 600 ]] ||
        die 'dev-root-password.hash must have permission mode 0600'
    python3 - "$SYSROOT/etc/shadow" "$LOCAL/dev-root-password.hash" <<'PY'
from pathlib import Path
import sys
shadow, source = map(Path, sys.argv[1:])
value = source.read_text().strip()
if not value.startswith("$6$") or ":" in value or "\n" in value:
    raise SystemExit("Expected a single SHA-512 crypt hash beginning with $6$")
lines = shadow.read_text().splitlines()
found = False
for i, line in enumerate(lines):
    fields = line.split(":")
    if fields[0] == "root":
        fields[1] = value
        lines[i] = ":".join(fields)
        found = True
if not found:
    raise SystemExit("Missing root shadow entry")
shadow.write_text("\n".join(lines) + "\n")
PY
    echo '==> Installed developer root password hash (local console use only)'
else
    echo '==> Root remains locked until initial setup'
fi


mkinitfs -b "$SYSROOT" -o "$SYSROOT/boot/initramfs-rpi" "$ABI" || die 'Initramfs build failed'
BOOT="$SYSROOT/boot"
for f in vmlinuz-rpi initramfs-rpi bcm2710-rpi-cm0.dtb \
         bootcode.bin start.elf fixup.dat alcard.cfg overlays/alcard-zero-v5.dtbo; do
    [[ -s "$BOOT/$f" ]] || die "Boot file missing: $f; inspect Alpine package contents"
done

DISK_ID="$(python3 -c 'import secrets; print("%08x" % (secrets.randbelow(0xfffffffe)+1))')"
ROOT_UUID="$(python3 -c 'import uuid; print(uuid.uuid4())')"
for slot in select boot-a boot-b; do
    python3 "$ROOT/tools/image-zero.py" stage-boot --source "$BOOT" --destination "$WORK/$slot"
    cat > "$WORK/$slot/config.txt" <<'EOF'
[all]
arm_64bit=1
kernel=vmlinuz-rpi
initramfs initramfs-rpi followkernel
include alcard.cfg
EOF
    printf 'console=tty1 root=PARTUUID=%s-04 rootfstype=ext4 rootwait rw\n' \
        "$DISK_ID" > "$WORK/$slot/cmdline.txt"
done
OUTPUT="$DEST/alcard-zero-$2-$IMAGE_VERSION.img"
[[ ! -L "$OUTPUT" ]] || die 'Refusing symlinked output'
python3 "$ROOT/tools/image-zero.py" pack --work "$WORK" \
  --output "$OUTPUT" --profile "$2" --disk-id "$DISK_ID" \
  --root-uuid "$ROOT_UUID" \
  --select-mib "$(printenv ALCARD_SELECT_MIB || echo 64)" \
  --boot-mib "$(printenv ALCARD_BOOT_MIB || echo 128)" \
  --root-mib "$(printenv ALCARD_ROOT_MIB || echo 2048)"
(cd "$DEST"; sha256sum "alcard-zero-$2-$IMAGE_VERSION.img" > "alcard-zero-$2-$IMAGE_VERSION.img.sha256")
echo "==> Image built: $OUTPUT"
echo '==> CM0 boot and tryboot remain unverified on hardware'
