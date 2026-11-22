#!/usr/bin/env bash
# Real Shell UI with a deliberately simulated login1 on a private test bus.
# No method in this fixture changes power, sessions, credentials or host services.
# Invoke with: dbus-run-session -- bash scripts/check-shell-system.sh
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
BIN="${ALCARD_SHELL_BIN:-$ROOT/rust/apps/alcard-shell/target/debug/alcard-shell}"
POINTER_BIN="${ALCARD_POINTER_BIN:-$(dirname -- "$BIN")/examples/park-pointer}"
OUT="${ALCARD_SYSTEM_SCREENSHOT_DIR:-$ROOT/.alcard-local/system-check}"
[[ -x "$BIN" ]] || { echo 'Build alcard-shell before system checks' >&2; exit 1; }
[[ -x "$POINTER_BIN" ]] || { echo 'Build the park-pointer example before system checks' >&2; exit 1; }
[[ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]] ||
  { echo 'Run check-shell-system.sh through a fresh dbus-run-session' >&2; exit 1; }
for tool in labwc grim wtype wlr-randr dbus-daemon gdbus python3 timeout; do
  command -v "$tool" >/dev/null || { echo "Missing system-check tool: $tool" >&2; exit 1; }
done
python3 -c 'from PIL import Image; from gi.repository import Gio, GLib; import tomllib'
if [[ "$(timeout 5 gdbus call --session --dest org.freedesktop.DBus \
    --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.NameHasOwner \
    org.freedesktop.Notifications)" == *true* ]]; then
  echo 'Refusing a bus with an existing notification server; use dbus-run-session' >&2
  exit 1
fi

mkdir -p -- "$OUT"
OUT="$(cd -- "$OUT" && pwd -P)"
TMP="$(mktemp -d)"
shell_pid='' compositor_pid='' bus_pid='' fixture_pid='' keyboard_pid='' active_shell_log=''
last_wait='startup' last_key='none' case_name='startup'
stop_shell() {
  [[ -n "$shell_pid" ]] || return 0
  kill "$shell_pid" 2>/dev/null || true
  for _ in $(seq 1 20); do
    kill -0 "$shell_pid" 2>/dev/null || break
    sleep 0.1
  done
  kill -KILL "$shell_pid" 2>/dev/null || true
  wait "$shell_pid" 2>/dev/null || true
  shell_pid=''
}
stop_fixture() {
  [[ -n "$fixture_pid" ]] || return 0
  kill "$fixture_pid" 2>/dev/null || true
  wait "$fixture_pid" 2>/dev/null || true
  fixture_pid=''
}
cleanup() {
  local result=$? pid
  trap - EXIT INT TERM
  if [[ $result -ne 0 ]]; then
    [[ ! -f "$TMP/observed.png" ]] || cp -- "$TMP/observed.png" "$OUT/failure-last-observed.png" || true
    printf 'Case: %s\nLast condition: %s\nLast key: %s\nExit: %s\n' \
      "$case_name" "$last_wait" "$last_key" "$result" >"$OUT/failure.txt" || true
    tail -100 "$OUT/fixture.log" >&2 2>/dev/null || true
    tail -40 "$OUT/keyboard-idle.log" >&2 2>/dev/null || true
    [[ -z "$active_shell_log" ]] || tail -100 "$active_shell_log" >&2 2>/dev/null || true
  fi
  stop_shell
  stop_fixture
  for pid in "$keyboard_pid" "$compositor_pid" "$bus_pid"; do
    [[ -z "$pid" ]] || kill "$pid" 2>/dev/null || true
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
# An intentionally wrong environment ID must never become the logout target.
export XDG_SESSION_ID='s212-environment-is-not-a-session-target'
mkdir -m 700 "$XDG_RUNTIME_DIR"
mkdir -p "$TMP/labwc" "$XDG_CONFIG_HOME" "$XDG_CONFIG_DIRS" \
  "$XDG_DATA_HOME/applications" "$XDG_DATA_DIRS"

# This bus has no service directories or activation helpers. Its only login1
# implementation is the fixture below, connected to this exact private socket.
cat >"$TMP/system-bus.conf" <<XML
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path=$TMP/system-bus</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow user="$(id -u)"/>
    <allow own="*"/>
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
  </policy>
</busconfig>
XML
dbus-daemon --nofork --config-file="$TMP/system-bus.conf" --print-address=1 \
  >"$TMP/system-address" 2>"$OUT/private-bus.log" & bus_pid=$!
for _ in $(seq 1 50); do
  [[ ! -s "$TMP/system-address" || ! -S "$TMP/system-bus" ]] || break
  kill -0 "$bus_pid"
  sleep 0.1
done
read -r private_address <"$TMP/system-address"
[[ "$private_address" == "unix:path=$TMP/system-bus,"* && -S "$TMP/system-bus" ]] ||
  { echo 'Expected only the newly created private test-bus address' >&2; exit 1; }
export DBUS_SYSTEM_BUS_ADDRESS="$private_address"

cat >"$TMP/login1-fixture.py" <<'PY'
import json, os, pathlib, sys
from gi.repository import Gio, GLib

address, journal_path, ready_path = sys.argv[1:]
assert address == os.environ['DBUS_SYSTEM_BUS_ADDRESS']
assert address.startswith('unix:path=')
connection = Gio.DBusConnection.new_for_address_sync(
    address,
    Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
    None, None)
connection.set_exit_on_close(False)
owner = connection.get_unique_name()
case, mode = 'startup', 'allowed'
session_path = '/org/freedesktop/login1/session/c212'
session_id = 'c212'
journal = pathlib.Path(journal_path)

# Gio implements Introspectable and Properties.Get/GetAll from this exact XML.
# https://docs.gtk.org/gio/method.DBusConnection.register_object.html
xml = '''<node>
<interface name="org.freedesktop.login1.Manager">
  <method name="GetSessionByPID"><arg type="u" direction="in"/><arg type="o" direction="out"/></method>
  <method name="CanReboot"><arg type="s" direction="out"/></method>
  <method name="CanPowerOff"><arg type="s" direction="out"/></method>
  <method name="ListInhibitors"><arg type="a(ssssuu)" direction="out"/></method>
  <method name="TerminateSession"><arg type="s" direction="in"/></method>
  <method name="RebootWithFlags"><arg type="t" direction="in"/></method>
  <method name="PowerOffWithFlags"><arg type="t" direction="in"/></method>
</interface>
<interface name="org.freedesktop.login1.Session">
  <property name="Id" type="s" access="read"/>
  <property name="User" type="(uo)" access="read"/>
  <property name="Type" type="s" access="read"/>
  <property name="Remote" type="b" access="read"/>
</interface>
<interface name="org.alcard.TestLogin1">
  <method name="SetCase"><arg type="s" direction="in"/><arg type="s" direction="in"/></method>
</interface>
</node>'''
node = Gio.DBusNodeInfo.new_for_xml(xml)

def record(method, args, **extra):
    with journal.open('a') as stream:
        stream.write(json.dumps(dict(case=case, mode=mode, method=method,
                                     args=args, owner=owner, **extra)) + '\n')

def get_property(_connection, _sender, _path, _interface, name):
    values = {
        'Id': GLib.Variant('s', session_id),
        'User': GLib.Variant('(uo)', (os.getuid(), f'/org/freedesktop/login1/user/_{os.getuid()}')),
        'Type': GLib.Variant('s', 'wayland'),
        'Remote': GLib.Variant('b', False),
    }
    record('GetProperty', [name])
    return values[name]

def method_call(_connection, sender, _path, interface, method, parameters, invocation):
    global case, mode
    args = list(parameters.unpack())
    if interface == 'org.alcard.TestLogin1':
        requested_case, requested_mode = args
        if requested_mode not in {'allowed', 'challenge', 'policy', 'blocked', 'weak-blocked', 'unknown'}:
            invocation.return_dbus_error('org.freedesktop.DBus.Error.InvalidArgs', 'Unknown test scenario')
            return
        case, mode = requested_case, requested_mode
        record('SetCase', args)
        invocation.return_value(GLib.Variant('()', ()))
        return
    message = invocation.get_message()
    record(method, args, sender=sender, destination=message.get_destination(),
           flags=int(message.get_flags()))
    if method == 'GetSessionByPID':
        invocation.return_value(GLib.Variant('(o)', (session_path,)))
    elif method in {'CanReboot', 'CanPowerOff'}:
        value = 'challenge' if mode == 'challenge' else 'yes'
        if mode == 'policy':
            value = 'no' if method == 'CanReboot' else 'na'
        invocation.return_value(GLib.Variant('(s)', (value,)))
    elif method == 'ListInhibitors':
        inhibitors = []
        if mode in {'blocked', 'weak-blocked'}:
            inhibitors = [('shutdown', 'Alcard test fixture', 'Simulated unsaved work',
                           'block' if mode == 'blocked' else 'block-weak', os.getuid(), os.getpid())]
        invocation.return_value(GLib.Variant('(a(ssssuu))', (inhibitors,)))
    elif method in {'TerminateSession', 'RebootWithFlags', 'PowerOffWithFlags'}:
        # Record only: there are deliberately no real backend calls here.
        if mode == 'challenge':
            invocation.return_dbus_error('org.freedesktop.DBus.Error.AccessDenied', 'Simulated policy denial')
        elif mode == 'unknown':
            invocation.return_dbus_error('org.freedesktop.DBus.Error.NoReply', 'Simulated unknown outcome')
        else:
            invocation.return_value(GLib.Variant('()', ()))
    else:
        invocation.return_dbus_error('org.freedesktop.DBus.Error.UnknownMethod', method)

connection.register_object('/org/freedesktop/login1', node.interfaces[0], method_call, None, None)
connection.register_object(session_path, node.interfaces[1], None, get_property, None)
connection.register_object('/org/alcard/TestLogin1', node.interfaces[2], method_call, None, None)
reply = connection.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus',
    'org.freedesktop.DBus', 'RequestName',
    GLib.Variant('(su)', ('org.freedesktop.login1', 4)), GLib.VariantType.new('(u)'),
    Gio.DBusCallFlags.NONE, 5000, None)
assert reply.unpack() == (1,), reply.unpack()
record('Ready', [os.getpid()])
pathlib.Path(ready_path).write_text(owner)
GLib.MainLoop().run()
PY

start_fixture() {
  rm -f -- "$TMP/fixture-ready"
  python3 "$TMP/login1-fixture.py" "$private_address" "$OUT/login1-calls.jsonl" \
    "$TMP/fixture-ready" >>"$OUT/fixture.log" 2>&1 & fixture_pid=$!
  for _ in $(seq 1 50); do
    [[ ! -s "$TMP/fixture-ready" ]] || return 0
    kill -0 "$fixture_pid"
    sleep 0.1
  done
  echo 'Simulated login1 did not acquire its private test name' >&2
  return 1
}
set_case() {
  case_name=$1
  timeout 5 gdbus call --address "$private_address" --dest org.freedesktop.login1 \
    --object-path /org/alcard/TestLogin1 --method org.alcard.TestLogin1.SetCase \
    "$case_name" "$2" >/dev/null
}
start_fixture

export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=1
export WLR_LIBINPUT_NO_DEVICES=1 WLR_XWAYLAND=
unset WAYLAND_DISPLAY DISPLAY
cat >"$TMP/labwc/rc.xml" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<labwc_config>
  <windowRules>
    <windowRule identifier="alcard-shell" serverDecoration="no" event="onFirstMap">
      <action name="ToggleFullscreen" />
    </windowRule>
  </windowRules>
</labwc_config>
XML
printf '#!/bin/sh\nexit 0\n' >"$TMP/labwc/autostart"
printf '#!/bin/sh\nexit 0\n' >"$TMP/labwc/shutdown"
chmod 700 "$TMP/labwc/autostart" "$TMP/labwc/shutdown"
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
output="$(wlr-randr | python3 -c 'import sys; print(next(line.split()[0] for line in sys.stdin if line and not line[0].isspace()))')"
[[ "$output" == HEADLESS-* ]] || { echo "Unexpected output: $output" >&2; exit 1; }
wlr-randr --output "$output" --custom-mode 320x170@60Hz --scale 1 --pos 0,0
wlr-randr >"$OUT/output.txt"

cat >"$TMP/observe.py" <<'PY'
import json, pathlib, sys, tomllib
from PIL import Image

root, mode = pathlib.Path(sys.argv[1]), sys.argv[2]
palette = tomllib.loads((root / 'rust/crates/alcard-settings-core/themes/classic.toml').read_text())['palette']
colors = {tuple(bytes.fromhex(palette[key][1:])) for key in
          ('text', 'muted', 'accent', 'disabled', 'success', 'warning', 'error')}
glyphs = {}
for line in (root / 'fonts/alcard-pixel/source/AlcardPixel-Regular.hex').read_text().splitlines():
    if ':' in line:
        code, bits = line.split(':', 1)
        if len(bits) == 16:
            glyphs[chr(int(code, 16))] = bytes.fromhex(bits)

def positions(path, text):
    image = Image.open(path).convert('RGB')
    assert image.size == (320, 170), image.size
    pixels = bytes(pixel in colors for pixel in image.getdata())
    rows = [pixels[y * 320:(y + 1) * 320] for y in range(170)]
    template = [bytes(bool(glyphs[ch][y] & (0x80 >> x)) for ch in text for x in range(8)) for y in range(8)]
    anchor = max(range(8), key=lambda row: sum(template[row]))
    assert sum(template[anchor]), 'Text assertion must contain a visible glyph'
    found = []
    for y in range(163):
        start = 0
        while (x := rows[y + anchor].find(template[anchor], start)) != -1:
            if all(rows[y + line][x:x + len(template[line])] == template[line] for line in range(8)):
                found.append((x, y))
            start = x + 1
    return found

if mode == 'text':
    raise SystemExit(0 if all(positions(sys.argv[3], text) for text in sys.argv[4:]) else 1)
if mode == 'footer':
    for text in sys.argv[4:]:
        found = positions(sys.argv[3], text)
        assert any(y >= 154 for _, y in found), f'Missing bottom action {text}'
        assert not any(90 <= y < 148 for _, y in found), f'Duplicated internal action {text}'
if mode == 'receipts':
    journal, case, expected, expected_pid = sys.argv[3:7]
    rows = [json.loads(line) for line in pathlib.Path(journal).read_text().splitlines()]
    rows = [row for row in rows if row['case'] == case]
    mutations = [row for row in rows if row['method'] in {'TerminateSession', 'RebootWithFlags', 'PowerOffWithFlags'}]
    if expected == 'none':
        assert not mutations, mutations
    else:
        assert len(mutations) == 1 and mutations[0]['method'] == expected, mutations
        call = mutations[0]
        assert call['args'] == (['c212'] if expected == 'TerminateSession' else [1]), call
        assert call['flags'] & 4, f'AllowInteractiveAuth flag missing: {call}'
        assert call['destination'] == call['owner'], f'Mutation did not pin the unique owner: {call}'
        if expected == 'TerminateSession':
            probes = [row['args'] for row in rows if row['method'] == 'GetSessionByPID']
            assert probes and all(args == [int(expected_pid)] for args in probes), probes
PY
observe() { python3 "$TMP/observe.py" "$ROOT" "$@"; }
wait_for() {
  local description=$1 deadline=$((SECONDS + 10)); shift
  last_wait=$description
  while ((SECONDS < deadline)); do
    kill -0 "$compositor_pid"
    kill -0 "$bus_pid"
    [[ -z "$shell_pid" ]] || kill -0 "$shell_pid"
    [[ -z "$keyboard_pid" ]] || kill -0 "$keyboard_pid"
    if "$@"; then return 0; fi
    sleep 0.1
  done
  printf 'Timed out waiting for %s\n' "$description" >&2
  return 1
}
screen_has() {
  timeout 5 grim -o "$output" "$TMP/observed.png" || return 1
  observe text "$TMP/observed.png" "$@"
}
wait_text() { wait_for "visible text: $*" screen_has "$@"; }
keyboard_ready() {
  python3 - "$OUT/keyboard-idle.log" <<'PY'
import pathlib, re, sys
log = pathlib.Path(sys.argv[1]).read_text()
keymap = re.search(r'zwp_virtual_keyboard_v1[^\n]*\.keymap\(', log)
# wtype uploads its keymap and waits for a Wayland roundtrip before sleeping.
acknowledged = keymap and re.search(r'wl_callback[^\n]*\.done\(', log[keymap.end():])
raise SystemExit(0 if acknowledged else 1)
PY
}
shell_keyboard_ready() {
  python3 - "$active_shell_log" "${1:-wait}" <<'PY'
import pathlib, re, sys
events = list(re.finditer(r'wl_keyboard[@#](\d+)\.(keymap|enter|modifiers),?\s*\(([^)]*)\)',
                          pathlib.Path(sys.argv[1]).read_text()))
enter = next((event for event in events if event[2] == 'enter'), None)
if enter is None:
    raise SystemExit(1)
keyboard = [event for event in events if event[1] == enter[1]]
modifiers = next((event for event in keyboard if event[2] == 'modifiers'), None)
if modifiers is None:
    raise SystemExit(1)
if sys.argv[2] == 'check':
    keymap = next((event for event in keyboard if event[2] == 'keymap'), None)
    assert keymap and keymap[3].split(',')[0].strip() == '1', 'Initial keyboard has no XkbV1 keymap'
    assert keymap.start() < enter.start() < modifiers.start(), 'Expected initial XkbV1 keymap -> enter -> modifiers'
PY
}
physical_key() {
  local raw=$1 symbol=$2 placeholder slot
  last_key="evdev $raw ($symbol)"
  local -a args=(-s 100)
  for ((slot=1; slot<raw; slot++)); do
    printf -v placeholder 'U%04X' "$((0xE000 + slot))"
    args+=(-p "$placeholder")
  done
  args+=(-s 100 -P "$symbol" -s 30 -p "$symbol" -s 100)
  WAYLAND_DEBUG=1 timeout 5 wtype "${args[@]}" >"$TMP/wtype.out" 2>"$TMP/wtype.log"
  python3 - "$TMP/wtype.log" "$raw" <<'PY'
import pathlib, re, sys
text = pathlib.Path(sys.argv[1]).read_text()
assert re.search(r'zwp_virtual_keyboard_v1[^\n]*\.key\([^,]*,\s*' + sys.argv[2] + r',\s*1\)', text)
PY
  cat "$TMP/wtype.log" >>"$OUT/keyboard.log"
  sleep 0.15
}
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
with manifest.open('a') as stream:
    stream.write(f'{path.name}: 320x170; simulated login1; SHA256 {hashlib.sha256(path.read_bytes()).hexdigest()}\n')
PY
}
receipts() { observe receipts "$OUT/login1-calls.jsonl" "$case_name" "$1" "$shell_pid"; }
shell_registered() {
  local registration
  registration="$(timeout 5 gdbus call --session --dest org.freedesktop.DBus \
    --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.GetConnectionUnixProcessID \
    org.freedesktop.Notifications 2>/dev/null)" || return 1
  [[ "$registration" == "(uint32 $shell_pid,)" ]]
}
start_shell() {
  # Short wtype calls replace labwc's current keyboard; the old idle device is
  # not restored when they exit. Install a fresh current device before EACH
  # Shell starts, so SCTK receives its keymap before the initial focus event.
  if [[ -n "$keyboard_pid" ]]; then
    kill "$keyboard_pid" 2>/dev/null || true
    wait "$keyboard_pid" 2>/dev/null || true
    keyboard_pid=''
  fi
  # This background safety cap does not change any assertion deadline.
  WAYLAND_DEBUG=1 wtype -s 300000 >"$OUT/keyboard-idle.out" 2>"$OUT/keyboard-idle.log" & keyboard_pid=$!
  wait_for 'fresh virtual keyboard keymap acknowledgement' keyboard_ready
  active_shell_log="$OUT/$case_name-shell.log"
  WAYLAND_DEBUG=1 "$BIN" --theme classic --locale en \
    >"$active_shell_log" 2>&1 & shell_pid=$!
  wait_for 'the new Shell process owns its notification service' shell_registered
  wait_for 'initial Shell keyboard focus and modifiers' shell_keyboard_ready
  shell_keyboard_ready check
  # Keep the compositor cursor away from glyphs; pixel assertions remain exact.
  ALCARD_POINTER_FIXTURE=1 timeout 5 "$POINTER_BIN"
  wait_text ALCARD Quick
  printf 'Case %s: Shell PID %s; private simulated login1.\n' "$case_name" "$shell_pid" >>"$OUT/manifest.txt"
}
begin_case() {
  stop_shell
  set_case "$1" "$2"
  start_shell
  physical_key 6 5
  wait_text 'Quick settings' Brightness Volume Wi-Fi Bluetooth
  # Keep all three system capability rows visible below the radio controls.
  focus_operation 2
}
focus_operation() {
  local index=$1
  for ((i=0; i<8; i++)); do physical_key 103 Up; done
  for ((i=0; i<4+index; i++)); do physical_key 108 Down; done
}
select_operation() {
  focus_operation "$1"
  physical_key 5 4
}
assert_latched() {
  local expected=$1 message=$2
  physical_key 5 4
  physical_key 28 Return
  physical_key 6 5
  receipts "$expected"
  physical_key 9 8
  wait_text ALCARD Quick
  physical_key 6 5
  wait_text "$message" Back
  physical_key 5 4
  physical_key 6 5
  receipts "$expected"
}

cat >"$OUT/manifest.txt" <<'TEXT'
Alcard Shell: real production binary, simulated login1 on a private test bus.
Original 320x170 grim screenshots; Classic/EN; separate headless labwc/Pixman.
The system bus is a new private TMP socket, without service activation directories.
The fake login1 records calls and returns fixture replies; it never invokes host services.
No real logout, reboot, power-off, polkit authorization or physical power cycle is tested here.
Challenge checks only capabilities and AllowInteractiveAuth/flags; real polkit/PAM is a separate existing gate.
Current-session logout is checked against GetSessionByPID, not XDG_SESSION_ID.
All power mutations target the pinned unique fake owner and flags=1; no bool fallback.
A fresh idle virtual keyboard is prepared before each Shell start.
The native virtual-pointer fixture parks the cursor at the upper-right status edge before each Shell check; screenshots are not retouched.
Hardware indicators are the execution host's real read-only observations, not Cardputer evidence.
TEXT
python3 - "$BIN" "$OUT/manifest.txt" <<'PY'
import datetime, hashlib, pathlib, sys
binary, manifest = map(pathlib.Path, sys.argv[1:])
with manifest.open('a') as stream:
    stream.write('UTC: ' + datetime.datetime.now(datetime.timezone.utc).isoformat() + '\n')
    stream.write('Binary SHA256: ' + hashlib.sha256(binary.read_bytes()).hexdigest() + '\n')
PY

begin_case logout allowed
wait_text Available
capture 01-mock-quick-menu
physical_key 6 5
physical_key 7 6
wait_text 'Quick settings'
receipts none
select_operation 0
wait_text 'Log out?' Yes No
capture 02-mock-logout-confirm
observe footer "$OUT/02-mock-logout-confirm.png" Yes No
physical_key 9 8
wait_text 'Quick settings'
receipts none
capture 03-mock-logout-cancelled
select_operation 0
wait_text 'Log out?' Yes No
physical_key 5 4
wait_text 'The system accepted' Back
receipts TerminateSession
capture 04-mock-logout-accepted
assert_latched TerminateSession 'The system accepted'
capture 05-mock-accepted-latched

begin_case reboot allowed
wait_text Available
select_operation 1
wait_text 'Restart the device?' Yes No
capture 06-mock-restart-confirm
observe footer "$OUT/06-mock-restart-confirm.png" Yes No
physical_key 28 Return
wait_text 'The system accepted' Back
receipts RebootWithFlags
capture 07-mock-restart-accepted

begin_case poweroff allowed
wait_text Available
select_operation 2
wait_text 'Power off the device?' Yes No
capture 08-mock-poweroff-confirm
observe footer "$OUT/08-mock-poweroff-confirm.png" Yes No
physical_key 5 4
wait_text 'The system accepted' Back
receipts PowerOffWithFlags
capture 09-mock-poweroff-accepted

begin_case challenge challenge
wait_text 'Authentication required'
capture 10-mock-authentication-required
select_operation 2
wait_text 'Power off the device?' Yes No
physical_key 5 4
wait_text 'The system did not permit' OK Back
receipts PowerOffWithFlags
capture 11-mock-authorization-denied

begin_case policy policy
wait_text 'Not permitted' 'Not supported'
capture 12-mock-capabilities
select_operation 1
wait_text Restart 'Not permitted' Retry
receipts none
capture 13-mock-restart-not-permitted
physical_key 28 Return
wait_text 'Quick settings'
select_operation 2
wait_text 'Power off' 'Not supported' Retry
receipts none
capture 14-mock-poweroff-unsupported
set_case policy allowed
physical_key 6 5
wait_text 'Quick settings' Available
receipts none
capture 15-mock-capabilities-refreshed

begin_case inhibitor blocked
wait_text 'Blocked by the system'
select_operation 2
wait_text 'Power off' 'Blocked by the system' Retry
receipts none
capture 16-mock-inhibited
set_case inhibitor weak-blocked
physical_key 6 5
wait_text 'Quick settings' 'Blocked by the system'
select_operation 2
wait_text 'Blocked by the system' Retry
receipts none

stop_shell
stop_fixture
case_name=unavailable
start_shell
physical_key 6 5
wait_text 'Quick settings' Brightness Volume Wi-Fi Bluetooth
focus_operation 2
wait_text 'Quick settings' 'Service unavailable'
capture 17-mock-service-unavailable
select_operation 2
wait_text 'Power off' 'Service unavailable' Retry
capture 18-mock-service-retry
start_fixture
set_case recovered allowed
physical_key 6 5
wait_text 'Quick settings' Available
receipts none
capture 19-mock-service-recovered

begin_case unknown unknown
wait_text Available
select_operation 1
wait_text 'Restart the device?' Yes No
physical_key 5 4
wait_text 'The result of the request' Back
receipts RebootWithFlags
capture 20-mock-outcome-unknown
assert_latched RebootWithFlags 'The result of the request'
capture 21-mock-unknown-latched

begin_case owner-change allowed
wait_text Available
select_operation 1
wait_text 'Restart the device?' Yes No
stop_fixture
start_fixture
set_case owner-change allowed
physical_key 5 4
wait_text 'The system action service' OK Back
receipts none
capture 22-mock-owner-changed
physical_key 28 Return
wait_text 'Quick settings' Available
select_operation 1
wait_text 'Restart the device?' Yes No
physical_key 5 4
wait_text 'The system accepted' Back
receipts RebootWithFlags
capture 23-mock-owner-refreshed

kill -0 "$shell_pid"
if grep -Ei 'protocol error|panicked at|Wayland keyboard initialization|SHM buffer length mismatch' "$OUT/"*-shell.log; then
  exit 1
fi
printf '\nPASS: real Shell quick settings with simulated login1; cancel, current-session identity, pinned owner, flags/auth, policy/inhibitors, retry, accepted/unknown duplicate protection. No real system action executed.\n' >>"$OUT/manifest.txt"
printf 'PASS: simulated login1 screenshots and receipts are available in %s\n' "$OUT"
