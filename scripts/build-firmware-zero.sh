#!/usr/bin/env bash
# Assemble alcard-firmware-zero APK in a PREPARED persistent Alpine aarch64 Distrobox.
# Never installs Alpine packages or creates containers. All paths derive from repo root.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
LOCAL="$ROOT/.alcard-local"
RECIPE="$ROOT/packaging/apk/alcard-firmware-zero/APKBUILD"
SOURCE_DIR="$LOCAL/sources/m5stack-linux-dtoverlays"
WORK_DIR="$LOCAL/work/aports/alcard/alcard-firmware-zero"
CACHE_DIR="$LOCAL/cache"
OUTPUT_DIR="$LOCAL/packages"

log() { printf '==> %s\n' "$*"; }
fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
require() { command -v "$1" >/dev/null 2>&1 || fail "Missing tool in ARM64 Distrobox: $1"; }

get_recipe_field() {
  local key="$1"
  sed -n "s/^${key}=//p" "$RECIPE" | head -n 1 | tr -d "'\""
}

check_environment() {
  [[ -r "$RECIPE" ]] || fail "Missing APKBUILD: $RECIPE"
  require apk
  [[ "$(apk --print-arch)" == aarch64 ]] || fail 'Use an Alpine aarch64 Distrobox'
  [[ "$(id -u)" -ne 0 ]] || fail 'Run as a normal user (abuild does not run as root)'

  local cmd version pyver rev
  for cmd in git abuild python3 tar gzip sha256sum sha512sum sed; do require "$cmd"; done
  for cmd in alpine-sdk python3; do
    apk info -e "$cmd" >/dev/null 2>&1 || fail "Install $cmd once inside alcard-arm64"
  done
  python3 -c 'import sys, tomllib; assert sys.version_info >= (3, 11)' ||
    fail 'Python >= 3.11 with tomllib is required (Alpine 3.24 provides this)'

  version="$(get_recipe_field pkgver)"
  rev="$(get_recipe_field _m5rev)"
  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail 'Invalid APK pkgver'
  [[ "$rev" =~ ^[[:xdigit:]]{40}$ ]] || fail 'Invalid pinned M5Stack SHA'
  [[ -r "$ROOT/board/cardputer-zero/firmware/st7789v3.toml" ]] || fail 'Missing ST7789 TOML'
  [[ -r "$ROOT/tools/mipi-dbi-firmware.py" ]] || fail 'Missing MIPI DBI generator'
  log "Build environment: Alpine $(cat /etc/alpine-release), $(apk --print-arch)"
  log "Firmware package: $version; M5Stack pinned to $rev"
  log "Working path: ${LOCAL#"$ROOT"/} (ignored by Git)"
}

ensure_sources() {
  local rev="$1"
  mkdir -p "$LOCAL/sources" "$CACHE_DIR" "$OUTPUT_DIR" "$(dirname "$WORK_DIR")"
  if [[ ! -d "$SOURCE_DIR/.git" ]]; then
    [[ ! -e "$SOURCE_DIR" ]] || fail "$SOURCE_DIR exists but is not a Git checkout"
    log 'Cloning M5Stack BSP into reusable local cache'
    git clone --depth 1 --no-checkout \
      https://github.com/m5stack/m5stack-linux-dtoverlays.git "$SOURCE_DIR"
  fi
  if ! git -C "$SOURCE_DIR" cat-file -e "${rev}^{commit}" 2>/dev/null; then
    log "Fetching missing pinned M5Stack commit: $rev"
    git -C "$SOURCE_DIR" fetch --depth 1 origin "$rev"
  fi
  git -C "$SOURCE_DIR" cat-file -e "${rev}^{commit}" 2>/dev/null ||
    fail "Pinned M5Stack commit missing: $rev"
}

archive_m5stack() {
  local rev="$1" prefix="m5stack-linux-dtoverlays-$1"
  local archive="$CACHE_DIR/$prefix.tar.gz" tmp
  if [[ ! -s "$archive" ]]; then
    tmp="$(mktemp "$CACHE_DIR/.m5stack.XXXXXXXX")"
    trap 'rm -f "$tmp"' EXIT
    git -C "$SOURCE_DIR" archive --format=tar --prefix="$prefix/" "$rev" | gzip -n > "$tmp"
    mv -f "$tmp" "$archive"
    trap - EXIT
  fi
  printf '%s\n' "$archive"
}

archive_alcard_sources() {
  local version="$1" archive="$2"
  # Archive the current working tree, not git HEAD. Fixed metadata makes the
  # tarball reproducible even when source file mtimes or host UID differ.
  python3 - "$ROOT" "$version" "$archive" <<'PY'
import gzip
import io
from pathlib import Path
import sys
import tarfile

root, version, dest = Path(sys.argv[1]), sys.argv[2], Path(sys.argv[3])
paths = [
    'board/cardputer-zero/firmware/st7789v3.toml',
    'licenses/Bosch-BMI270-BSD-3-Clause.txt',
    'tools/mipi-dbi-firmware.py',
    'packaging/apk/alcard-firmware-zero/README.md',
]
with dest.open('wb') as fh:
    with gzip.GzipFile(filename='', mode='wb', compresslevel=9, mtime=0, fileobj=fh) as gz:
        with tarfile.open(mode='w|', fileobj=gz, format=tarfile.USTAR_FORMAT) as tar:
            for name in sorted(paths):
                content = (root / name).read_bytes()
                info = tarfile.TarInfo(f'alcard-firmware-zero-{version}/{name}')
                info.size = len(content)
                info.mtime = 0
                info.mode = 0o644
                info.uid = info.gid = 0
                info.uname = info.gname = ''
                tar.addfile(info, io.BytesIO(content))
PY
}

build_package() {
  check_environment
  local version rel rev vendor_archive local_archive expected
  version="$(get_recipe_field pkgver)"
  rel="$(get_recipe_field pkgrel)"
  rev="$(get_recipe_field _m5rev)"
  [[ "$rel" =~ ^[0-9]+$ ]] || fail 'Invalid APK pkgrel'
  expected="$OUTPUT_DIR/alcard/aarch64/alcard-firmware-zero-${version}-r${rel}.apk"

  ensure_sources "$rev"
  vendor_archive="$(archive_m5stack "$rev")"
  [[ -s "$vendor_archive" ]] || fail 'Missing M5Stack source archive'
  rm -rf -- "$WORK_DIR"
  mkdir -p "$WORK_DIR" "$(dirname "$expected")"
  cp -- "$RECIPE" "$WORK_DIR/APKBUILD"
  cp -- "$vendor_archive" "$WORK_DIR/"
  local_archive="$WORK_DIR/alcard-firmware-zero-${version}.tar.gz"
  archive_alcard_sources "$version" "$local_archive"

  # Always regenerate checksums in ignored workspace; tracked recipe stays clean.
  (
    cd "$WORK_DIR"
    abuild checksum
    # A malformed checksum append must fail before invoking the package builder.
    /bin/sh -n APKBUILD || fail 'APKBUILD syntax invalid after abuild checksum'
    log 'Building firmware package through abuild'
    # All build dependencies are installed in the persistent container.
    # -d skips dependency installation entirely (sudo/setuid is unavailable).
    REPODEST="$OUTPUT_DIR" abuild -d
  )
  [[ -s "$expected" ]] || fail "Firmware APK not found: $expected"
  log "Package built: ${expected#"$ROOT"/}"
  log 'Includes ST7789 MIPI DBI commands and pinned BMI270 firmware.'
  log 'Bosch BMI270 is distributable under BSD-3-Clause with the bundled license.'
}

case "${1:-build}" in
  check) check_environment ;;
  build) build_package ;;
  *) fail 'Usage: scripts/build-firmware-zero.sh [check|build]' ;;
esac
