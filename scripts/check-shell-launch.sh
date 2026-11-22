#!/usr/bin/env bash
# Real launch/window and focus-local input checks in private Wayland.
# Invoke with: dbus-run-session -- bash scripts/check-shell-launch.sh
# The fixtures are ordinary .desktop applications, not production test modes.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
BIN="${ALCARD_SHELL_BIN:-$ROOT/rust/apps/alcard-shell/target/debug/alcard-shell}"
OUT="${ALCARD_LAUNCH_SCREENSHOT_DIR:-$ROOT/.alcard-local/launch-check}"
[[ -x "$BIN" ]] || { echo 'Build alcard-shell before launch checks' >&2; exit 1; }
[[ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]] ||
  { echo 'Run check-shell-launch.sh through a fresh dbus-run-session' >&2; exit 1; }
for tool in labwc foot grim wtype wlr-randr gdbus python3 timeout; do
  command -v "$tool" >/dev/null || { echo "Missing launch-check tool: $tool" >&2; exit 1; }
done
python3 -c 'from PIL import Image; import tomllib'

bus_call() {
  timeout 5 gdbus call --session --dest org.freedesktop.DBus \
    --object-path /org/freedesktop/DBus \
    --method "org.freedesktop.DBus.$1" "${@:2}"
}
if [[ "$(bus_call NameHasOwner org.freedesktop.Notifications)" == *true* ]]; then
  echo 'Refusing a bus with an existing notification server; use dbus-run-session' >&2
  exit 1
fi

mkdir -p -- "$OUT"
OUT="$(cd -- "$OUT" && pwd -P)"
TMP="$(mktemp -d)"
shell_pid='' compositor_pid=''
last_wait='startup' last_capture='none' last_key='none'
cleanup() {
  local result=$? pid
  trap - EXIT INT TERM
  # Preserve observed fixture output even if an assertion failed.
  if [[ -d "$TMP/fixtures" ]]; then
    mkdir -p "$OUT/fixtures"
    for evidence in "$TMP/fixtures/"*-starts "$TMP/fixtures/"*-ready \
      "$TMP/fixtures/"*-input "$TMP/fixtures/"*-exit "$TMP/fixtures/"*.log; do
      [[ ! -f "$evidence" ]] || cp -- "$evidence" "$OUT/fixtures/"
    done
  fi
  if [[ $result -ne 0 ]]; then
    # A failed text wait has not reached capture(). Keep exactly the last
    # polled image and its expected condition for the CI artifact.
    if [[ -s "$TMP/observed.png" ]]; then
      cp -- "$TMP/observed.png" "$OUT/failure-last-observed.png" || true
    fi
    {
      printf 'Exit status: %s\nLast awaited condition: %s\n' "$result" "$last_wait"
      printf 'Last completed screenshot: %s\nLast physical key: %s\n' "$last_capture" "$last_key"
      printf 'failure-last-observed.png is the last text-poll image, if available.\n'
    } >"$OUT/failure.txt" || true
    echo 'Shell launch check failed; compositor and Shell logs follow.' >&2
    tail -80 "$OUT/labwc.log" >&2 2>/dev/null || true
    tail -120 "$OUT/shell.log" >&2 2>/dev/null || true
  fi
  # Only terminate PIDs still running one of this session's fixture paths.
  # A short-lived wrapper may already have exited and its PID been reused.
  if [[ -f "$TMP/fixtures/pids" ]]; then
    while read -r pid; do
      [[ "$pid" =~ ^[0-9]+$ && -r "/proc/$pid/cmdline" ]] || continue
      if tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null | grep -Fq -- "$TMP/"; then
        kill "$pid" 2>/dev/null || true
      fi
    done <"$TMP/fixtures/pids"
  fi
  for pid in "$shell_pid" "$compositor_pid"; do
    [[ -z "$pid" ]] || kill "$pid" 2>/dev/null || true
  done
  # Bound cleanup even if a compositor or child fails to process SIGTERM.
  for _ in $(seq 1 20); do
    if ! kill -0 ${shell_pid:+"$shell_pid"} ${compositor_pid:+"$compositor_pid"} 2>/dev/null; then
      break
    fi
    sleep 0.1
  done
  for pid in "$shell_pid" "$compositor_pid"; do
    [[ -z "$pid" ]] || kill -KILL "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  rm -rf -- "$TMP"
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

export XDG_RUNTIME_DIR="$TMP/runtime"
export XDG_CONFIG_HOME="$TMP/config-root"
export XDG_CONFIG_DIRS="$TMP/config-dirs"
export XDG_DATA_HOME="$TMP/data"
export XDG_DATA_DIRS="$TMP/data-dirs"
export S29_DIR="$TMP/fixtures"
export ALCARD_FOOT_CONFIG="$TMP/foot.ini"
export S29_ROOT="$ROOT" S29_SHELL_LOG="$OUT/shell.log"
mkdir -m 700 "$XDG_RUNTIME_DIR"
mkdir -p "$TMP/labwc" "$XDG_CONFIG_HOME" "$XDG_CONFIG_DIRS" \
  "$XDG_DATA_HOME/applications" "$XDG_DATA_DIRS" "$S29_DIR"
# Terminal=true resolves Foot through the normal production adapter. Keep
# that path, argv and process lifetime intact, but isolate child diagnostics:
# inherited WAYLAND_DEBUG=1 and stderr can otherwise split a Rust protocol
# line in shell.log when Foot/libwayland writes concurrently.
export S29_REAL_FOOT="$(command -v foot)"
[[ "$S29_REAL_FOOT" == /* && -x "$S29_REAL_FOOT" ]] ||
  { echo 'Expected an absolute executable Foot path' >&2; exit 1; }
mkdir -p "$TMP/bin"
cat >"$TMP/bin/foot" <<'SH'
#!/bin/sh
unset WAYLAND_DEBUG
exec "$S29_REAL_FOOT" "$@" >>"$S29_DIR/foot.log" 2>&1
SH
chmod 700 "$TMP/bin/foot"
export PATH="$TMP/bin:$PATH"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=1
export WLR_LIBINPUT_NO_DEVICES=1 WLR_XWAYLAND=
unset WAYLAND_DISPLAY DISPLAY

cat >"$ALCARD_FOOT_CONFIG" <<'INI'
[main]
font=monospace:size=8
pad=0x0
[colors]
background=101820
foreground=efefef
INI
cat >"$S29_DIR/tui.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
kind=$1
printf '%s\n' "$BASHPID" >>"$S29_DIR/pids"
printf 'start\n' >>"$S29_DIR/$kind-tui-starts"
printf 'ready\n' >"$S29_DIR/$kind-ready"
trap 'exit 0' TERM HUP INT
printf 'Alcard launch fixture: %s\nKeys belong to this application.\n' "$kind"
while IFS= read -r line; do
  printf '%s\n' "$line" >>"$S29_DIR/$kind-input"
  printf 'Received: %s\n' "$line"
done
SH
cat >"$S29_DIR/slow.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$BASHPID" >>"$S29_DIR/pids"
printf 'start\n' >>"$S29_DIR/slow-starts"
for _ in $(seq 1 120); do
  if [[ -f "$S29_DIR/slow-gate" ]]; then
    exec foot --app-id s29-slow --title 'S29 slow window' \
      --config "$ALCARD_FOOT_CONFIG" -- "$S29_DIR/tui.sh" slow \
      >"$S29_DIR/slow.log" 2>&1
  fi
  sleep 0.1
done
echo 'Slow fixture gate timed out' >&2
exit 24
SH
cat >"$S29_DIR/handoff.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$BASHPID" >>"$S29_DIR/pids"
printf 'start\n' >>"$S29_DIR/handoff-starts"
(
  printf '%s\n' "$BASHPID" >>"$S29_DIR/pids"
  for _ in $(seq 1 120); do
    if [[ -f "$S29_DIR/handoff-gate" ]]; then
      exec foot --app-id s29-handoff --title 'S29 handoff window' \
        --config "$ALCARD_FOOT_CONFIG" -- "$S29_DIR/tui.sh" handoff
    fi
    sleep 0.1
  done
  echo 'Handoff fixture gate timed out' >&2
  exit 25
) </dev/null >"$S29_DIR/handoff.log" 2>&1 &
printf '0\n' >"$S29_DIR/handoff-parent-exit"
exit 0
SH
cat >"$S29_DIR/fail.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf 'start\n' >>"$S29_DIR/fail-starts"
exit 23
SH
chmod 700 "$S29_DIR/"*.sh

write_desktop() {
  local id=$1 name=$2 terminal=$3 executable=$4 argument=${5:-}
  cat >"$XDG_DATA_HOME/applications/$id.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=$name
Exec="$S29_DIR/$executable" $argument
Terminal=$terminal
Icon=application-x-executable
DESKTOP
}
write_desktop s29-slow '01 Delayed GUI' false slow.sh
# This case must acquire its app_id from the production Terminal=true path.
write_desktop s29-tui '02 Terminal' true tui.sh tui
write_desktop s29-handoff '03 Late handoff' false handoff.sh
write_desktop s29-fail '04 Failure' false fail.sh

# Ubuntu 24.04 labwc 0.7.1 supports ToggleFullscreen onFirstMap.
# No skipTaskbar: the Shell must see the ordinary foreign-toplevel handles.
cat >"$TMP/labwc/rc.xml" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<labwc_config>
  <keyboard>
    <!-- Test-only compositor focus switch; does not reset Shell state. -->
    <keybind key="A-Tab"><action name="NextWindow" /></keybind>
  </keyboard>
  <windowRules>
    <windowRule identifier="alcard-shell" serverDecoration="no" event="onFirstMap">
      <action name="ToggleFullscreen" />
    </windowRule>
    <windowRule identifier="s29-slow" serverDecoration="no" event="onFirstMap">
      <action name="ToggleFullscreen" />
    </windowRule>
    <windowRule identifier="s29-tui" serverDecoration="no" event="onFirstMap">
      <action name="ToggleFullscreen" />
    </windowRule>
    <windowRule identifier="s29-handoff" serverDecoration="no" event="onFirstMap">
      <action name="ToggleFullscreen" />
    </windowRule>
  </windowRules>
</labwc_config>
XML
printf '#!/bin/sh\nexit 0\n' >"$TMP/labwc/autostart"
printf '#!/bin/sh\nexit 0\n' >"$TMP/labwc/shutdown"
chmod 700 "$TMP/labwc/autostart" "$TMP/labwc/shutdown"

# Read actual Wayland protocol observations and actual screenshots. The
# bitmap font makes text assertions deterministic without OCR, rendering
# another Shell, or introducing an introspection/test API into production.
cat >"$TMP/observe.py" <<'PY'
import os
import pathlib
import re
import sys
import tomllib
from PIL import Image

def text_positions(path, text):
    root = pathlib.Path(os.environ["S29_ROOT"])
    palette = tomllib.loads(
        (root / "rust/crates/alcard-settings-core/themes/classic.toml").read_text()
    )["palette"]
    colors = {
        tuple(bytes.fromhex(palette[key][1:]))
        for key in ("text", "muted", "accent", "disabled")
    }
    glyphs = {}
    for line in (root / "fonts/alcard-pixel/source/AlcardPixel-Regular.hex").read_text().splitlines():
        if ":" in line:
            code, bits = line.split(":", 1)
            if len(bits) == 16:
                glyphs[chr(int(code, 16))] = bytes.fromhex(bits)
    image = Image.open(path).convert("RGB")
    assert image.size == (320, 170), image.size
    pixels = bytes(pixel in colors for pixel in image.getdata())
    rows = [pixels[y * 320:(y + 1) * 320] for y in range(170)]
    template = [
        bytes(bool(glyphs[character][y] & (0x80 >> x)) for character in text for x in range(8))
        for y in range(8)
    ]
    anchor = max(range(8), key=lambda row: sum(template[row]))
    assert sum(template[anchor]), "Text assertion must have a visible glyph"
    result = []
    for y in range(163):
        start = 0
        while (x := rows[y + anchor].find(template[anchor], start)) != -1:
            if all(rows[y + line][x:x + len(template[line])] == template[line] for line in range(8)):
                result.append((x, y))
            start = x + 1
    return result

def windows():
    live, history = {}, []
    pattern = re.compile(
        r"(zwlr_foreign_toplevel_handle_v1[@#]\d+)\."
        r"(app_id|state|done|activate|close|closed)\s*,?\s*\((.*)\)"
    )
    # wayland-rs formats incoming events as ".app_id, (Some(...))";
    # outgoing requests use ".activate(...)". Accept both actual formats.
    for line in pathlib.Path(os.environ["S29_SHELL_LOG"]).read_text(errors="replace").splitlines():
        match = pattern.search(line)
        if not match:
            continue
        handle, event, args = match.groups()
        record = live.get(handle)
        if record is None or record["closed"]:
            record = dict(handle=handle, app_id=None, ready=False, active=False,
                          closed=False, activate=0, close=0)
            live[handle] = record
            history.append(record)
        if event == "app_id":
            value = re.search(r'"([^"]*)"', args)
            if value:
                record["app_id"] = value[1]
        elif event == "state":
            values = [int(value) for value in re.findall(r"\d+", args)]
            record["active"] = any(
                values[index:index + 4] == [2, 0, 0, 0]
                for index in range(0, len(values), 4)
            )
        elif event == "done":
            record["ready"] = True
        elif event == "closed":
            record["closed"] = True
            record["active"] = False
        else:
            record[event] += 1
    return history

if sys.argv[1] == "text":
    raise SystemExit(0 if all(text_positions(sys.argv[2], text) for text in sys.argv[3:]) else 1)
if sys.argv[1] == "text-count":
    raise SystemExit(0 if len(text_positions(sys.argv[2], sys.argv[3])) == int(sys.argv[4]) else 1)
if sys.argv[1] == "same-menu":
    before, after = (Image.open(path).convert("RGB") for path in sys.argv[2:4])
    bounds = (0, 18, 320, 170)
    assert before.crop(bounds).tobytes() == after.crop(bounds).tobytes(), "External input changed Shell Menu/filter/selection"
if sys.argv[1] == "same-home":
    before, after = (Image.open(path).convert("RGB") for path in sys.argv[2:4])
    # Time/date/hardware can update normally while another app is focused.
    bounds = (0, 154, 320, 170)
    assert before.crop(bounds).tobytes() == after.crop(bounds).tobytes(), "External input changed Home actions"
    assert text_positions(sys.argv[3], "ALCARD"), "Shell did not remain on Home"
if sys.argv[1] == "windows":
    app, operation = sys.argv[2:4]
    entries = [record for record in windows() if record["app_id"] == app]
    if operation in ("activate", "close"):
        value = sum(record[operation] for record in entries)
    elif operation == "closed":
        value = sum(record["closed"] for record in entries)
    else:
        value = sum(
            record["ready"] and not record["closed"] and
            (operation != "active" or record["active"]) for record in entries
        )
    if len(sys.argv) == 5:
        raise SystemExit(0 if value == int(sys.argv[4]) else 1)
    print(value)
PY

labwc -C "$TMP/labwc" >"$OUT/labwc.log" 2>&1 & compositor_pid=$!
for _ in $(seq 1 100); do
  for socket in "$XDG_RUNTIME_DIR"/wayland-*; do
    if [[ -S "$socket" ]]; then
      export WAYLAND_DISPLAY="$(basename -- "$socket")"
      break 2
    fi
  done
  kill -0 "$compositor_pid"
  sleep 0.1
done
[[ -n "${WAYLAND_DISPLAY:-}" ]] || { echo 'No headless Wayland socket' >&2; exit 1; }
output="$(timeout 5 wlr-randr | python3 -c 'import sys; print(next(line.split()[0] for line in sys.stdin if line and not line[0].isspace()))')"
[[ "$output" == HEADLESS-* ]] || { echo "Unexpected output: $output" >&2; exit 1; }
timeout 5 wlr-randr --output "$output" --custom-mode 320x170@60Hz --scale 1 --pos 0,0
timeout 5 wlr-randr >"$OUT/output.txt"
WAYLAND_DEBUG=1 "$BIN" --locale en --theme classic >"$OUT/shell.log" 2>&1 & shell_pid=$!

wait_for() {
  local description=$1 deadline=$((SECONDS + 15))
  last_wait=$description
  shift
  while ((SECONDS < deadline)); do
    kill -0 "$shell_pid"
    kill -0 "$compositor_pid"
    if "$@"; then return 0; fi
    sleep 0.1
  done
  echo "Timed out: $description" >&2
  return 1
}
ready() { [[ "$(bus_call NameHasOwner org.freedesktop.Notifications)" == *true* ]]; }
observe() { python3 "$TMP/observe.py" "$@"; }
window_ready() {
  wait_for "mapped $1" observe windows "$1" live 1
  # Let the acknowledged toplevel update reach the model before another key.
  sleep 0.2
}
lines_equal() {
  [[ -f "$1" ]] && [[ "$(wc -l <"$1")" -eq "$2" ]]
}
wait_for 'Shell notification service' ready
window_ready alcard-shell

cat >"$OUT/manifest.txt" <<'TEXT'
Alcard Shell: real launch/window operations and focus-local text/navigation.
Production binary, private D-Bus + labwc, headless Pixman, 320x170, English Classic.
Only four temporary XDG .desktop fixtures are present in the menu.
Terminal=true uses the production Foot launch path and a temporary Foot profile.
Foot child diagnostics are isolated in fixtures/foot.log; Shell protocol logging remains enabled.
Gate files delay ordinary fixture windows; no Shell state is injected.
Protocol assertions bind app IDs to the actual foreign-toplevel handles.
The private labwc Alt-Tab binding changes focus without using Shell Home/reset.
Actual Foot input receipts verify digits 4-8, Space, Tab, a letter and F3.
PNG text assertions read the bundled pixel font from the real screenshots.
Hardware status belongs to the CI host; this is not Cardputer hardware acceptance.
TEXT
python3 - "$BIN" "$OUT/manifest.txt" <<'PY'
import datetime, hashlib, pathlib, sys
binary, manifest = map(pathlib.Path, sys.argv[1:])
with manifest.open("a") as output:
    output.write("UTC: " + datetime.datetime.now(datetime.timezone.utc).isoformat() + "\n")
    output.write("Binary SHA256: " + hashlib.sha256(binary.read_bytes()).hexdigest() + "\n\n")
PY

capture() {
  local name=$1
  kill -0 "$shell_pid"
  sleep 0.15
  timeout 5 grim -o "$output" "$OUT/$name.png"
  python3 - "$OUT/$name.png" "$OUT/manifest.txt" <<'PY'
import hashlib, pathlib, sys
from PIL import Image
path, manifest = map(pathlib.Path, sys.argv[1:])
assert Image.open(path).size == (320, 170), path
with manifest.open("a") as output:
    output.write(f"{path.name}: 320x170; SHA256 {hashlib.sha256(path.read_bytes()).hexdigest()}\n")
PY
  last_capture="$name.png"
}
screen_has() {
  timeout 5 grim -o "$output" "$TMP/observed.png" || return 1
  observe text "$TMP/observed.png" "$@"
}
wait_text() { wait_for "visible text: $*" screen_has "$@"; }
record() { printf 'Verified: %s\n' "$*" >>"$OUT/manifest.txt"; }

physical_key() {
  local raw=$1 symbol=$2 placeholder slot
  last_key="evdev $raw ($symbol)"
  local -a args=(-s 100)
  # Explicit wtype keys allocate evdev codes from 1. Release-only private-use
  # symbols reserve earlier codes without pressing any other physical key.
  for ((slot=1; slot<raw; slot++)); do
    printf -v placeholder 'U%04X' "$((0xE000 + slot))"
    args+=(-p "$placeholder")
  done
  args+=(-s 100 -P "$symbol" -s 30 -p "$symbol" -s 100)
  WAYLAND_DEBUG=1 timeout 5 wtype "${args[@]}" >"$TMP/wtype.out" 2>"$TMP/wtype.log"
  python3 - "$TMP/wtype.log" "$raw" <<'PY'
import pathlib, re, sys
log = pathlib.Path(sys.argv[1]).read_text()
pattern = r"zwp_virtual_keyboard_v1[^\n]*\.key\([^,]*,\s*" + sys.argv[2] + r",\s*1\)"
assert re.search(pattern, log), "Unexpected virtual keyboard evdev code"
PY
  cat "$TMP/wtype.log" >>"$OUT/keyboard.log"
  sleep 0.15
}
home() {
  timeout 5 "$BIN" --home
  wait_for 'Shell focus' observe windows alcard-shell active 1
  sleep 0.2
}
menu() {
  local index=$1
  home
  physical_key 7 6
  # Menu remembers selection. Four isolated fixtures fit in the first row;
  # Up therefore selects index zero regardless of the preceding selection.
  physical_key 103 Up
  for ((i=0; i<index; i++)); do physical_key 106 Right; done
}
next_window() {
  local target=$1
  WAYLAND_DEBUG=1 timeout 5 wtype -M alt -P Tab -p Tab -m alt \
    >"$TMP/wtype.out" 2>"$TMP/wtype.log"
  cat "$TMP/wtype.log" >>"$OUT/keyboard.log"
  wait_for "Alt-Tab focus to $target" observe windows "$target" active 1
}
search_text() {
  wait_text 'Search apps'
  WAYLAND_DEBUG=1 timeout 5 wtype -d 10 -- "$1" >"$TMP/wtype.out" 2>"$TMP/wtype.log"
  cat "$TMP/wtype.log" >>"$OUT/keyboard.log"
}
clear_search() {
  wait_text 'Search apps'
  WAYLAND_DEBUG=1 timeout 5 wtype -M ctrl -P u -p u -m ctrl >"$TMP/wtype.out" 2>"$TMP/wtype.log"
  cat "$TMP/wtype.log" >>"$OUT/keyboard.log"
}
external_input() {
  local before_lines shell_activations
  before_lines="$(wc -l <"$S29_DIR/tui-input")"
  shell_activations="$(observe windows alcard-shell activate)"
  observe windows s29-tui active 1
  physical_key 5 4
  physical_key 57 space
  physical_key 6 5
  physical_key 57 space
  physical_key 7 6
  physical_key 57 space
  physical_key 8 7
  physical_key 57 space
  physical_key 9 8
  physical_key 15 Tab
  physical_key 21 y
  physical_key 28 Return
  physical_key 61 F3
  physical_key 28 Return
  wait_for 'focused Foot received text and F3' lines_equal "$S29_DIR/tui-input" "$((before_lines + 2))"
  python3 - "$S29_DIR/tui-input" <<'PY'
import pathlib, sys
lines = pathlib.Path(sys.argv[1]).read_bytes().splitlines()
assert lines[-2] == b'4 5 6 7 8\ty', f'Focused application lost ordinary input: {lines[-2:]!r}'
assert lines[-1] in (b'\x1bOR', b'\x1b[13~'), f'Focused application did not receive F3: {lines[-1]!r}'
PY
  observe windows s29-tui active 1
  observe windows alcard-shell activate "$shell_activations"
}

capture 01-home
menu 0
wait_text '01 Delayed GUI'
physical_key 5 4
wait_for 'first delayed launch' lines_equal "$S29_DIR/slow-starts" 1
wait_text Starting
capture 02-starting
menu 0
physical_key 28 Return
lines_equal "$S29_DIR/slow-starts" 1
observe windows s29-slow live 0
wait_text Starting
record 'Repeated primary launch while Starting created one process.'
touch "$S29_DIR/slow-gate"
window_ready s29-slow
wait_for 'delayed fixture ready' test -s "$S29_DIR/slow-ready"
observe windows s29-slow activate 0
menu 0
physical_key 28 Return
wait_for 'existing delayed window activation request' observe windows s29-slow activate 1
wait_for 'existing delayed window activation acknowledgement' observe windows s29-slow active 1
lines_equal "$S29_DIR/slow-starts" 1
capture 03-existing-window
record 'Primary Open activated the mapped handle without another process.'

menu 0
physical_key 6 5
wait_text 'Opts:' Windows Details
capture 04-menu-options
physical_key 28 Return
wait_text 'Opts:' Windows
observe windows s29-slow activate 1
physical_key 57 space
wait_text Windows 'S29 slow window'
capture 05-window-picker
physical_key 5 4
wait_text Details 'app_id: s29-slow'
capture 06-window-details
physical_key 9 8
wait_text Windows 'S29 slow window'
physical_key 57 space
wait_for 'picker activation of the same window' observe windows s29-slow activate 2
wait_for 'picker activation acknowledgement' observe windows s29-slow active 1
menu 0
physical_key 6 5
physical_key 57 space
wait_text Windows 'S29 slow window'
physical_key 1 Escape
wait_text 'Opts:' Windows
physical_key 9 8
wait_text '01 Delayed GUI'
record 'Context Enter did nothing; Space selected Windows; Read/Back preserved the picker target; Space activated it.'

menu 1
wait_text '02 Terminal'
# Leave a committed filter behind the external application, so a background
# F3/number key accidentally handled by Shell cannot hide behind Home reset.
physical_key 61 F3
search_text Terminal
physical_key 28 Return
wait_text 'Search: Terminal (1)'
physical_key 5 4
window_ready s29-tui
wait_for 'Terminal=true fixture ready' test -s "$S29_DIR/tui-ready"
lines_equal "$S29_DIR/tui-tui-starts" 1
capture 07-terminal-app
physical_key 5 4
physical_key 28 Return
wait_for 'physical 4 delivered to the focused terminal' grep -Fxq 4 "$S29_DIR/tui-input"
record 'Terminal=true mapped app_id s29-tui and received physical 4 as application input.'
next_window alcard-shell
wait_text 'Search: Terminal (1)'
capture 18-menu-before-external-input
next_window s29-tui
external_input
next_window alcard-shell
wait_text 'Search: Terminal (1)'
capture 19-menu-after-external-input
observe same-menu "$OUT/18-menu-before-external-input.png" "$OUT/19-menu-after-external-input.png"
home
wait_text ALCARD
capture 20-home-before-external-input
next_window s29-tui
external_input
next_window alcard-shell
wait_text ALCARD
capture 21-home-after-external-input
observe same-home "$OUT/20-home-before-external-input.png" "$OUT/21-home-after-external-input.png"
record 'Real Foot received digits 4-8, Space, Tab, y and F3 while focused; Alt-Tab returned to the identical committed Menu/filter/selection and unchanged Home actions without Shell Home resetting either check.'
# Clear the temporary filter through the real editor before continuing the
# existing launch/navigation cases, which intentionally cover all four apps.
physical_key 7 6
physical_key 61 F3
clear_search
physical_key 28 Return
wait_text '02 Terminal'
home
physical_key 5 4
physical_key 103 Up
physical_key 6 5
wait_text 'Opts: 02 Terminal' 'Close window'
physical_key 57 space
wait_text 'Close window?' Yes No
capture 08-close-yes-no
physical_key 9 8
observe windows s29-tui close 0
observe windows s29-tui live 1
wait_text 'Opts: 02 Terminal'
physical_key 57 space
wait_text 'Close window?' Yes No
physical_key 5 4
wait_for 'one explicit close request' observe windows s29-tui close 1
wait_for 'terminal closed acknowledgement' observe windows s29-tui closed 1
observe windows s29-tui live 0
sleep 0.2
home
physical_key 5 4
wait_text '02 Terminal' Closed
capture 09-recent-closed-history
record 'Close has only bottom Yes/No: physical 8 cancelled, physical 4 sent one close; closed terminal remained in Recent history.'

menu 2
wait_text '03 Late handoff'
physical_key 5 4
wait_for 'handoff parent exit' test -s "$S29_DIR/handoff-parent-exit"
wait_text Starting
# The tracker reaps ordinary children once per second. Let the original
# successful parent exit be observed before allowing its child's window.
sleep 1.2
wait_text Starting
capture 10-successful-parent-still-starting
menu 2
physical_key 28 Return
lines_equal "$S29_DIR/handoff-starts" 1
observe windows s29-handoff live 0
home
physical_key 5 4
touch "$S29_DIR/handoff-gate"
window_ready s29-handoff
wait_for 'handoff child ready' test -s "$S29_DIR/handoff-ready"
# A compositor may focus a newly mapped window itself. This asserts that
# the Shell did not send a stale launch-time activation request.
observe windows s29-handoff activate 0
menu 2
physical_key 5 4
wait_for 'explicit late-window activation' observe windows s29-handoff activate 1
wait_for 'late-window activation acknowledgement' observe windows s29-handoff active 1
lines_equal "$S29_DIR/handoff-starts" 1
capture 11-late-handoff-window
record 'Successful parent exit stayed pending; its later window resolved the same attempt, with no duplicate spawn or unsolicited activation.'

menu 3
wait_text '04 Failure'
physical_key 5 4
wait_for 'failing fixture execution' lines_equal "$S29_DIR/fail-starts" 1
wait_text 'Launch failed'
capture 12-launch-failed
physical_key 106 Right
wait_text 'Code: 23.'
capture 13-launch-failure-reason
physical_key 6 5
wait_text 'Opts: 04 Failure' 'Retry launch'
capture 14-explicit-retry
physical_key 28 Return
lines_equal "$S29_DIR/fail-starts" 1
wait_text 'Opts: 04 Failure'
physical_key 57 space
wait_for 'explicit retry execution' lines_equal "$S29_DIR/fail-starts" 2
wait_text 'Launch failed'
record 'Exit 23 produced Launch failed and a readable reason; context Enter did not retry, explicit Space did.'

# Both internal records must remain readable after the ordinary 5 s expiry.
sleep 5.5
home
physical_key 9 8
wait_text '04 Failure'
capture 15-internal-error-history
observe text-count "$OUT/15-internal-error-history.png" '04 Failure' 2
physical_key 5 4
wait_text 'From: Alcard Shell' 'Code: 23.'
capture 16-internal-error-detail
physical_key 9 8
physical_key 108 Down
physical_key 5 4
wait_text 'From: Alcard Shell' 'Code: 23.'
capture 17-previous-error-detail
record 'Both internal Alcard Shell errors survived expiry, retained full Code: 23 text, and were readable from notification history.'

lines_equal "$S29_DIR/slow-starts" 1
lines_equal "$S29_DIR/tui-tui-starts" 1
lines_equal "$S29_DIR/handoff-starts" 1
lines_equal "$S29_DIR/fail-starts" 2
kill -0 "$shell_pid"
if grep -Ei 'protocol error|panicked at|Wayland keyboard initialization|SHM buffer length mismatch|Could not record Shell notification' "$OUT/shell.log"; then
  exit 1
fi
printf '\nPASS: real launch, duplicate suppression, terminal identity/input, focus-local text/F3 with preserved Menu/Home, activation, cancel/close, navigation, delayed handoff, retry and internal error history.\n' >>"$OUT/manifest.txt"
printf 'PASS: launch checks and screenshots are available in %s\n' "$OUT"
