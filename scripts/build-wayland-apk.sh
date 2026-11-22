#!/usr/bin/env bash
# Build optional signed Alcard Wayland package in existing Alpine aarch64 Distrobox.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
source "$ROOT/scripts/package-metadata.sh"
LOCAL="$ROOT/.alcard-local"
profile="${2:-}"
case "$profile" in
    fonts-gui) PKG=alcard-fonts-gui ;;
    desktop) PKG=alcard-desktop-minimal ;;
    *) echo "Usage: $0 check|build fonts-gui|desktop" >&2; exit 2 ;;
esac
VER="$(alcard_package pkgver "$PKG")"
APK_FILE="$(alcard_package filename "$PKG")"
WORK="$LOCAL/work/aports/alcard/$PKG"
OUT="$LOCAL/packages/alcard/aarch64"
fail() { echo "ERROR: $*" >&2; exit 1; }
check() {
    [[ "$(apk --print-arch)" == aarch64 ]] || fail "Requires Alpine aarch64 Distrobox"
    [[ "$(id -u)" != 0 ]] || fail "Never run abuild as root"
    for command in abuild python3 tar gzip; do
        type -P "$command" >/dev/null || fail "Missing $command"
    done
    apk info -e alpine-sdk >/dev/null 2>&1 || fail "Install alpine-sdk"
    [[ -s "$ROOT/packaging/apk/$PKG/APKBUILD" ]] || fail "Missing APKBUILD"
    case "$profile" in
        fonts-gui)
            python3 -c 'from fontTools.ttLib import TTFont' ||
                fail "Install py3-fonttools inside Distrobox"
            for file in "$ROOT/tools/build-alcard-fonts.py" \
                  "$ROOT/fonts/alcard-pixel/source/AlcardPixel-Regular.hex" \
                  "$ROOT/fonts/alcard-pixel/source/AlcardPixel-Bold.hex"; do
                [[ -s "$file" ]] || fail "Missing font input: $file"
            done
            ;;
        desktop)
            [[ -s "$OUT/$(alcard_package filename alcard-fonts-gui)" ]] ||
                fail "Build GUI fonts first: ./build.sh fonts-gui"
            for name in wayland-session autostart environment foot.ini; do
                [[ -s "$ROOT/scripts/wayland/$name" ]] ||
                    fail "Missing Wayland profile: $name"
            done
            ;;
    esac
    echo "==> $PKG prerequisites OK"
}
build() {
    check
    mkdir -p "$WORK" "$OUT"
    cp "$ROOT/packaging/apk/$PKG/APKBUILD" "$WORK/APKBUILD"
    python3 - "$ROOT" "$WORK/$PKG-$VER.tar.gz" "$PKG-$VER" "$profile" <<'PY'
import gzip, io, pathlib, sys, tarfile
root=pathlib.Path(sys.argv[1])
out=pathlib.Path(sys.argv[2])
prefix=sys.argv[3]
profile=sys.argv[4]
if profile == "fonts-gui":
    files={
        "fonts/AlcardPixel-Regular.hex": root/"fonts/alcard-pixel/source/AlcardPixel-Regular.hex",
        "fonts/AlcardPixel-Bold.hex": root/"fonts/alcard-pixel/source/AlcardPixel-Bold.hex",
        "tools/build-alcard-fonts.py": root/"tools/build-alcard-fonts.py",
    }
else:
    files={name:root/"scripts/wayland"/name
           for name in ("wayland-session", "autostart", "environment", "foot.ini")}
with open(out, "wb") as output:
    with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode="w|", format=tarfile.USTAR_FORMAT) as tar:
            for name, path in sorted(files.items()):
                data=path.read_bytes()
                entry=tarfile.TarInfo(f"{prefix}/{name}")
                entry.size=len(data)
                entry.mode=0o755 if name in ("wayland-session", "autostart") else 0o644
                entry.uid=entry.gid=entry.mtime=0
                tar.addfile(entry, io.BytesIO(data))
PY
    (
        cd "$WORK"
        abuild checksum
        /bin/sh -n APKBUILD || fail "Invalid APKBUILD"
        REPODEST="$LOCAL/packages" abuild -d
    )
    [[ -s "$OUT/$APK_FILE" ]] ||
        fail "Expected signed package missing: $OUT/$APK_FILE"
    echo "==> Built $OUT/$APK_FILE"
}
case "${1:-}" in
    check) check ;;
    build) build ;;
    *) fail "Usage: $0 check|build fonts-gui|desktop" ;;
esac
