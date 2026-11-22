#!/usr/bin/env bash
# Build alcard-boot-zero using an already provisioned Alpine aarch64 Distrobox.
# No container creation, package installation, host mutation or boot flashing.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
LOCAL="$ROOT/.alcard-local"
RECIPE="$ROOT/packaging/apk/alcard-boot-zero/APKBUILD"
WORK_DIR="$LOCAL/work/aports/alcard/alcard-boot-zero"
OUTPUT_DIR="$LOCAL/packages"

log() { printf '==> %s\n' "$*"; }
fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
require() { command -v "$1" >/dev/null 2>&1 || fail "Missing tool in Alpine ARM64 Distrobox: $1"; }

recipe_value() {
    local key="$1"
    sed -n "s/^${key}=//p" "$RECIPE" | head -n 1 | tr -d "'\""
}

check_environment() {
    [[ -r "$RECIPE" ]] || fail "Missing APKBUILD: $RECIPE"
    require apk
    [[ "$(apk --print-arch)" == "aarch64" ]] || fail "Use a persistent Alpine aarch64 Distrobox"
    [[ "$(id -u)" -ne 0 ]] || fail "Run as an unprivileged abuild user"

    local cmd pkg
    for cmd in abuild dtc python3 tar gzip sha512sum sed grep; do require "$cmd"; done
    for pkg in alpine-sdk dtc python3; do
        apk info -e "$pkg" >/dev/null 2>&1 ||
            fail "Install $pkg once manually inside the alcard-arm64 Distrobox"
    done
    python3 -c 'import sys; assert sys.version_info >= (3, 11)' ||
        fail "Python >=3.11 required"
    [[ -r "$ROOT/board/cardputer-zero/device-tree/alcard-zero-v5.dts" ]] ||
        fail 'Missing Alcard v5 source DTS'
    log "Boot package $(recipe_value pkgver)-r$(recipe_value pkgrel); work under .alcard-local"
}

pack_sources() {
    local version="$1" target="$2"
    python3 - "$ROOT" "$version" "$target" <<'PY'
import gzip
import io
from pathlib import Path
import sys
import tarfile

root, version, target = Path(sys.argv[1]), sys.argv[2], Path(sys.argv[3])
paths = [
    "board/cardputer-zero/device-tree/alcard-zero-v5.dts",
    "board/cardputer-zero/device-tree/bq27220-v5.dts",
    "board/cardputer-zero/device-tree/bmi270-bmm150.dts",
    "board/cardputer-zero/device-tree/bmi270-only.dts",
    "packaging/apk/alcard-boot-zero/README.md",
]
with target.open("wb") as output:
    with gzip.GzipFile(fileobj=output, mode="wb", filename="", compresslevel=9, mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode="w|", format=tarfile.USTAR_FORMAT) as archive:
            for relative in sorted(paths):
                data = (root / relative).read_bytes()
                info = tarfile.TarInfo(f"alcard-boot-zero-{version}/{relative}")
                info.size = len(data)
                info.mode = 0o644
                info.mtime = 0
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                archive.addfile(info, io.BytesIO(data))
PY
}

build_package() {
    check_environment
    local version rel expected
    version="$(recipe_value pkgver)"
    rel="$(recipe_value pkgrel)"
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && "$rel" =~ ^[0-9]+$ ]] ||
        fail "Invalid version in APKBUILD"
    expected="$OUTPUT_DIR/alcard/aarch64/alcard-boot-zero-${version}-r${rel}.apk"

    rm -rf -- "$WORK_DIR"
    mkdir -p -- "$WORK_DIR" "$(dirname "$expected")"
    cp -- "$RECIPE" "$WORK_DIR/APKBUILD"
    pack_sources "$version" "$WORK_DIR/alcard-boot-zero-${version}.tar.gz"

    (
        cd "$WORK_DIR"
        abuild checksum
    # A malformed checksum append must fail before invoking the package builder.
    /bin/sh -n APKBUILD || fail 'APKBUILD syntax invalid after abuild checksum'
        log 'Building signed boot APK (does not modify host /boot)'
        # All build dependencies are installed in the persistent container.
    # -d skips dependency installation entirely (sudo/setuid is unavailable).
    REPODEST="$OUTPUT_DIR" abuild -d
    )
    [[ -s "$expected" ]] || fail "Expected APK missing: $expected"
    log "Built: ${expected#"$ROOT"/}"
}

case "${1:-build}" in
    check) check_environment ;;
    build) build_package ;;
    *) fail "Usage: scripts/build-boot-zero.sh [check|build]" ;;
esac
