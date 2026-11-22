#!/usr/bin/env bash
# Signed, Alcard Pixel PSF2 APK (noarch package built inside aarch64 repo) in the existing Alpine aarch64 Distrobox.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
source "$ROOT/scripts/package-metadata.sh"
LOCAL="$ROOT/.alcard-local"
PKG=alcard-fonts-console
VER="$(alcard_package pkgver "$PKG")"
WORK="$LOCAL/work/aports/alcard/$PKG"
OUT="$LOCAL/packages/alcard/aarch64"
fail() { echo "ERROR: $*" >&2; exit 1; }
check() {
  [[ "$(apk --print-arch)" == aarch64 ]] || fail "Requires Alpine aarch64 Distrobox"
  [[ "$(id -u)" != 0 ]] || fail "Do not run abuild as root"
  for c in abuild python3 tar gzip; do command -v "$c" >/dev/null || fail "Missing $c"; done
  apk info -e alpine-sdk >/dev/null 2>&1 || fail "Install alpine-sdk"
  [[ -s "$ROOT/fonts/alcard-pixel/source/AlcardPixel-Regular.hex" ]] || fail "Missing Regular HEX"
  [[ -s "$ROOT/fonts/alcard-pixel/source/AlcardPixel-Bold.hex" ]] || fail "Missing Bold HEX"
  [[ -s "$ROOT/tools/build-alcard-fonts.py" ]] || fail "Missing PSF2 converter"
  echo "==> $PKG build prerequisites OK"
}
build() {
  check
  mkdir -p "$WORK" "$OUT"
  cp "$ROOT/packaging/apk/$PKG/APKBUILD" "$WORK/APKBUILD"
  python3 - "$ROOT" "$WORK/$PKG-$VER.tar.gz" "$PKG-$VER" <<'PY'
import gzip, io, pathlib, sys, tarfile
root=pathlib.Path(sys.argv[1]);out=pathlib.Path(sys.argv[2]);prefix=sys.argv[3]
paths={
 "fonts/AlcardPixel-Regular.hex":root/"fonts/alcard-pixel/source/AlcardPixel-Regular.hex",
 "fonts/AlcardPixel-Bold.hex":root/"fonts/alcard-pixel/source/AlcardPixel-Bold.hex",
 "tools/build-alcard-fonts.py":root/"tools/build-alcard-fonts.py",
}
with open(out,"wb") as f:
 with gzip.GzipFile(fileobj=f,mode="wb",filename="",mtime=0) as gz:
  with tarfile.open(fileobj=gz,mode="w|",format=tarfile.USTAR_FORMAT) as tar:
   for name,path in sorted(paths.items()):
    data=path.read_bytes()
    info=tarfile.TarInfo(f"{prefix}/{name}")
    info.size=len(data);info.mode=0o644
    info.uid=info.gid=info.mtime=0
    tar.addfile(info,io.BytesIO(data))
PY
  (cd "$WORK" && abuild checksum && /bin/sh -n APKBUILD && REPODEST="$LOCAL/packages" abuild -d)
  [[ -s "$OUT/$(alcard_package filename "$PKG")" ]] || fail "Missing signed font APK"
  echo "==> Ready: $OUT/$(alcard_package filename "$PKG")"
}
case "${1:-}" in check) check ;; build) build ;; *) fail "Usage: $0 check|build" ;; esac
