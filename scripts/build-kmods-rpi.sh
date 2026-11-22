#!/usr/bin/env bash
# Build alcard-kmods-rpi with an ALREADY provisioned Alpine aarch64 Distrobox.
# Does not create containers or install/update Alpine packages.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
LOCAL="$ROOT/.alcard-local"
RECIPE="$ROOT/packaging/apk/alcard-kmods-rpi/APKBUILD"
SOURCE_DIR="$LOCAL/sources/m5stack-linux-dtoverlays"
WORK_DIR="$LOCAL/work/aports/alcard/alcard-kmods-rpi"
CACHE_DIR="$LOCAL/cache"
OUTPUT_DIR="$LOCAL/packages"

log() { printf '==> %s\n' "$*"; }
fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
require() { command -v "$1" >/dev/null 2>&1 || fail "Missing required tool in ARM64 Distrobox: $1"; }

get_recipe_field() {
  local key="$1"
  sed -n "s/^${key}=//p" "$RECIPE" | head -n 1 | tr -d "'\""
}

kconfig_value() {
  local key="$1" cfg="$2" found
  found="$(grep -E "^${key}=" "$cfg" | head -n 1 || :)"
  if [[ -n "$found" ]]; then
    printf '%s' "${found#*=}"
  else
    printf 'n'
  fi
}

check_environment() {
  [[ -f "$RECIPE" ]] || fail "Missing APKBUILD: $RECIPE"
  require apk
  [[ "$(apk --print-arch)" == aarch64 ]] || fail 'Use a persistent aarch64 Alpine Distrobox'
  [[ "$(id -u)" -ne 0 ]] || fail 'Run unprivileged (abuild must not run as root)'

  local cmd pkgver kernrel pkgrel kernel_abi installed m5rev hdr config value
  for cmd in git abuild make gcc bash tar gzip sha512sum sed awk modinfo; do require "$cmd"; done

  pkgver="$(get_recipe_field pkgver)"
  kernrel="$(get_recipe_field _kernrel)"
  pkgrel="$(get_recipe_field pkgrel)"
  m5rev="$(get_recipe_field _m5rev)"
  [[ "$pkgver" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail 'Invalid kernel pkgver in APKBUILD'
  [[ "$kernrel" =~ ^[0-9]+$ && "$pkgrel" =~ ^[0-9]+$ ]] || fail 'Invalid revision in APKBUILD'
  [[ "$m5rev" =~ ^[[:xdigit:]]{40}$ ]] || fail 'Invalid pinned M5Stack SHA in APKBUILD'
  kernel_abi="${pkgver}-${kernrel}-rpi"

  local package_name
  for package_name in alpine-sdk linux-rpi-dev linux-rpi kmod git bash; do
    apk info -e "$package_name" >/dev/null 2>&1 ||
      fail "Install $package_name once in the aarch64 Alpine Distrobox"
  done
  # apk info -v PACKAGE returns a human-readable description in apk-tools 3.
  # apk list --installed --manifest returns stable "name version" fields.
  for package_name in linux-rpi linux-rpi-dev; do
    installed="$(apk list --installed --manifest "$package_name" |
      awk -v name="$package_name" '$1 == name { print $2; exit }')"
    [[ "$installed" == "${pkgver}-r${kernrel}" ]] ||
      fail "Expected $package_name=${pkgver}-r${kernrel}, got ${installed:-<not found>}"
  done

  hdr="/usr/src/linux-headers-${kernel_abi}"
  config="$hdr/.config"
  [[ -r "$config" && -s "$hdr/Module.symvers" ]] ||
    fail "Missing matching kernel config/symbols under $hdr"
  [[ "$(cat /usr/share/kernel/rpi/kernel.release 2>/dev/null)" == "$kernel_abi" ]] ||
    fail "Installed kernel release metadata does not match $kernel_abi"

  log "Kernel: linux-rpi ${pkgver}-r${kernrel} (ABI ${kernel_abi})"
  log "M5Stack revision: $m5rev"
  log "Build/cache: ${LOCAL#"$ROOT"/} (ignored by Git)"
  log 'Kernel driver selection (from installed linux-rpi-dev .config):'
  for value in \
    CONFIG_BATTERY_BQ27XXX CONFIG_BATTERY_BQ27XXX_I2C \
    CONFIG_BMI270 CONFIG_BMI270_I2C CONFIG_BMC150_MAGN \
    CONFIG_SND_SOC_SIMPLE_AMPLIFIER; do
    printf '    %-37s %s\n' "$value" "$(kconfig_value "$value" "$config")"
  done

  # A generic enabled BQ27xxx would conflict with the vendor replacement.
  if [[ "$(kconfig_value CONFIG_BATTERY_BQ27XXX "$config")" != n ||
        "$(kconfig_value CONFIG_BATTERY_BQ27XXX_I2C "$config")" != n ]]; then
    fail 'BQ27220 conflict: kernel generic BQ27xxx is enabled, but has no ti,bq27220 support. Coordinate an Alpine kernel patch/config before building the complete vendor module package.'
  fi
}

ensure_sources() {
  local rev="$1"
  mkdir -p "$LOCAL/sources" "$CACHE_DIR" "$OUTPUT_DIR" "$(dirname "$WORK_DIR")"

  if [[ ! -d "$SOURCE_DIR/.git" ]]; then
    [[ ! -e "$SOURCE_DIR" ]] || fail "$SOURCE_DIR exists but is not a Git checkout"
    log 'Cloning M5Stack BSP into ignored local cache'
    git clone --no-checkout --depth 1 \
      https://github.com/m5stack/m5stack-linux-dtoverlays.git "$SOURCE_DIR"
  fi
  if ! git -C "$SOURCE_DIR" cat-file -e "${rev}^{commit}" 2>/dev/null; then
    log "Fetching pinned M5Stack revision $rev"
    git -C "$SOURCE_DIR" fetch --depth 1 origin "$rev"
  fi
  git -C "$SOURCE_DIR" cat-file -e "${rev}^{commit}" 2>/dev/null ||
    fail "M5Stack commit $rev not available"
}

build_package() {
  check_environment
  local pkgver pkgrel rev prefix archive cached tmp expected
  pkgver="$(get_recipe_field pkgver)"
  pkgrel="$(get_recipe_field pkgrel)"
  rev="$(get_recipe_field _m5rev)"
  prefix="m5stack-linux-dtoverlays-${rev}"
  archive="${prefix}.tar.gz"
  cached="$CACHE_DIR/$archive"
  expected="$OUTPUT_DIR/alcard/aarch64/alcard-kmods-rpi-${pkgver}-r${pkgrel}.apk"

  ensure_sources "$rev"
  if [[ ! -s "$cached" ]]; then
    log 'Creating immutable source tarball from pinned Git commit'
    tmp="$(mktemp "$CACHE_DIR/.m5stack.XXXXXXXX")"
    trap 'rm -f "$tmp"' EXIT
    git -C "$SOURCE_DIR" archive --format=tar --prefix="${prefix}/" "$rev" |
      gzip -n > "$tmp"
    mv -f "$tmp" "$cached"
    trap - EXIT
  fi

  # Never mutate tracked APKBUILD. abuild checksum and compiled objects stay
  # strictly inside .alcard-local; output is in a distinct local directory.
  rm -rf -- "$WORK_DIR"
  mkdir -p "$WORK_DIR" "$(dirname "$expected")"
  cp -- "$RECIPE" "$WORK_DIR/APKBUILD"
  cp -- "$cached" "$WORK_DIR/$archive"
  # Out-of-tree M5Stack bridge is patched reproducibly; never modify cached Git sources.
  cp -- "$ROOT/patches/m5stack/0001-bmi270-aux-address-property.patch" "$WORK_DIR/"
  (
    cd "$WORK_DIR"
    abuild checksum
    # A malformed checksum append must fail before invoking the package builder.
    /bin/sh -n APKBUILD || fail 'APKBUILD syntax invalid after abuild checksum'
    log 'Building signed ARM64 kernel-module APK (abuild)'
    # All build dependencies are installed in the persistent container.
    # -d skips dependency installation entirely (sudo/setuid is unavailable).
    REPODEST="$OUTPUT_DIR" abuild -d
  )
  [[ -s "$expected" ]] || fail "APK missing after abuild: $expected"
  log "Built: ${expected#"$ROOT"/}"
  log 'Package includes only external modules and driver-source selection manifest.'
}

case "${1:-build}" in
  check) check_environment ;;
  build) build_package ;;
  *) fail 'Usage: scripts/build-kmods-rpi.sh [check|build]' ;;
esac
