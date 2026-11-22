#!/usr/bin/env bash
# Disposable Alpine software acceptance: production Shell/locker, private
# simulated login1, real headless compositor, and read-only Docker sysfs mounts.
# No kernel sleep, host service, device, credential or PAM mutation is performed.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
if [[ ${1:-} == --help ]]; then
  cat <<'HELP'
Run as root inside a disposable Alpine 3.24 container, after installing Shell
and its sibling locker. Required environment:
  ALCARD_BATTERY_TEST_CONTAINER=1
  ALCARD_BATTERY_SYSFS_FIXTURE=/work/.../power-supply
  ALCARD_BACKLIGHT_SYSFS_FIXTURE=/work/.../backlight
The host driver must create EMPTY ordinary directories at those RW repo aliases
and bind them read-only over /sys/class/power_supply and /sys/class/backlight.
The script verifies mount identity and drops to a dedicated unprivileged UID.
No privileged container, CAP_SYS_ADMIN, unshare, host /run or host device is used.
Optional:
  ALCARD_SHELL_BIN=/usr/bin/alcard-shell
  ALCARD_LOCK_BIN=/usr/bin/alcard-lock
  ALCARD_POINTER_BIN=/path/to/examples/park-pointer
  ALCARD_BATTERY_EVIDENCE=/absolute/output/directory
Dependencies: bash util-linux dbus glib py3-gobject3 py3-pillow python3,
              labwc foot grim wlr-randr wtype, and the installed locker libraries.
Software receipts and unchanged 320x170 captures are saved in the output.
Hardware charge accuracy, real kernel suspend and PAM authentication are separate.
HELP
  exit 0
fi
[[ $# == 0 ]] || { echo 'Only --help or no arguments are supported' >&2; exit 2; }
[[ ${ALCARD_BATTERY_TEST_CONTAINER:-} == 1 && -f /.dockerenv && $(id -u) == 0 ]] || {
  echo 'Refusing battery fixtures outside an explicitly enabled disposable container' >&2
  exit 1
}
case "$(cat /etc/alpine-release 2>/dev/null)" in
  3.24.*) ;;
  *) echo 'Native Alpine 3.24 is required for this runtime acceptance' >&2; exit 1 ;;
esac
BIN="${ALCARD_SHELL_BIN:-/usr/bin/alcard-shell}"
LOCK="${ALCARD_LOCK_BIN:-/usr/bin/alcard-lock}"
OUT="${ALCARD_BATTERY_EVIDENCE:-$ROOT/.alcard-local/battery-runtime}"
POWER="${ALCARD_BATTERY_SYSFS_FIXTURE:?Set the RW alias of the readonly power_supply mount}"
BACKLIGHT="${ALCARD_BACKLIGHT_SYSFS_FIXTURE:?Set the RW alias of the readonly backlight mount}"
[[ "$BIN" == /* && -x "$BIN" && "$LOCK" == /* && -x "$LOCK" ]] || {
  echo 'Absolute executable paths to the production Shell and locker are required' >&2; exit 1;
}
[[ "$OUT" == /* && "$OUT" != / && ! -L "$OUT" ]] || {
  echo 'Evidence requires an ordinary absolute output directory' >&2; exit 1;
}
for tool in python3 dbus-run-session dbus-daemon labwc foot grim wlr-randr wtype setpriv timeout; do
  command -v "$tool" >/dev/null || { echo "Missing battery runtime tool: $tool" >&2; exit 1; }
done
python3 -c 'from gi.repository import Gio, GLib; from PIL import Image; import tomllib'
python3 "$ROOT/tools/shell-battery.py" mounts --power "$POWER" --backlight "$BACKLIGHT" --empty

# Account creation is confined to the disposable container. Neither Shell nor
# its real locker runs as root; no authentication is replaced with a stub.
if id alcard-battery-test >/dev/null 2>&1; then
  echo 'Fixture account already exists; use a fresh disposable container' >&2
  exit 1
fi
adduser -D -s /bin/sh alcard-battery-test
test_uid="$(id -u alcard-battery-test)"
test_gid="$(id -g alcard-battery-test)"
[[ "$test_uid" -ge 1000 ]] || { echo 'Expected an ordinary fixture UID' >&2; exit 1; }
mkdir -p -- "$OUT"
chown "$test_uid:$test_gid" "$OUT" "$POWER" "$BACKLIGHT"
TMP="$(mktemp -d /tmp/alcard-battery.XXXXXX)"
chown "$test_uid:$test_gid" "$TMP"
chmod 700 "$TMP"
cleanup() { rm -rf -- "$TMP"; }
trap cleanup EXIT

env -u DBUS_SYSTEM_BUS_ADDRESS -u DBUS_SESSION_BUS_ADDRESS \
  -u WAYLAND_DISPLAY -u WAYLAND_SOCKET -u DISPLAY -u WAYLAND_DEBUG \
  HOME=/home/alcard-battery-test USER=alcard-battery-test LOGNAME=alcard-battery-test \
  setpriv --reuid "$test_uid" --regid "$test_gid" --clear-groups -- \
  timeout -k 10 480 dbus-run-session -- \
  python3 "$ROOT/tools/shell-battery.py" run \
    --root "$ROOT" --temporary "$TMP" --out "$OUT" --shell "$BIN" --locker "$LOCK" \
    --power "$POWER" --backlight "$BACKLIGHT"
