#!/usr/bin/env bash
# Offline image inspection only: no mounting or block-device writes.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
source "$ROOT/scripts/package-metadata.sh"
LOCAL="$ROOT/.alcard-local"
case "$1" in
  v5|v5-lite) profile="$1" ;;
  *) echo 'Usage: verify-image-zero.sh v5|v5-lite' >&2; exit 2 ;;
esac
IMAGE_VERSION="$(alcard_package pkgver "alcard-board-zero-$profile")"
image="$LOCAL/images/alcard-zero-$profile-$IMAGE_VERSION.img"
[[ -s "$image" && ! -L "$image" ]] || {
  echo "ERROR: image missing: $image" >&2; exit 1;
}
if [[ "$(printenv ALCARD_IMAGE_CONTAINER_ROOT || :)" != 1 ]]; then
  box="$(printenv ALCARD_BUILD_BOX || echo alcard-arm64)"
  exec podman exec --user 0 -e ALCARD_IMAGE_CONTAINER_ROOT=1 \
    "$box" /bin/bash "$ROOT/scripts/verify-image-zero.sh" "$profile"
fi
(cd "$LOCAL/images" && sha256sum -c "alcard-zero-$profile-$IMAGE_VERSION.img.sha256")
for required in python3 e2fsck debugfs cpio mcopy mtype sha256sum; do
  command -v "$required" >/dev/null 2>&1 || {
    echo "ERROR: missing verifier tool $required (install e2fsprogs-extra and cpio in alcard-arm64)" >&2
    exit 1
  }
done
python3 "$ROOT/tools/image-zero.py" verify "$image"
python3 "$ROOT/tools/verify-image-deep-zero.py" --image "$image" --profile "$profile"
echo "==> PASS: $profile raw image SHA256, partition structure, ext4, rootfs and initramfs"
echo '==> Does not prove first hardware boot or CM0 A/B tryboot'
