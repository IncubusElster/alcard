#!/usr/bin/env bash
# Build signed Cardputer Zero US/RU/ES/DE keyboard APK in the existing Alpine aarch64 Distrobox.
# Host /etc, keyboard, boot and kernel module state are never touched.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
LOCAL="$ROOT/.alcard-local"
PKG=alcard-keyboard-zero
WORK="$LOCAL/work/aports/alcard/$PKG"
REPO="$LOCAL/packages/alcard/aarch64"
RECIPE="$ROOT/packaging/apk/$PKG/APKBUILD"

fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
value() {
  sed -n "s/^$1=//p" "$RECIPE" | head -1 | tr -d '"'
}
check() {
  [[ "$(id -u)" -ne 0 ]] || fail 'Do not run as root'
  [[ "$(apk --print-arch)" == aarch64 ]] || fail 'Requires Alpine aarch64 Distrobox'
  local name
  for name in alpine-sdk kbd python3; do
    apk info -e "$name" >/dev/null 2>&1 ||
      fail "Missing Alpine build package: $name. Install it once inside alcard-arm64 as root (e.g. on host: podman exec --user 0 alcard-arm64 apk add $name)"
  done
  for name in abuild apk python3 tar gzip; do
    command -v "$name" >/dev/null || fail "Missing build tool: $name"
  done
  # Alpine's 'kbd' package provides /usr/bin/loadkeys. abuild -d skips
  # automatic dependency installation, so it must exist in the Distrobox.
  command -v loadkeys >/dev/null ||
    fail "Missing loadkeys from Alpine 'kbd'. Install/reinstall kbd inside alcard-arm64 (host: podman exec --user 0 alcard-arm64 apk add kbd)"
  for name in us ru es de; do
    [[ -s "$ROOT/keyboard-zero/console/$name.map" ]] || fail "Missing $name TTY map"
  done
  [[ -s "$ROOT/keyboard-zero/symbols/alcard" &&
     -s "$ROOT/keyboard-zero/layouts.json" &&
     -s "$ROOT/keyboard-zero/verify-keyboard.py" ]] || fail 'Missing keyboard data'
  echo "==> alcard-keyboard-zero prerequisites OK"
}
build() {
  check
  local ver rel
  ver="$(value pkgver)"
  rel="$(value pkgrel)"
  [[ "$ver" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ &&
     "$rel" =~ ^[0-9]+$ ]] || fail 'Invalid APKBUILD version'
  rm -rf -- "$WORK"
  mkdir -p -- "$WORK" "$REPO"
  cp -- "$RECIPE" "$WORK/APKBUILD"
  python3 - "$ROOT" "$WORK/$PKG-$ver.tar.gz" "$PKG-$ver" <<'PY'
import gzip, io, pathlib, sys, tarfile
root, target, prefix = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3]
paths = {
 "layouts.json":"keyboard-zero/layouts.json",
 "verify-keyboard.py":"keyboard-zero/verify-keyboard.py",
 "symbols/alcard":"keyboard-zero/symbols/alcard",
 **{f"console/{lang}.map":f"keyboard-zero/console/{lang}.map"
    for lang in ("us","ru","es","de")},
}
with target.open("wb") as stream:
 with gzip.GzipFile(fileobj=stream,mode="wb",filename="",mtime=0) as zipstream:
  with tarfile.open(fileobj=zipstream,mode="w|",format=tarfile.USTAR_FORMAT) as archive:
   for dest, source in sorted(paths.items()):
    data=(root/source).read_bytes()
    info=tarfile.TarInfo(f"{prefix}/{dest}")
    info.size=len(data); info.mode=0o644
    info.mtime=0; info.uid=info.gid=0
    archive.addfile(info,io.BytesIO(data))
PY
  (
    cd "$WORK"
    abuild checksum
    /bin/sh -n APKBUILD || fail 'Invalid APKBUILD'
    REPODEST="$LOCAL/packages" abuild -d
  )
  [[ -s "$REPO/$PKG-$ver-r$rel.apk" ]] || fail 'Signed keyboard APK missing'
  echo "==> Built: $REPO/$PKG-$ver-r$rel.apk"
}
case "${1:-}" in
check) check ;;
build) build ;;
*) fail 'Usage: build-keyboard-zero.sh check|build' ;;
esac
