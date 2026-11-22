#!/usr/bin/env bash
# Alcard build entrypoint. Paths are always resolved from the repository root.
# Use the pre-provisioned Alpine aarch64 Distrobox. Never create it here.
set -euo pipefail

PROJECT_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
BOX_NAME="${ALCARD_BUILD_BOX:-alcard-arm64}"

usage() {
  cat <<'HELP'
Usage: ./build.sh <command>

Commands:
  audit             Static check of scripts, APKBUILD and full-image package graph
  doctor            Check ARM64 Distrobox/kernel-module build environment
  kmods             Build alcard-kmods-rpi in persistent Distrobox
  firmware-check    Check ARM64 Distrobox/firmware build environment
  firmware          Build alcard-firmware-zero in persistent Distrobox
  boot-check        Check ARM64 Distrobox/Device Tree build environment
  boot              Build alcard-boot-zero board overlays APK
  board-v5-check    Check standard v5 profile package prerequisites
  board-v5          Build alcard-board-zero-v5 package
  board-lite-check  Check v5 Lite profile package prerequisites
  board-v5-lite     Build alcard-board-zero-v5-lite package
  keyboard-check    Check alcard-keyboard-zero US/RU/ES/DE APK prerequisites
  keyboard          Build signed alcard-keyboard-zero APK
  hello-check       Check aarch64 Hello APK build prerequisites
  hello             Build signed aarch64 alcard-hello APK
  fonts-check       Check Alcard Pixel PSF2 APK prerequisites
  fonts             Build signed noarch alcard-fonts-console APK
  fonts-gui-check   Check Alcard Pixel TTF APK dependencies
  fonts-gui         Build signed Alcard Pixel TTF fonts package
  shell-check       Test SCTK/tiny-skia Shell in existing ARM64 container
  polkit-check      Test graphical polkit agent in the existing ARM64 Distrobox
  lock-check        Test session locker and PAM client in the existing ARM64 Distrobox
  shell-apk-check   Check native Shell, agent, locker and session APK prerequisites
  shell-apks        Build signed Shell, agent, locker and desktop-shell APKs
  shell-install-verify Install signed session APKs in a disposable Alpine rootfs
  desktop-check     Check optional labwc / foot profile dependencies
  desktop           Build signed alcard-desktop-minimal Wayland package
  bsp-check         Check dependency-only BSP metapackage build prerequisites
  bsp               Build alcard-bsp-zero signed meta APK
  verify-bsp        Verify standard v5 profile, DT and kernel DRM config
  verify-bsp-lite   Verify v5 Lite profile, DT and kernel DRM config
  install-check     Check isolated Alpine rootfs install prerequisites
  install-verify-lite Install/verify v5 Lite packages in isolated rootfs
  install-verify    Install/verify packages into disposable Alpine aarch64 rootfs
  qemu-hello-check  Check Alpine aarch64 QEMU Hello image prerequisites
  qemu-hello        Build Alpine aarch64 QEMU Hello image
  image-check-v5    Check full v5 image prerequisites
  image-check-lite  Check v5 Lite image prerequisites
  image-v5          Build complete v5 raw microSD image
  image-v5-lite     Build complete v5 Lite raw microSD image
  image-verify-v5   Verify full v5 image offline
  image-verify-lite Verify v5 Lite image offline
  help              Show this help

Prepare the container and install needed Alpine packages once beforehand.
Set ALCARD_BUILD_BOX to override the default container name (alcard-arm64).
Set ALCARD_DEV_ROOT=1 for an image with the local developer root password hash.
Sources, intermediate files and APK outputs remain in .alcard-local/.
HELP
}

[[ $# -eq 1 ]] || { usage >&2; exit 2; }
case "$1" in
  help|-h|--help) usage; exit 0 ;;
  audit)          exec bash "$PROJECT_ROOT/scripts/audit-build.sh" ;;
  doctor)         worker='build-kmods-rpi.sh'; stage='check' ;;
  kmods)          worker='build-kmods-rpi.sh'; stage='build' ;;
  firmware-check) worker='build-firmware-zero.sh'; stage='check' ;;
  firmware)       worker='build-firmware-zero.sh'; stage='build' ;;
  boot-check)     worker='build-boot-zero.sh'; stage='check' ;;
  boot)           worker='build-boot-zero.sh'; stage='build' ;;
  board-v5-check) worker='build-board-zero.sh'; stage='check'; profile='v5' ;;
  board-v5)       worker='build-board-zero.sh'; stage='build'; profile='v5' ;;
  board-lite-check) worker='build-board-zero.sh'; stage='check'; profile='v5-lite' ;;
  board-v5-lite)  worker='build-board-zero.sh'; stage='build'; profile='v5-lite' ;;
  keyboard-check) worker='build-keyboard-zero.sh'; stage='check' ;;
  keyboard)       worker='build-keyboard-zero.sh'; stage='build' ;;
  hello-check)    worker='build-hello.sh'; stage='check' ;;
  hello)          worker='build-hello.sh'; stage='build' ;;
  fonts-check)    worker='build-fonts-console.sh'; stage='check' ;;
  fonts)          worker='build-fonts-console.sh'; stage='build' ;;
  fonts-gui-check) worker='build-wayland-apk.sh'; stage='check'; profile='fonts-gui' ;;
  fonts-gui)       worker='build-wayland-apk.sh'; stage='build'; profile='fonts-gui' ;;
  shell-check)     worker='check-shell.sh'; stage='test' ;;
  polkit-check)    worker='check-polkit.sh'; stage='check' ;;
  lock-check)      worker='check-lock.sh'; stage='check' ;;
  shell-apk-check)  worker='build-shell-apk.sh'; stage='check'; profile='all' ;;
  shell-apks)       worker='build-shell-apk.sh'; stage='build'; profile='all' ;;
  shell-install-verify) worker='verify-session-install.sh'; stage='install' ;;
  desktop-check)   worker='build-wayland-apk.sh'; stage='check'; profile='desktop' ;;
  desktop)         worker='build-wayland-apk.sh'; stage='build'; profile='desktop' ;;
  qemu-hello-check) worker='build-qemu-hello-aarch64.sh'; stage='check' ;;
  qemu-hello)       worker='build-qemu-hello-aarch64.sh'; stage='build' ;;
  bsp-check)      worker='build-bsp-zero.sh'; stage='check' ;;
  bsp)            worker='build-bsp-zero.sh'; stage='build' ;;
  verify-bsp)     worker='verify-bsp-zero.sh'; stage='all'; profile='v5' ;;
  verify-bsp-lite) worker='verify-bsp-zero.sh'; stage='all'; profile='v5-lite' ;;
  install-check)  worker='verify-install-zero.sh'; stage='check' ;;
  install-verify) worker='verify-install-zero.sh'; stage='install'; profile='v5' ;;
  install-verify-lite) worker='verify-install-zero.sh'; stage='install'; profile='v5-lite' ;;
  image-check-v5) worker='build-image-zero.sh'; stage='check'; profile='v5' ;;
  image-check-lite) worker='build-image-zero.sh'; stage='check'; profile='v5-lite' ;;
  image-v5) worker='build-image-zero.sh'; stage='build'; profile='v5' ;;
  image-v5-lite) worker='build-image-zero.sh'; stage='build'; profile='v5-lite' ;;
  image-verify-v5) worker='verify-image-zero.sh'; stage='v5' ;;
  image-verify-lite) worker='verify-image-zero.sh'; stage='v5-lite' ;;
  *) usage >&2; exit 2 ;;
esac

command -v distrobox >/dev/null 2>&1 || {
  echo 'Missing distrobox on the host' >&2
  exit 1
}

if [[ "$worker" == build-image-zero.sh || "$worker" == verify-image-zero.sh || "$worker" == build-qemu-hello-aarch64.sh || "$worker" == verify-session-install.sh ]]; then
  exec bash "$PROJECT_ROOT/scripts/$worker" "$stage" "${profile:-}"
fi
exec distrobox enter --no-tty --name "$BOX_NAME" -- \
  bash "$PROJECT_ROOT/scripts/$worker" "$stage" "${profile:-}"
