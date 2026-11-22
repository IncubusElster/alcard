#!/usr/bin/env bash
# Static verification of signed APK payload, merged CM0 DT and Alpine DRM config.
# No installs, root access, or changes outside .alcard-local/verify.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
source "$ROOT/scripts/package-metadata.sh"
LOCAL="$ROOT/.alcard-local"
PKGS="$LOCAL/packages/alcard/aarch64"
OUT="$LOCAL/verify"
PAYLOAD="$OUT/payload"
ABI="$(alcard_package kernel-abi)"
CFG="/usr/src/linux-headers-$ABI/.config"
BASE="${ALCARD_BASE_DTB:-/boot/bcm2710-rpi-cm0.dtb}"
PROFILE="${2:-v5}"
case "$PROFILE" in
    v5) BOARD=alcard-board-zero-v5 ;;
    v5-lite) BOARD=alcard-board-zero-v5-lite ;;
    *) echo "ERROR: unknown board profile $PROFILE" >&2; exit 2 ;;
esac
log() { printf '==> %s\n' "$*"; }
fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
require() { command -v "$1" >/dev/null 2>&1 || fail "Missing $1 inside ARM64 Distrobox"; }

check_setup() {
    [ "$(apk --print-arch)" = aarch64 ] || fail 'Expected Alpine ARM64 Distrobox'
    [ "$(id -u)" -ne 0 ] || fail 'Run as normal user'
    local c
    for c in apk fdtoverlay fdtget modinfo sha256sum find wc; do require "$c"; done
}

check_packages() {
    log "Checking signed common BSP APK plus $BOARD"
    local files=(
        "$PKGS/$(alcard_package filename alcard-kmods-rpi)"
        "$PKGS/$(alcard_package filename alcard-firmware-zero)"
        "$PKGS/$(alcard_package filename alcard-boot-zero)"
        "$PKGS/$(alcard_package filename alcard-bsp-zero)"
        "$PKGS/$(alcard_package filename "$BOARD")"
    )
    local f n v
    for f in "${files[@]}"; do [ -s "$f" ] || fail "Missing $f"; done
    apk verify "${files[@]}" || fail 'APK signature/checksum failure'
    rm -rf -- "$PAYLOAD"
    mkdir -p -- "$PAYLOAD"
    for f in "${files[@]}"; do
        apk extract --no-chown --destination "$PAYLOAD" "$f" || fail "Extraction failed: $f"
    done

    local mods="$PAYLOAD/usr/lib/modules/$ABI/extra/alcard"
    [ -d "$mods" ] || fail 'Missing kernel module directory'
    n="$(find "$mods" -type f -name '*.ko' | wc -l | tr -d ' ')"
    [ "$n" -eq 12 ] || fail "Expected 12 kernel modules; got $n"
    for f in "$mods"/*.ko; do
        v="$(modinfo -F vermagic "$f" | cut -d ' ' -f1)"
        [ "$v" = "$ABI" ] || fail "Invalid vermagic $v: $f"
    done
    local lcd="$PAYLOAD/usr/lib/firmware/cardputerzero,st7789v.bin"
    local bmi="$PAYLOAD/usr/lib/firmware/bmi270-init-data.fw"
    [ -f "$lcd" ] && [ "$(wc -c < "$lcd")" -eq 93 ] || fail 'LCD firmware missing/invalid size'
    [ -f "$bmi" ] && [ "$(wc -c < "$bmi")" -eq 8192 ] || fail 'BMI270 firmware missing/invalid size'
    printf '%s  %s\n' \
        cb8a403f7f7648622b386f312a5dd3e42d706c3419e1fd5216a36289d399fbe2 "$lcd" \
        2d75e68e343a13ff99be98261dfbd99d9e8c6f267da9e3c5aeb883276ad178db "$bmi" |
        sha256sum -c - || fail 'Firmware checksum mismatch'
    [ -s "$PAYLOAD/usr/share/licenses/alcard-firmware-zero/Bosch-BMI270-BSD-3-Clause.txt" ] ||
        fail 'Bosch notice not present'
    for f in alcard-zero-v5 alcard-bq27220-v5 alcard-bmi270-bmm150 alcard-bmi270-only; do
        [ -s "$PAYLOAD/boot/overlays/$f.dtbo" ] || fail "Missing $f.dtbo"
    done
    [ -s "$PAYLOAD/boot/alcard.cfg" ] || fail 'Missing board profile'
    local cfg="$PAYLOAD/boot/alcard.cfg"
    grep -qx 'dtoverlay=alcard-zero-v5' "$cfg" || fail 'Main board overlay missing'
    grep -qx 'dtoverlay=alcard-bq27220-v5' "$cfg" || fail 'BQ27220 overlay missing'
    if [[ "$PROFILE" == v5 ]]; then
        grep -qx 'dtoverlay=alcard-bmi270-bmm150' "$cfg" ||
            fail 'Full v5 must activate BMI270/BMM150'
    else
        ! grep -Eq '^dtoverlay=(alcard-bmi270|imx219)' "$cfg" ||
            fail 'Lite profile enables absent IMU/camera'
    fi
    log "PASS: $BOARD, signatures, 12 modules, firmware, notices and DTBO"
}

check_dt() {
    log "Merging overlays with $BASE"
    [ -s "$BASE" ] || fail "Missing CM0 DTB: $BASE; set ALCARD_BASE_DTB if needed"
    local overlays=(
        "$PAYLOAD/boot/overlays/alcard-zero-v5.dtbo"
        "$PAYLOAD/boot/overlays/alcard-bq27220-v5.dtbo"
    )
    if [[ "$PROFILE" == v5 ]]; then
        overlays+=("$PAYLOAD/boot/overlays/alcard-bmi270-bmm150.dtbo")
    fi
    fdtoverlay -i "$BASE" -o "$OUT/merged-$PROFILE.dtb" "${overlays[@]}" ||
        fail 'DT merge failed'
    local t="$OUT/merged-$PROFILE.dtb" bus='/soc/spi@7e204000' compat
    compat="$(fdtget -t s "$t" "$bus/display@0" compatible)" || fail 'LCD missing'
    [[ "$compat" == *cardputerzero,st7789v* && "$compat" == *panel-mipi-dbi-spi* ]] ||
        fail "Wrong LCD compatible: $compat"
    [ "$(fdtget -t x "$t" "$bus/display@0" reg)" = 0 ] || fail 'LCD not on CS0'
    [ "$(fdtget -t s "$t" "$bus/spidev@0" status)" = disabled ] || fail 'spidev@0 active'
    if [[ "$PROFILE" == v5 ]]; then
        [ "$(fdtget -t s "$t" /magnetometer compatible)" = bosch,bmm150-bmi270 ] ||
            fail 'BMM150 bridge missing'
        [ "$(fdtget -t x "$t" /magnetometer bosch,aux-i2c-addr)" = 10 ] ||
            fail 'Wrong BMM150 AUX address'
    else
        ! fdtget "$t" /magnetometer compatible >/dev/null 2>&1 ||
            fail 'Lite has unexpected BMM150 bridge'
        ! fdtget "$t" /soc/i2c@7e804000/imu@68 compatible >/dev/null 2>&1 ||
            fail 'Lite has unexpected BMI270'
    fi
    log "PASS: CM0 DT + $PROFILE overlays (LCD CS0, spidev disabled, IMU as specified)"
}

kconfig() {
    local v
    v="$(grep -m1 "^$1=" "$CFG" || :)"
    if [ -z "$v" ]; then printf n; else printf '%s' "${v#*=}"; fi
}

check_kernel() {
    log "Checking DRM/fbcon against installed linux-rpi-dev $ABI"
    [ -r "$CFG" ] || fail "Missing $CFG"
    local key v bad=0
    for key in \
        CONFIG_DRM CONFIG_DRM_MIPI_DBI CONFIG_DRM_PANEL_MIPI_DBI \
        CONFIG_DRM_FBDEV_EMULATION CONFIG_FRAMEBUFFER_CONSOLE \
        CONFIG_VT CONFIG_VT_CONSOLE CONFIG_SPI CONFIG_SPI_BCM2835 \
        CONFIG_FW_LOADER CONFIG_BACKLIGHT_CLASS_DEVICE CONFIG_PWM; do
        v="$(kconfig "$key")"
        printf '    %-39s %s\n' "$key" "$v"
        case "$v" in y|m) ;; *) bad=1 ;; esac
    done
    [ "$bad" = 0 ] || fail 'Kernel lacks one or more required DRM/console options'
    log 'PASS: required flags enabled (boot on real hardware not yet verified)'
}

check_setup
check_packages
check_dt
check_kernel
