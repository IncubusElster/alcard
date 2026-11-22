#!/usr/bin/env bash
# Build a signed Cardputer Zero v5 board-profile package using existing Distrobox.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
source "$ROOT/scripts/package-metadata.sh"
LOCAL="$ROOT/.alcard-local"
OUTPUT="$LOCAL/packages"
profile="${2:-}"
case "$profile" in
    v5) pkg=alcard-board-zero-v5; config=alcard.cfg ;;
    v5-lite) pkg=alcard-board-zero-v5-lite; config=alcard-lite.cfg ;;
    *) echo "Usage: $0 [check|build] [v5|v5-lite]" >&2; exit 2 ;;
esac
RECIPE="$ROOT/packaging/apk/$pkg/APKBUILD"
WORK="$LOCAL/work/aports/alcard/$pkg"
log() { printf '==> %s\n' "$*"; }
fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

check_env() {
    [[ "$(apk --print-arch)" == aarch64 ]] || fail 'Requires Alpine aarch64 Distrobox'
    [[ "$(id -u)" != 0 ]] || fail 'Use unprivileged user'
    for cmd in abuild python3 apk sed tar gzip; do command -v "$cmd" >/dev/null || fail "Missing $cmd"; done
    apk info -e alpine-sdk >/dev/null 2>&1 || fail 'Missing alpine-sdk'
    [[ -s "$RECIPE" && -s "$ROOT/board/cardputer-zero/boot/$config" ]] ||
        fail "Missing board profile input for $pkg"
    [[ -s "$OUTPUT/alcard/aarch64/$(alcard_package filename alcard-bsp-zero)" ]] ||
        fail 'Build the current alcard-bsp-zero first: ./build.sh bsp'
    log "$pkg profile prerequisites OK"
}

build() {
    check_env
    local ver rel outfile
    ver="$(sed -n 's/^pkgver=//p' "$RECIPE" | head -n1)"
    rel="$(sed -n 's/^pkgrel=//p' "$RECIPE" | head -n1)"
    outfile="$OUTPUT/alcard/aarch64/$pkg-$ver-r$rel.apk"
    rm -rf -- "$WORK"
    mkdir -p -- "$WORK" "$(dirname "$outfile")"
    cp "$RECIPE" "$WORK/APKBUILD"
    python3 - "$ROOT/board/cardputer-zero/boot/$config" "$WORK/$pkg-$ver.tar.gz" "$pkg-$ver" <<'PY'
import gzip, io, pathlib, sys, tarfile
data=pathlib.Path(sys.argv[1]).read_bytes()
with open(sys.argv[2],"wb") as output:
    with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode="w|", format=tarfile.USTAR_FORMAT) as tar:
            info=tarfile.TarInfo(f"{sys.argv[3]}/alcard.cfg")
            info.size=len(data); info.mode=0o644
            info.uid=info.gid=info.mtime=0
            tar.addfile(info,io.BytesIO(data))
PY
    (
        cd "$WORK"
        abuild checksum
        /bin/sh -n APKBUILD || fail 'Invalid APKBUILD'
        REPODEST="$OUTPUT" abuild -d
    )
    [[ -s "$outfile" ]] || fail "Output APK missing: $outfile"
    log "Built: ${outfile#"$ROOT"/}"
}
case "${1:-}" in
    check) check_env ;;
    build) build ;;
    *) fail "Usage: $0 [check|build] [v5|v5-lite]" ;;
esac
