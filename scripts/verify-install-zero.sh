#!/usr/bin/env bash
# Install prebuilt Alcard APK into a disposable rootless Alpine aarch64 sys-mode
# filesystem for package integration checks. Never touch host /boot or apk DB.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
source "$ROOT/scripts/package-metadata.sh"
LOCAL="$ROOT/.alcard-local"
# apk repositories are architecture roots; apk appends /aarch64 itself.
REPO_ROOT="$LOCAL/packages/alcard"
REPO="$REPO_ROOT/aarch64"
CHECK="$LOCAL/verify"
TESTROOT="$CHECK/install-rootfs"
REPOLIST="$CHECK/install-repositories.list"
ABI="$(alcard_package kernel-abi)"
PROFILE="${2:-v5}"
case "$PROFILE" in
    v5) BOARD=alcard-board-zero-v5; FORBIDDEN=alcard-board-zero-v5-lite ;;
    v5-lite) BOARD=alcard-board-zero-v5-lite; FORBIDDEN=alcard-board-zero-v5 ;;
    *) echo "ERROR: unknown profile $PROFILE" >&2; exit 2 ;;
esac

log() { printf '==> %s\n' "$*"; }
fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

check() {
    [[ "$(apk --print-arch)" == "aarch64" ]] || fail 'Use the Alpine aarch64 Distrobox'
    [[ "$(id -u)" != 0 ]] || fail 'Do not execute as root'
    for cmd in apk depmod modinfo find wc grep sha256sum; do
        command -v "$cmd" >/dev/null 2>&1 || fail "Missing tool: $cmd"
    done
    [[ ! -L "$TESTROOT" ]] || fail 'Refusing symlink as disposable rootfs'
    [[ -d "$REPO_ROOT" ]] || fail "Missing local APK repo root $REPO_ROOT"
    [[ -d "$REPO" ]] || fail "Missing local APK repository $REPO"
    [[ -s "$REPO/APKINDEX.tar.gz" || -s "$REPO/Packages.adb" ]] ||
        fail "Missing abuild-signed index in $REPO"
    local entry
    for entry in \
        "$(alcard_package filename alcard-kmods-rpi)" \
        "$(alcard_package filename alcard-firmware-zero)" \
        "$(alcard_package filename alcard-boot-zero)" \
        "$(alcard_package filename alcard-bsp-zero)" \
        "$(alcard_package filename "$BOARD")" \
        "$(alcard_package filename "$FORBIDDEN")"; do
        [[ -s "$REPO/$entry" ]] || fail "Missing local APK: $entry"
    done
    apk verify \
        "$REPO/$(alcard_package filename alcard-kmods-rpi)" \
        "$REPO/$(alcard_package filename alcard-firmware-zero)" \
        "$REPO/$(alcard_package filename alcard-boot-zero)" \
        "$REPO/$(alcard_package filename alcard-bsp-zero)" \
        "$REPO/$(alcard_package filename "$BOARD")" \
        "$REPO/$(alcard_package filename "$FORBIDDEN")" || fail 'One or more APK signatures invalid'
    [[ -d /etc/apk/keys ]] || fail 'Missing Alpine trusted APK keys'
    log 'Ready for rootless isolated Alpine installation'
}

install_test() {
    check
    log "Creating disposable rootfs under ${TESTROOT#"$ROOT"/}"
    mkdir -p "$CHECK"
    rm -rf -- "$TESTROOT"
    mkdir -p "$TESTROOT/etc/apk/keys" "$TESTROOT/boot" "$TESTROOT/var/cache/apk"

    # The isolated apk database trusts exactly the public keys that were used
    # successfully by apk verify on the build container.
    cp -L /etc/apk/keys/* "$TESTROOT/etc/apk/keys/" ||
        fail 'Cannot stage public Alpine/Alcard signing keys'

    # Use only the pinned Alpine branch plus the signed local APK index.
    # No untrusted package override or --force-overwrite is allowed.
    cat >"$REPOLIST" <<EOF
https://dl-cdn.alpinelinux.org/alpine/v3.24/main
https://dl-cdn.alpinelinux.org/alpine/v3.24/community
$REPO_ROOT
EOF
    log "Local repository root: $REPO_ROOT (apk resolves aarch64/APKINDEX.tar.gz)"
    log "Installing alpine-base + $BOARD via apk dependency solver"
    log 'The first run downloads the referenced official Alpine packages.'
    apk --root "$TESTROOT" \
        --arch aarch64 \
        --keys-dir /etc/apk/keys \
        --repositories-file "$REPOLIST" \
        --no-scripts \
        add --initdb --usermode \
        alpine-base "$(alcard_package pin "$BOARD")" ||
        fail 'Rootless apk add failed (no host or build-container packages were modified)'

    local k base count mod
    for k in linux-rpi alcard-kmods-rpi alcard-firmware-zero alcard-boot-zero alcard-bsp-zero "$BOARD"; do
        apk --root "$TESTROOT" list --installed --manifest "$k" |
            grep -q "^$k " || fail "Package $k absent from test rootfs"
    done
    if apk --root "$TESTROOT" list --installed --manifest "$FORBIDDEN" |
        grep -q "^$FORBIDDEN "; then
        fail "Mutually exclusive package $FORBIDDEN is installed"
    fi
    local cfg="$TESTROOT/boot/alcard.cfg"
    [[ -s "$cfg" ]] || fail "Missing selected $BOARD firmware config"
    grep -qx 'dtoverlay=alcard-zero-v5' "$cfg" || fail 'Main board overlay missing'
    grep -qx 'dtoverlay=alcard-bq27220-v5' "$cfg" || fail 'Battery overlay missing'
    case "$PROFILE" in
        v5)
            grep -qx 'dtoverlay=alcard-bmi270-bmm150' "$cfg" ||
                fail 'Standard v5 missing IMU/BMM150'
            ;;
        v5-lite)
            if grep -Eq '^dtoverlay=(alcard-bmi270|imx219)' "$cfg"; then
                fail 'Lite activates absent IMU/camera'
            fi
            ;;
    esac
    log "Checking that the other board profile cannot coexist with $BOARD"
    if apk --root "$TESTROOT" --arch aarch64 \
        --keys-dir /etc/apk/keys \
        --repositories-file "$REPOLIST" \
        --simulate add "$(alcard_package pin "$FORBIDDEN")" >/dev/null 2>&1; then
        fail "apk incorrectly accepted both $BOARD and $FORBIDDEN"
    fi
    log "PASS: only $BOARD may be installed"
    base="$TESTROOT/usr/lib/modules/$ABI/extra/alcard"
    [[ -d "$base" ]] || fail 'Wrong module install directory'
    count="$(find "$base" -type f -name '*.ko' | wc -l | tr -d ' ')"
    [[ "$count" -eq 12 ]] || fail "Expected 12 external modules, found $count"
    for mod in "$base"/*.ko; do
        [[ "$(modinfo -F vermagic "$mod" | cut -d ' ' -f1)" == "$ABI" ]] ||
            fail "Kernel ABI mismatch: $mod"
    done

    for k in \
        "$TESTROOT/usr/lib/firmware/cardputerzero,st7789v.bin" \
        "$TESTROOT/usr/lib/firmware/bmi270-init-data.fw" \
        "$TESTROOT/usr/share/licenses/alcard-firmware-zero/Bosch-BMI270-BSD-3-Clause.txt" \
        "$TESTROOT/boot/alcard.cfg" \
        "$TESTROOT/boot/overlays/alcard-zero-v5.dtbo" \
        "$TESTROOT/boot/overlays/alcard-bq27220-v5.dtbo" \
        "$TESTROOT/boot/overlays/alcard-bmi270-bmm150.dtbo"; do
        [[ -s "$k" ]] || fail "Expected installed file missing: $k"
    done

    # --no-scripts prevents package hooks and trigger execution in the
    # non-bootable rootfs; generate and inspect module dependency metadata here.
    log "Running depmod against isolated rootfs, ABI $ABI"
    depmod -b "$TESTROOT" -a "$ABI" ||
        fail 'depmod failed for the isolated rootfs'
    [[ -s "$TESTROOT/usr/lib/modules/$ABI/modules.dep" ||
       -s "$TESTROOT/lib/modules/$ABI/modules.dep" ]] ||
        fail 'modules.dep not generated'
    log "PASS: $BOARD pulled common BSP, all 12 modules and depmod"
    log "Test rootfs remains at: ${TESTROOT#"$ROOT"/}"
    log 'This rootfs is not a bootable installed image; boot FAT, initramfs and hardware remain unverified.'
}

case "${1:-check}" in
    check) check ;;
    install) install_test ;;
    *) fail 'Usage: scripts/verify-install-zero.sh [check|install]' ;;
esac
