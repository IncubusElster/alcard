#!/usr/bin/env bash
# Build a signed, dependency-only Alcard BSP APK in the existing ARM64 Distrobox.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
source "$ROOT/scripts/package-metadata.sh"
LOCAL="$ROOT/.alcard-local"
RECIPE="$ROOT/packaging/apk/alcard-bsp-zero/APKBUILD"
WORK_DIR="$LOCAL/work/aports/alcard/alcard-bsp-zero"
OUTPUT_DIR="$LOCAL/packages"
REPO="$OUTPUT_DIR/alcard/aarch64"

log() { printf '==> %s\n' "$*"; }
fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

recipe_value() {
    sed -n "s/^${1}=//p" "$RECIPE" | head -n 1 | tr -d "'\""
}

check_environment() {
    [[ -r "$RECIPE" ]] || fail 'Missing alcard-bsp-zero APKBUILD'
    command -v apk >/dev/null || fail 'Missing apk'
    [[ "$(apk --print-arch)" == aarch64 ]] || fail 'Expected Alpine aarch64 Distrobox'
    [[ "$(id -u)" -ne 0 ]] || fail 'Do not run abuild as root'
    local c
    for c in abuild sed grep python3; do
        command -v "$c" >/dev/null || fail "Missing $c"
    done
    apk info -e alpine-sdk >/dev/null 2>&1 || fail 'Install alpine-sdk once in Distrobox'
    local item
    for item in \
        "$(alcard_package filename alcard-kmods-rpi)" \
        "$(alcard_package filename alcard-firmware-zero)" \
        "$(alcard_package filename alcard-boot-zero)"; do
        [[ -s "$REPO/$item" ]] || fail "Build component APK first: $item"
    done
    log 'Meta BSP prerequisites present in existing signed APK repository'
}

build_package() {
    check_environment
    local ver rel result
    ver="$(recipe_value pkgver)"
    rel="$(recipe_value pkgrel)"
    [[ "$ver" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && "$rel" =~ ^[0-9]+$ ]] ||
        fail 'Invalid metapackage version'
    result="$REPO/alcard-bsp-zero-${ver}-r${rel}.apk"
    rm -rf -- "$WORK_DIR"
    mkdir -p -- "$WORK_DIR" "$REPO"
    cp -- "$RECIPE" "$WORK_DIR/APKBUILD"
    (
        cd "$WORK_DIR"
        abuild checksum
        /bin/sh -n APKBUILD || fail 'Invalid metapackage APKBUILD'
        REPODEST="$OUTPUT_DIR" abuild -d
    )
    [[ -s "$result" ]] || fail "Missing built APK: $result"
    log "Built: ${result#"$ROOT"/}"
}

case "${1:-build}" in
    check) check_environment ;;
    build) build_package ;;
    *) fail 'Usage: scripts/build-bsp-zero.sh [check|build]' ;;
esac
