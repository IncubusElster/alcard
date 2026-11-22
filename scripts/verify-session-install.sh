#!/usr/bin/env bash
# Install and inspect signed runtime APKs in a fresh disposable Alpine rootfs.
# Root is used only inside the existing rootless container, never the host.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
source "$ROOT/scripts/package-metadata.sh"
LOCAL="$ROOT/.alcard-local"
WORK="$LOCAL/session-install"
REPO="$LOCAL/packages/alcard"
fail() { echo "ERROR: $*" >&2; exit 1; }
[[ "${1:-install}" == install ]] || fail 'Usage: verify-session-install.sh install'
if [[ "${ALCARD_SESSION_CONTAINER_ROOT:-}" != 1 ]]; then
    command -v podman >/dev/null || fail 'Podman is required on the host'
    exec podman exec --user 0 -e ALCARD_SESSION_CONTAINER_ROOT=1 \
        "${ALCARD_BUILD_BOX:-alcard-arm64}" /bin/bash "$ROOT/scripts/verify-session-install.sh" install
fi
[[ "$(id -u)" == 0 && "$(apk --print-arch)" == aarch64 ]] ||
    fail 'Requires root inside Alpine aarch64 rootless container'
alcard_package_array USER_APKS filename --group user
alcard_package_array SESSION_PINS pin --group session-roots
for filename in "${USER_APKS[@]}"; do
    apk verify "$REPO/aarch64/$filename" || fail "Missing/invalid signed APK: $filename"
done
[[ ! -L "$WORK" && ! -L "$LOCAL" ]] || fail 'Refusing symlinked installation directory'
rm -rf -- "$WORK"
mkdir -p "$WORK/rootfs/etc/apk/keys"
cp -L /etc/apk/keys/*.pub "$WORK/rootfs/etc/apk/keys/"
cat > "$WORK/repositories" <<EOF
https://dl-cdn.alpinelinux.org/alpine/v3.24/main
https://dl-cdn.alpinelinux.org/alpine/v3.24/community
$REPO
EOF
# Finish the base transaction before desktop pre-install scripts create service
# accounts. Their adduser/addgroup calls require baselayout's account databases
# and BusyBox's applet links; APK does not order unrelated packages for us.
apk --root "$WORK/rootfs" --arch aarch64 --keys-dir /etc/apk/keys \
    --repositories-file "$WORK/repositories" add --initdb alpine-base
apk --root "$WORK/rootfs" --arch aarch64 --keys-dir /etc/apk/keys \
    --repositories-file "$WORK/repositories" add \
    "${SESSION_PINS[@]}"
python3 "$ROOT/tools/ensure-greetd-user.py" "$WORK/rootfs"
python3 "$ROOT/tools/verify-session-install.py" --root "$WORK/rootfs" --mode graphics
echo '==> PASS signed runtime installation; login/hardware acceptance remains separate'
