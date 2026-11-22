#!/usr/bin/env bash
# Real headless labwc integration smoke; not a Cardputer hardware acceptance test.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
BIN="${ALCARD_SHELL_BIN:-$ROOT/rust/apps/alcard-shell/target/debug/alcard-shell}"
[[ -x "$BIN" ]] || { echo 'Build alcard-shell before running smoke' >&2; exit 1; }
[[ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]] || { echo 'Run through dbus-run-session' >&2; exit 1; }
for tool in labwc foot grim notify-send dbus-send; do command -v "$tool" >/dev/null; done
TMP="$(mktemp -d)"
export XDG_RUNTIME_DIR="$TMP/runtime"
mkdir -m 700 "$XDG_RUNTIME_DIR" "$TMP/config"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1
unset WAYLAND_DISPLAY DISPLAY
shell_pid='' compositor_pid='' terminal_pid=''
cleanup() {
  local result=$?
  if [[ $result -ne 0 ]]; then
    echo '--- labwc ---'; tail -80 "$TMP/labwc.log" 2>/dev/null || true
    echo '--- shell ---'; tail -160 "$TMP/shell.log" 2>/dev/null || true
  fi
  for pid in "$terminal_pid" "$shell_pid" "$compositor_pid"; do
    [[ -z "$pid" ]] || kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  rm -rf -- "$TMP"
  exit "$result"
}
trap cleanup EXIT
labwc -C "$TMP/config" >"$TMP/labwc.log" 2>&1 & compositor_pid=$!
for _ in $(seq 1 100); do
  for socket in "$XDG_RUNTIME_DIR"/wayland-*; do
    if [[ -S "$socket" ]]; then export WAYLAND_DISPLAY="$(basename "$socket")"; break 2; fi
  done
  kill -0 "$compositor_pid"
  sleep 0.1
done
[[ -n "${WAYLAND_DISPLAY:-}" ]]
WAYLAND_DEBUG=1 "$BIN" --locale en --theme classic >"$TMP/shell.log" 2>&1 & shell_pid=$!
for _ in $(seq 1 100); do
  kill -0 "$shell_pid"
  if dbus-send --session --print-reply --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.NameHasOwner string:org.freedesktop.Notifications | grep -q 'boolean true'; then break; fi
  sleep 0.1
done
sleep 0.4
grim "$TMP/home.png"
/usr/bin/python3 - "$TMP/home.png" <<'PY'
from PIL import Image
import sys
colors = set(Image.open(sys.argv[1]).convert('RGB').getdata())
assert (0x10, 0x1d, 0x30) in colors, 'Shell background not mapped or incorrect channel order'
assert (0x75, 0xdc, 0xeb) in colors, 'Shell accent not rendered'
PY
if timeout 5 "$BIN" --locale en >"$TMP/duplicate.log" 2>&1; then echo 'Second Shell unexpectedly accepted' >&2; exit 1; fi
kill -0 "$shell_pid"
foot --app-id=alcard-ci -- sh -c 'sleep 30' >"$TMP/foot.log" 2>&1 & terminal_pid=$!
sleep 0.3
for _ in 1 2 3; do timeout 5 "$BIN" --home; done
for n in $(seq 1 12); do
  notify-send --expire-time=700 "Alcard smoke $n" 'SCTK notification lifecycle'
  sleep 0.12
done
sleep 1.0
notify-send --expire-time=700 'Alcard remap' 'Fresh layer after previous expiration'
sleep 0.3
grim "$TMP/notice.png"
sleep 1
kill -0 "$shell_pid"
grep -q 'zwlr_layer_shell_v1' "$TMP/shell.log"
grep -Eq 'set_keyboard_interactivity\(0\)' "$TMP/shell.log"
grep -Eq 'zwlr_foreign_toplevel_handle_v1.*activate\(' "$TMP/shell.log"
if grep -Ei 'protocol error|panicked at|Wayland keyboard initialization|SHM buffer length mismatch' "$TMP/shell.log"; then exit 1; fi
echo 'PASS: headless labwc mapping/colors, one instance, foreign Home activation, repeated notification layers and expiry'
