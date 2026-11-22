#!/usr/bin/env bash
# Installed Alpine software acceptance in an existing disposable container.
# No builds, installs, real power requests, host sessions or audio devices.
# The caller supplies the already-built musl locker test binary and two empty
# fixture directories mounted read-only over the container's sysfs classes.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
OUT="${ALCARD_CI_RUNTIME_EVIDENCE:-$ROOT/.alcard-local/ci/acceptance/runtime}"
LOCK_TEST_BIN="${ALCARD_LOCK_TEST_BIN:-}"
SHELL_TEST_BIN="${ALCARD_SHELL_TEST_BIN:-}"
POINTER_BIN="${ALCARD_POINTER_BIN:-}"
OUTPUT_POWER_BIN="${ALCARD_OUTPUT_POWER_BIN:-}"
TEST_USER=alcard-ci-runtime
TEST_HOME=/home/alcard-ci-runtime
TEST_UID='' TMP='' created_user=0

[[ ${ALCARD_CI_RUNTIME_CONTAINER:-} == 1 && -f /.dockerenv && $(id -u) == 0 ]] || {
  echo 'Refusing installed runtime checks outside an explicit disposable container' >&2
  exit 1
}
[[ -r /etc/alpine-release && $(cat /etc/alpine-release) == 3.24.* && $(apk --print-arch) == aarch64 ]] || {
  echo 'Installed runtime acceptance requires native Alpine 3.24 aarch64' >&2
  exit 1
}
[[ "$OUT" == /* && "$OUT" != / && ! -L "$OUT" ]] || exit 1
[[ "$LOCK_TEST_BIN" == /* && -x "$LOCK_TEST_BIN" ]] || {
  echo 'ALCARD_LOCK_TEST_BIN must name the musl locker test executable from this source gate' >&2
  exit 1
}
[[ "$SHELL_TEST_BIN" == /* && -x "$SHELL_TEST_BIN" && "$POINTER_BIN" == /* && -x "$POINTER_BIN" ]] || {
  echo 'ALCARD_SHELL_TEST_BIN and ALCARD_POINTER_BIN must name this source gate musl executables' >&2
  exit 1
}
[[ "$OUTPUT_POWER_BIN" == /* && -x "$OUTPUT_POWER_BIN" ]] || {
  echo 'ALCARD_OUTPUT_POWER_BIN must name the source gate output-power fixture executable' >&2
  exit 1
}
[[ ! -e /dev/snd ]] || { echo 'Do not mount sound devices into this private runtime fixture' >&2; exit 1; }
for program in bash python3 timeout runuser dbus-run-session labwc wtype grim wlr-randr \
    gdbus sha256sum chpasswd adduser deluser; do
  command -v "$program" >/dev/null || { echo "Missing runtime dependency: $program" >&2; exit 1; }
done
for binary in /usr/bin/alcard-shell /usr/bin/alcard-lock /usr/libexec/alcard/alcard-radio-helper \
    /usr/libexec/alcard/audio-session \
    /usr/bin/pipewire /usr/bin/wireplumber /usr/bin/pipewire-pulse /usr/bin/pw-dump /usr/bin/wpctl; do
  [[ -x "$binary" ]] || { echo "Missing installed runtime binary: $binary" >&2; exit 1; }
  apk info --who-owns "$binary" >/dev/null
done
[[ -r /etc/pam.d/alcard-lock && -f "$ROOT/scripts/check-shell-battery.sh" ]] || {
  echo 'Installed PAM service and battery runtime gate are required' >&2; exit 1;
}
python3 -c 'from PIL import Image; from gi.repository import Gio, GLib; import tomllib'
python3 - <<'PY'
import os, pathlib
for name in ('ALCARD_BATTERY_SYSFS_FIXTURE', 'ALCARD_BACKLIGHT_SYSFS_FIXTURE'):
    path = pathlib.Path(os.environ.get(name, ''))
    assert path.is_absolute() and path.is_dir() and not path.is_symlink(), name
    assert not any(path.iterdir()), f'{name} must start empty'
PY
if id "$TEST_USER" >/dev/null 2>&1; then
  echo 'The disposable runtime test account already exists; use a clean container' >&2
  exit 1
fi

mkdir -p -- "$OUT"
OUT="$(cd -- "$OUT" && pwd -P)"
: >"$OUT/stages.tsv"
finish() {
  local result=$?
  trap - EXIT INT TERM
  if [[ $created_user == 1 ]]; then
    # This UID was created above solely for this gate. No pre-existing account
    # or process is eligible for cleanup; fixture credentials are never output.
    python3 - "$TEST_UID" <<'PY' || true
import os, pathlib, signal, sys, time
uid = int(sys.argv[1])
def owned():
    result = []
    for entry in pathlib.Path('/proc').iterdir():
        if not entry.name.isdecimal(): continue
        try:
            fields = (entry / 'status').read_text().splitlines()
            owner = next(line.split()[1] for line in fields if line.startswith('Uid:'))
            if int(owner) == uid: result.append(int(entry.name))
        except (OSError, StopIteration): pass
    return result
for sig in (signal.SIGTERM, signal.SIGKILL):
    for pid in owned():
        try: os.kill(pid, sig)
        except ProcessLookupError: pass
    time.sleep(.2)
PY
    rm -f -- "$TEST_HOME/.lock-test-secret"
    deluser --remove-home "$TEST_USER" >/dev/null 2>&1 || true
  fi
  [[ -z "$TMP" ]] || rm -rf -- "$TMP"
  python3 - "$OUT" "$result" <<'PY'
import json, pathlib, sys, time
out = pathlib.Path(sys.argv[1]); code = int(sys.argv[2])
stages = []
for line in (out / 'stages.tsv').read_text().splitlines():
    name, started, elapsed, status = line.split('\t')
    stages.append(dict(name=name, started_unix=int(started), elapsed_seconds=int(elapsed), exit_code=int(status)))
(out / 'result.json').write_text(json.dumps({
    'schema': 1, 'scope': 'installed Alpine software runtime with private headless compositor, real PAM and PipeWire; simulated login1/sysfs',
    'status': 'PASS' if code == 0 else 'FAIL', 'exit_code': code,
    'finished_unix': int(time.time()), 'stages': stages,
    'not_proven': ['installed elogind mutation', 'kernel suspend/resume', 'physical battery/backlight/audio', 'Cardputer wake and drivers', 'resource benchmark'],
}, indent=2) + '\n')
PY
  chmod -R a+rX -- "$OUT"
  exit "$result"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
{
  printf '%s\n' 'Alcard session installed Alpine software runtime gate.'
  printf '%s\n' 'Private labwc/PipeWire and disposable current-user PAM; no host devices or power operations.'
  cat /etc/alpine-release
  uname -m
  apk info -v alcard-shell alcard-lock alcard-desktop-shell labwc pipewire wireplumber pipewire-pulse
  sha256sum /usr/bin/alcard-shell /usr/bin/alcard-lock /usr/libexec/alcard/alcard-radio-helper \
    /usr/share/polkit-1/actions/org.alcard.radio.policy /etc/dbus-1/system-services/org.bluez.service \
    /etc/alcard/bluetooth.conf /usr/libexec/alcard/audio-session \
    "$LOCK_TEST_BIN" "$SHELL_TEST_BIN" "$POINTER_BIN" "$OUTPUT_POWER_BIN"
} >"$OUT/manifest.txt"

adduser -D -s /bin/bash "$TEST_USER"
created_user=1
TEST_UID="$(id -u "$TEST_USER")"
python3 - "$TEST_USER" "$TEST_HOME" <<'PY'
import os, pathlib, pwd, subprocess, sys
name, home = sys.argv[1:]; account = pwd.getpwnam(name)
secret = '45678' + os.urandom(18).hex()
subprocess.run(['chpasswd'], input=f'{name}:{secret}\n', text=True, check=True, stdout=subprocess.DEVNULL)
path = pathlib.Path(home) / '.lock-test-secret'
fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, 'w') as stream: stream.write(secret)
os.chown(path, account.pw_uid, account.pw_gid)
PY
TMP="$(mktemp -d)"
chown "$TEST_USER:$TEST_USER" "$TMP"
install -d -o "$TEST_USER" -g "$TEST_USER" -m 755 "$OUT/idle" "$OUT/audio" "$OUT/output-power"
install -d -m 755 /opt/alcard
for name in alcard-shell alcard-lock; do
  if [[ -e "/opt/alcard/$name" ]]; then
    cmp "/usr/bin/$name" "/opt/alcard/$name"
  else
    install -m 755 "/usr/bin/$name" "/opt/alcard/$name"
  fi
done
install -m 755 "$LOCK_TEST_BIN" "$TMP/test-lock"
install -m 755 "$SHELL_TEST_BIN" "$TMP/test-shell"
install -m 755 "$POINTER_BIN" "$TMP/park-pointer"
install -m 755 "$OUTPUT_POWER_BIN" "$TMP/output-power-fixture"
export CI_FIXTURE_ROOT="$ROOT" CI_FIXTURE_OUT="$OUT" CI_FIXTURE_TMP="$TMP"

# This adapter reuses the real ignored Rust idle test, including its input
# observer, current-user PAM, canonical lifecycle and two-output assertions.
# Only public idle/power protocol messages reach disk; keyboard traces do not.
cat >"$TMP/idle.sh" <<'IDLE'
#!/usr/bin/env bash
set -euo pipefail
export ALCARD_LOCK_TEST_CONTAINER=1 ALCARD_IDLE_TEST=1
export ALCARD_LOCK_TEST_HELPER=/opt/alcard/alcard-lock
export ALCARD_LOCK_SCREENSHOT_DIR="$CI_FIXTURE_OUT/idle"
export XDG_RUNTIME_DIR="$(mktemp -d)"
chmod 700 "$XDG_RUNTIME_DIR"
export XDG_CONFIG_HOME="$XDG_RUNTIME_DIR/config" XDG_DATA_HOME="$XDG_RUNTIME_DIR/data"
export XDG_CONFIG_DIRS="$XDG_RUNTIME_DIR/no-config" XDG_DATA_DIRS="$XDG_RUNTIME_DIR/no-data"
export DBUS_SYSTEM_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/no-system-bus"
mkdir -p "$XDG_CONFIG_HOME/labwc" "$XDG_DATA_HOME" "$XDG_CONFIG_DIRS" "$XDG_DATA_DIRS" "$XDG_RUNTIME_DIR/staged"
cat >"$XDG_CONFIG_HOME/labwc/rc.xml" <<'XML'
<?xml version="1.0"?><labwc_config><focus><followMouse>no</followMouse></focus><keyboard><default/></keyboard></labwc_config>
XML
printf '#!/bin/sh\nexit 0\n' >"$XDG_CONFIG_HOME/labwc/autostart"
cp "$XDG_CONFIG_HOME/labwc/autostart" "$XDG_CONFIG_HOME/labwc/shutdown"
chmod 700 "$XDG_CONFIG_HOME/labwc/"{autostart,shutdown}
cp /opt/alcard/alcard-shell "$XDG_RUNTIME_DIR/staged/alcard-shell-real"
cat >"$XDG_RUNTIME_DIR/staged/alcard-lock" <<'LOCK'
#!/usr/bin/env bash
set -euo pipefail
(
  # Exec preserves this lifetime. The hardened locker deliberately makes
  # /proc/PID/exe unreadable; its canonical LOCKED line proves exec completed.
  native_pid=$BASHPID
  read -r native_stat <"/proc/$native_pid/stat"
  read -r -a native_fields <<<"${native_stat##*) }"
  [[ ${native_fields[19]:-} =~ ^[0-9]+$ ]]
  printf '%s %s\n' "$native_pid" "${native_fields[19]}" >"$XDG_RUNTIME_DIR/shell-lock.identity.txt"
  printf '%s\n' "$native_pid" >"$XDG_RUNTIME_DIR/shell-lock.pid"
  exec /opt/alcard/alcard-lock "$@"
) | while IFS= read -r lifecycle; do
  case "$lifecycle" in
    LOCKED|UNLOCKED|FAILED)
      printf '%s\n' "$lifecycle" >>"$XDG_RUNTIME_DIR/shell-lock.receipts"
      printf '\nALCARD_IDLE_LIFECYCLE %s\n' "$lifecycle" >&2
      printf '%s\n' "$lifecycle"
      ;;
    *) exit 97 ;;
  esac
done
LOCK
cat >"$XDG_RUNTIME_DIR/idle-protocol-filter.py" <<'FILTER'
import re, sys
protocol = re.compile(rb'^\[[0-9 .]+\](?:\[rs\])? (?:<-|->) (?:ext_idle_notifier_v1|ext_idle_notification_v1|zwlr_output_power_manager_v1|zwlr_output_power_v1)[@#][0-9]+\.[a-z_]+,?\s*\([^\r\n]*\)\s*$')
receipts = {b'ALCARD_IDLE_LIFECYCLE LOCKED', b'ALCARD_IDLE_LIFECYCLE UNLOCKED', b'ALCARD_IDLE_LIFECYCLE FAILED'}
for line in sys.stdin.buffer:
    if line.strip() in receipts or protocol.fullmatch(line):
        sys.stdout.buffer.write(line); sys.stdout.buffer.flush()
FILTER
cat >"$XDG_RUNTIME_DIR/staged/alcard-shell" <<'SHELL'
#!/usr/bin/env bash
set -euo pipefail
exec "$XDG_RUNTIME_DIR/staged/alcard-shell-real" "$@" 2> >(python3 -u "$XDG_RUNTIME_DIR/idle-protocol-filter.py" >&2)
SHELL
chmod 755 "$XDG_RUNTIME_DIR/staged/"{alcard-lock,alcard-shell}
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=2 WLR_LIBINPUT_NO_DEVICES=1 XKB_DEFAULT_LAYOUT=us WLR_XWAYLAND=
unset DISPLAY WAYLAND_DISPLAY WAYLAND_SOCKET WAYLAND_DEBUG
compositor='' keyboard=''
cleanup() {
  local result=$?
  trap - EXIT INT TERM
  for pid in "$keyboard" "$compositor"; do
    [[ -z "$pid" ]] || kill "$pid" 2>/dev/null || true
  done
  for pid in "$keyboard" "$compositor"; do
    [[ -z "$pid" ]] || wait "$pid" 2>/dev/null || true
  done
  if ! python3 - "$HOME/.lock-test-secret" "$XDG_RUNTIME_DIR" "$ALCARD_LOCK_SCREENSHOT_DIR" <<'SCAN'
import pathlib, sys
secret = pathlib.Path(sys.argv[1]).read_bytes()
for root in map(pathlib.Path, sys.argv[2:]):
    for path in root.rglob('*'):
        if path.is_file() and path.suffix in {'.log', '.txt', '.png', '.receipts', '.pid'}:
            if secret and secret in path.read_bytes():
                for artifact in pathlib.Path(sys.argv[3]).iterdir():
                    if artifact.is_file(): artifact.unlink()
                print('Runtime output rejected: fixture credential leaked; contents suppressed', file=sys.stderr)
                raise SystemExit(1)
SCAN
  then result=1
  else
    for name in idle-shell-protocol.log idle-restarted-protocol.log idle-foreign-protocol.log shell-lock.receipts shell-lock.identity.txt test.log labwc.log; do
      [[ ! -f "$XDG_RUNTIME_DIR/$name" ]] || cp "$XDG_RUNTIME_DIR/$name" "$ALCARD_LOCK_SCREENSHOT_DIR/$name"
    done
    [[ ! -f "$XDG_RUNTIME_DIR/test.log" ]] || cat "$XDG_RUNTIME_DIR/test.log"
  fi
  rm -rf -- "$XDG_RUNTIME_DIR"
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
labwc -C "$XDG_CONFIG_HOME/labwc" >"$XDG_RUNTIME_DIR/labwc.log" 2>&1 & compositor=$!
for _ in $(seq 1 100); do
  for socket in "$XDG_RUNTIME_DIR"/wayland-*; do
    if [[ -S "$socket" ]]; then export WAYLAND_DISPLAY="$(basename -- "$socket")"; break 2; fi
  done
  kill -0 "$compositor"; sleep .1
done
[[ -n ${WAYLAND_DISPLAY:-} ]]
mapfile -t outputs < <(wlr-randr | awk 'NF && $0 !~ /^[[:space:]]/ { print $1 }')
[[ ${#outputs[@]} == 2 ]]
for index in 0 1; do
  [[ ${outputs[$index]} == HEADLESS-* ]]
  wlr-randr --output "${outputs[$index]}" --custom-mode 320x170@60Hz --scale 1 --pos "$((index * 320)),0"
done
export ALCARD_LOCK_TEST_OUTPUT="${outputs[0]}"
wtype -p Ue000 -s 240000 >/dev/null 2>&1 & keyboard=$!
sleep .2
timeout --kill-after=5 210 "$CI_FIXTURE_TMP/test-lock" --ignored --exact --test-threads=1 --nocapture \
  integration::real_idle_lock_and_output_power >"$XDG_RUNTIME_DIR/test.log" 2>&1
grep -Eq 'test result: ok\. 1 passed; 0 failed; 0 ignored;' "$XDG_RUNTIME_DIR/test.log"
IDLE

cat >"$TMP/audio.sh" <<'AUDIO'
#!/usr/bin/env bash
set -euo pipefail
OUT="$CI_FIXTURE_OUT/audio"
export XDG_RUNTIME_DIR="$(mktemp -d)"
chmod 700 "$XDG_RUNTIME_DIR"
export XDG_CONFIG_HOME="$XDG_RUNTIME_DIR/config" XDG_DATA_HOME="$XDG_RUNTIME_DIR/data" XDG_STATE_HOME="$XDG_RUNTIME_DIR/state"
export XDG_CONFIG_DIRS="$XDG_RUNTIME_DIR/no-config" XDG_DATA_DIRS="$XDG_RUNTIME_DIR/no-data"
export DBUS_SYSTEM_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/no-system-bus"
export PIPEWIRE_REMOTE=pipewire-0 PIPEWIRE_CORE=pipewire-0 PULSE_SERVER="unix:$XDG_RUNTIME_DIR/pulse/native"
unset PIPEWIRE_RUNTIME_DIR PIPEWIRE_DAEMON PIPEWIRE_CONFIG_DIR PIPEWIRE_CONFIG_NAME PIPEWIRE_CONFIG_PREFIX \
  WIREPLUMBER_CONFIG_DIR WIREPLUMBER_CONFIG_NAME PULSE_RUNTIME_PATH
mkdir -p "$XDG_CONFIG_HOME/pipewire/pipewire.conf.d" "$XDG_CONFIG_HOME/wireplumber/wireplumber.conf.d" \
  "$XDG_CONFIG_HOME/alcard" "$XDG_CONFIG_HOME/labwc" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CONFIG_DIRS" "$XDG_DATA_DIRS"
cat >"$XDG_CONFIG_HOME/pipewire/pipewire.conf.d/90-ci-null.conf" <<'CONF'
context.objects = [
  { factory = adapter
    args = { factory.name = support.null-audio-sink
      node.name = alcard-ci-null
      node.description = "Alcard private software sink"
      media.class = Audio/Sink
      audio.position = [ FL FR ]
    }
  }
]
CONF
cat >"$XDG_CONFIG_HOME/wireplumber/wireplumber.conf.d/90-ci-private.conf" <<'CONF'
wireplumber.profiles = {
  main = {
    monitor.alsa = disabled
    monitor.bluez = disabled
    monitor.bluez-midi = disabled
    monitor.v4l2 = disabled
    monitor.libcamera = disabled
  }
}
CONF
cat >"$XDG_CONFIG_HOME/alcard/shell.toml" <<'CONF'
schema_version = 1
theme = "classic"
locale = "en"
[idle]
lock_after_seconds = 0
screen_off_after_seconds = 0
CONF
cat >"$XDG_CONFIG_HOME/labwc/rc.xml" <<'XML'
<?xml version="1.0"?><labwc_config><windowRules><windowRule identifier="alcard-shell" serverDecoration="no" event="onFirstMap"><action name="ToggleFullscreen"/></windowRule></windowRules></labwc_config>
XML
printf '#!/bin/sh\nexit 0\n' >"$XDG_CONFIG_HOME/labwc/autostart"
cp "$XDG_CONFIG_HOME/labwc/autostart" "$XDG_CONFIG_HOME/labwc/shutdown"
chmod 700 "$XDG_CONFIG_HOME/labwc/"{autostart,shutdown}
compositor='' keyboard='' shell_pid='' audio_pid='' output='' current_step='audio setup'
stop_pid() {
  local pid=$1
  [[ -n "$pid" ]] || return 0
  kill "$pid" 2>/dev/null || true
  for _ in $(seq 1 40); do kill -0 "$pid" 2>/dev/null || break; sleep .1; done
  kill -KILL "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}
cleanup() {
  local result=$?
  trap - EXIT INT TERM
  if [[ $result != 0 ]]; then
    # This fixture enters only public navigation, never PAM or credentials.
    # Retain the observed UI and graph before stopping their private owners.
    printf 'status=%s\nstep=%s\n' "$result" "$current_step" >"$OUT/failure.txt"
    [[ ! -f "$XDG_RUNTIME_DIR/observed.png" ]] || cp "$XDG_RUNTIME_DIR/observed.png" "$OUT/last-observed.png"
    [[ ! -f "$XDG_RUNTIME_DIR/graph.json" ]] || cp "$XDG_RUNTIME_DIR/graph.json" "$OUT/last-observed-pw.json"
    if [[ -n "$output" && -n "$compositor" ]] && kill -0 "$compositor" 2>/dev/null; then
      timeout 3 grim -o "$output" "$OUT/failure.png" 2>>"$OUT/failure-diagnostics.log" || true
    fi
    if [[ -n "$audio_pid" ]] && kill -0 "$audio_pid" 2>/dev/null; then
      timeout 3 /usr/bin/pw-dump -r pipewire-0 >"$OUT/failure-pw.json" 2>>"$OUT/failure-diagnostics.log" || true
    fi
  fi
  stop_pid "$shell_pid"; stop_pid "$keyboard"; stop_pid "$audio_pid"; stop_pid "$compositor"
  rm -rf -- "$XDG_RUNTIME_DIR"
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
cat >"$XDG_RUNTIME_DIR/observe.py" <<'PY'
import json, math, os, pathlib, signal, sys, tomllib
from PIL import Image
mode = sys.argv[1]
if mode in {'node', 'default', 'level'}:
    graph = json.loads(pathlib.Path(sys.argv[2]).read_text())
    nodes = [entry for entry in graph if entry.get('type') == 'PipeWire:Interface:Node' and entry.get('info', {}).get('props', {}).get('node.name') == 'alcard-ci-null']
    assert len(nodes) == 1, 'Expected exactly the private null sink'
    node = nodes[0]
    assert node['info']['props']['media.class'] == 'Audio/Sink'
    if mode == 'node': print(node['id'])
    else:
        defaults = [p for entry in graph if entry.get('type') == 'PipeWire:Interface:Metadata' and entry.get('props', {}).get('metadata.name') == 'default' for p in entry.get('metadata', []) if p.get('subject') == 0 and p.get('key') == 'default.audio.sink']
        assert len(defaults) == 1 and defaults[0]['type'] == 'Spa:String:JSON' and defaults[0]['value']['name'] == 'alcard-ci-null', defaults
    if mode == 'level':
        props = [p for p in node['info']['params']['Props'] if 'channelVolumes' in p and 'mute' in p]
        assert len(props) == 1
        level, mute = float(sys.argv[3]), sys.argv[4] == 'true'
        values = props[0]['channelVolumes']
        assert len(values) == 2 and all(math.isfinite(v) and v >= 0 and abs(v ** (1/3) - level) < .0002 for v in values), values
        assert props[0]['mute'] is mute, props[0]
elif mode in {'text', 'text-volume'}:
    root = pathlib.Path(os.environ['CI_FIXTURE_ROOT'])
    palette = tomllib.loads((root / 'rust/crates/alcard-settings-core/themes/classic.toml').read_text())['palette']
    colors = {tuple(bytes.fromhex(palette[key][1:])) for key in ('text', 'muted', 'accent', 'disabled', 'success', 'warning', 'error')}
    glyphs = {}
    for line in (root / 'fonts/alcard-pixel/source/AlcardPixel-Regular.hex').read_text().splitlines():
        if ':' in line:
            code, bits = line.split(':', 1)
            if len(bits) == 16: glyphs[chr(int(code, 16))] = bytes.fromhex(bits)
    image = Image.open(sys.argv[2]).convert('RGB'); assert image.size == (320, 170)
    pixels = bytes(pixel in colors for pixel in image.getdata())
    rows = [pixels[y * 320:(y + 1) * 320] for y in range(170)]
    for text in sys.argv[3:]:
        template = [bytes(bool(glyphs[ch][y] & (0x80 >> x)) for ch in text for x in range(8)) for y in range(8)]
        anchor = max(range(8), key=lambda row: sum(template[row])); found = False
        # Volume is the second inline Quick row (y=63..87). Restrict its
        # status/read-back so unavailable brightness cannot satisfy this check.
        for y in (range(63, 81) if mode == 'text-volume' else range(163)):
            start = 0
            while (x := rows[y + anchor].find(template[anchor], start)) != -1:
                if all(rows[y + line][x:x + len(template[line])] == template[line] for line in range(8)): found = True; break
                start = x + 1
            if found: break
        if not found: raise SystemExit(1)
elif mode == 'tree':
    parent = int(sys.argv[2]); entries = []
    for path in pathlib.Path('/proc').iterdir():
        if not path.name.isdecimal(): continue
        try:
            stat = (path / 'stat').read_text().rsplit(') ', 1)[1].split()
            if stat[0] != 'Z':
                invocation = os.fsdecode((path / 'cmdline').read_bytes().split(b'\0', 1)[0])
                entries.append(dict(pid=int(path.name), parent=int(stat[1]), start=stat[19], executable=os.readlink(path / 'exe'), invocation=invocation))
        except OSError: pass
    descendants = {parent}; changed = True
    while changed:
        previous = len(descendants)
        descendants.update(p['pid'] for p in entries if p['parent'] in descendants)
        changed = len(descendants) != previous
    rows = [p for p in entries if p['pid'] in descendants]
    for binary in ('pipewire', 'wireplumber', 'pipewire-pulse'):
        # Alpine installs pipewire-pulse as a symlink to pipewire. The real
        # executable alone cannot distinguish these two owned daemon roles.
        matches = [p for p in rows if p['invocation'] == '/usr/bin/' + binary]
        assert len(matches) == 1 and matches[0]['executable'] == os.path.realpath('/usr/bin/' + binary), rows
        matches[0]['role'] = binary
    pathlib.Path(sys.argv[3]).write_text(json.dumps(rows, indent=2) + '\n')
    print(next(p['pid'] for p in rows if p.get('role') == 'wireplumber'))
elif mode == 'stop-wireplumber':
    rows = json.loads(pathlib.Path(sys.argv[2]).read_text())
    targets = [p for p in rows if p.get('role') == 'wireplumber']
    assert len(targets) == 1
    target = targets[0]; path = pathlib.Path(f"/proc/{target['pid']}")
    stat = (path / 'stat').read_text().rsplit(') ', 1)[1].split()
    assert stat[0] != 'Z' and stat[19] == target['start'] and os.readlink(path / 'exe') == target['executable']
    os.kill(target['pid'], signal.SIGTERM)
elif mode == 'stopped':
    for p in json.loads(pathlib.Path(sys.argv[2]).read_text()):
        try: stat = pathlib.Path(f"/proc/{p['pid']}/stat").read_text().rsplit(') ', 1)[1].split()
        except FileNotFoundError: continue
        assert stat[0] == 'Z' or stat[19] != p['start'], f"Owned audio lifetime survived: {p['pid']}"
else: raise SystemExit('Unknown observation mode')
PY
observe() { python3 "$XDG_RUNTIME_DIR/observe.py" "$@"; }
wait_for() {
  local label=$1 end=$((SECONDS + 12)); shift
  current_step=$label
  printf 'Checking: %s\n' "$label"
  while ((SECONDS < end)); do
    [[ -z "$shell_pid" ]] || kill -0 "$shell_pid"
    if "$@"; then return 0; fi
    sleep .1
  done
  printf 'Timed out: %s\n' "$label" >&2
  return 1
}
dump() { timeout 4 /usr/bin/pw-dump -r pipewire-0 >"$XDG_RUNTIME_DIR/graph.json" 2>/dev/null; }
node_ready() { dump && observe node "$XDG_RUNTIME_DIR/graph.json" >"$XDG_RUNTIME_DIR/node-id" 2>/dev/null; }
default_ready() { dump && observe default "$XDG_RUNTIME_DIR/graph.json" 2>/dev/null; }
level_is() { dump && observe level "$XDG_RUNTIME_DIR/graph.json" "$1" "$2" 2>/dev/null; }
screen_has() { timeout 4 grim -o "$output" "$XDG_RUNTIME_DIR/observed.png" && observe text "$XDG_RUNTIME_DIR/observed.png" "$@"; }
volume_has() { timeout 4 grim -o "$output" "$XDG_RUNTIME_DIR/observed.png" && observe text-volume "$XDG_RUNTIME_DIR/observed.png" "$@"; }
owned_stopped() { observe stopped "$OUT/owned-audio-lifetimes.json" 2>/dev/null; }
keyboard_ready() {
  python3 - "$XDG_RUNTIME_DIR/keyboard-idle.log" <<'PY'
import pathlib, re, sys
log = pathlib.Path(sys.argv[1]).read_text()
keymap = re.search(r'zwp_virtual_keyboard_v1[^\n]*\.keymap\(', log)
raise SystemExit(0 if keymap and re.search(r'wl_callback[^\n]*\.done\(', log[keymap.end():]) else 1)
PY
}
physical_key() {
  local raw=$1 symbol=$2 slot placeholder
  local -a keys=(-s 100)
  for ((slot=1; slot<raw; slot++)); do printf -v placeholder 'U%04X' "$((0xE000 + slot))"; keys+=(-p "$placeholder"); done
  keys+=(-s 100 -P "$symbol" -s 30 -p "$symbol" -s 100)
  timeout 5 wtype "${keys[@]}"
  sleep .15
}
key() {
  # Production navigation uses evdev positions outside text editors. wtype's
  # temporary keymap does not give an arrow its physical evdev code by itself.
  case "$1" in
    Up) physical_key 103 Up ;;
    Down) physical_key 108 Down ;;
    Left) physical_key 105 Left ;;
    Right) physical_key 106 Right ;;
    Return) physical_key 28 Return ;;
    *) echo "Unsupported public navigation key: $1" >&2; return 1 ;;
  esac
}
capture() { timeout 4 grim -o "$output" "$OUT/$1.png"; cp "$XDG_RUNTIME_DIR/graph.json" "$OUT/$1-pw.json"; }
stamp="$(python3 -c 'import pathlib,sys; print(pathlib.Path("/proc/"+sys.argv[1]+"/stat").read_text().rsplit(") ",1)[1].split()[19])' "$$")"
/usr/libexec/alcard/audio-session "$$" "$stamp" >"$OUT/audio-session.log" 2>&1 & audio_pid=$!
wait_for 'private PipeWire null sink' node_ready
node_id="$(cat "$XDG_RUNTIME_DIR/node-id")"
wait_for 'private Pulse socket' test -S "$XDG_RUNTIME_DIR/pulse/native"
wait_for 'WirePlumber default metadata' default_ready
timeout 5 /usr/bin/wpctl set-default "$node_id"
timeout 5 /usr/bin/wpctl set-volume "$node_id" 0.50
timeout 5 /usr/bin/wpctl set-mute "$node_id" 0
wait_for 'default sink at 50 percent' level_is .50 false
observe tree "$audio_pid" "$OUT/owned-audio-lifetimes.json" >/dev/null
# A second supervisor is rejected by the real runtime lock; existing daemons
# remain the same lifetimes. Neither invocation can address another server.
/usr/libexec/alcard/audio-session "$$" "$stamp" >"$OUT/duplicate-owner.log" 2>&1 & duplicate=$!
duplicate_status=0
wait "$duplicate" || duplicate_status=$?
[[ $duplicate_status != 0 ]]
grep -q 'another Alcard audio session owns this runtime' "$OUT/duplicate-owner.log"
kill -0 "$audio_pid"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1 WLR_XWAYLAND=
unset DISPLAY WAYLAND_DISPLAY WAYLAND_SOCKET WAYLAND_DEBUG
labwc -C "$XDG_CONFIG_HOME/labwc" >"$OUT/labwc.log" 2>&1 & compositor=$!
for _ in $(seq 1 100); do
  for socket in "$XDG_RUNTIME_DIR"/wayland-*; do
    if [[ -S "$socket" ]]; then export WAYLAND_DISPLAY="$(basename -- "$socket")"; break 2; fi
  done
  kill -0 "$compositor"; sleep .1
done
[[ -n ${WAYLAND_DISPLAY:-} ]]
output="$(wlr-randr | awk 'NF && $0 !~ /^[[:space:]]/ { print $1; exit }')"
[[ $output == HEADLESS-* ]]
wlr-randr --output "$output" --custom-mode 320x170@60Hz --scale 1 --pos 0,0
ALCARD_POINTER_FIXTURE=1 timeout 5 "$CI_FIXTURE_TMP/park-pointer"
WAYLAND_DEBUG=1 wtype -s 180000 >/dev/null 2>"$XDG_RUNTIME_DIR/keyboard-idle.log" & keyboard=$!
wait_for 'acknowledged virtual keyboard keymap' keyboard_ready
/usr/bin/alcard-shell --locale en --theme classic >"$OUT/shell.log" 2>&1 & shell_pid=$!
wait_for 'installed Shell Home endpoint' timeout 2 /usr/bin/alcard-shell --home
wait_for 'mapped installed Home' screen_has ALCARD
physical_key 6 5
wait_for 'Quick settings' screen_has 'Quick settings'
key Down
wait_for 'read-back 50 percent in inline volume' volume_has Volume '50%'
capture 01-volume-50
key Right
wait_for 'Shell volume increase read-back' level_is .55 false
wait_for 'visible 55 percent' screen_has '55%'
capture 02-volume-55
key Left
wait_for 'Shell volume decrease read-back' level_is .50 false
wait_for 'visible 50 percent' screen_has '50%'
physical_key 6 5
wait_for 'Shell software mute' level_is .50 true
wait_for 'visible muted state' screen_has Muted
capture 03-muted
physical_key 6 5
wait_for 'Shell software unmute' level_is .50 false
wait_for 'visible unmuted state' screen_has Unmuted
timeout 5 /usr/bin/wpctl set-volume "$node_id" 1.10
physical_key 8 7
wait_for 'over-unity guard visible' screen_has 'Above 100%'
key Right
wait_for 'increase remains blocked above unity' level_is 1.10 false
key Left
wait_for 'decrease returns safely below unity' level_is .95 false
wait_for 'visible 95 percent' screen_has '95%'
capture 04-unity-guard
# Kill only the observed child of this exact private production supervisor.
# Its ordinary failure path must reap PipeWire and Pulse without auto-restart.
observe stop-wireplumber "$OUT/owned-audio-lifetimes.json"
wait_for 'all owned audio lifetimes stopped' owned_stopped
audio_status=0
wait "$audio_pid" || audio_status=$?
audio_pid=''
[[ $audio_status != 0 ]]
physical_key 8 7
wait_for 'failed audio becomes unavailable after explicit refresh' volume_has 'Device unavailable'
timeout 4 grim -o "$output" "$OUT/05-audio-unavailable.png"
sleep .3
observe stopped "$OUT/owned-audio-lifetimes.json"
python3 - "$OUT" <<'PY'
import json, pathlib, sys
out = pathlib.Path(sys.argv[1])
(out / 'checks.json').write_text(json.dumps({
 'status':'PASS', 'sink':'private software null sink, stereo',
 'verified':['production audio-session starts PipeWire, WirePlumber and Pulse', 'duplicate owner refused',
 'installed Shell volume 50 -> 55 -> 50 with complete channel readback', 'software mute/unmute without volume changes',
 'above-unity increase refused and decrease reaches 95 percent', 'WirePlumber loss stops all owned audio lifetimes',
 'Shell remains alive and explicit refresh reports unavailable'],
 'not_proven':['physical sound', 'hardware codec gain', 'device route restoration after real suspend'],
}, indent=2) + '\n')
PY
printf '%s\n' 'PASS: installed Shell and private production PipeWire stack, controls/readback and owned lifecycle.'
AUDIO
chmod 755 "$TMP/"{idle,audio}.sh

run_stage() {
  local name=$1 limit=$2 started=$SECONDS status=0 unix
  shift 2
  unix="$(date +%s)"
  printf 'Running installed runtime stage: %s\n' "$name"
  timeout --kill-after=8 "$limit" "$@" >"$OUT/$name.log" 2>&1 || status=$?
  printf '%s\t%s\t%s\t%s\n' "$name" "$unix" "$((SECONDS - started))" "$status" >>"$OUT/stages.tsv" || return 1
  if [[ $status != 0 ]]; then
    tail -80 "$OUT/$name.log" >&2 || true
    return "$status"
  fi
  tail -8 "$OUT/$name.log"
}
# The five private fixtures are independent. Collect all their results before
# failing this runtime gate, so one fixture defect does not require a fresh
# source/APK build merely to discover a second defect. Never turn a failure
# into PASS; the outer driver still stops before VM acceptance on any failure.
runtime_status=0
run_stage notifications 40 runuser -u "$TEST_USER" -- env \
  -u DBUS_SESSION_BUS_ADDRESS -u DBUS_SYSTEM_BUS_ADDRESS -u XDG_RUNTIME_DIR \
  -u XDG_CONFIG_HOME -u XDG_CONFIG_DIRS -u XDG_DATA_HOME -u XDG_DATA_DIRS \
  -u WAYLAND_DISPLAY -u WAYLAND_SOCKET -u DISPLAY \
  "$TMP/test-shell" notifications::tests::isolated_dbus_notification_lifecycle \
  --exact --ignored --test-threads=1 --nocapture || runtime_status=1
grep -Eq 'test result: ok\. 1 passed; 0 failed; 0 ignored;' "$OUT/notifications.log" || runtime_status=1
run_stage idle 250 runuser -u "$TEST_USER" -- dbus-run-session -- bash "$TMP/idle.sh" || runtime_status=1
run_stage audio 240 runuser -u "$TEST_USER" -- dbus-run-session -- bash "$TMP/audio.sh" || runtime_status=1
run_stage output-power 80 runuser -u "$TEST_USER" -- env \
  -u XDG_RUNTIME_DIR -u WAYLAND_DISPLAY -u WAYLAND_SOCKET -u DISPLAY \
  -u DBUS_SYSTEM_BUS_ADDRESS ALCARD_OUTPUT_POWER_TEST=1 \
  dbus-run-session -- python3 "$ROOT/tools/shell-output-power.py" \
  --helper "$TMP/output-power-fixture" --shell /usr/bin/alcard-shell \
  --out "$OUT/output-power" || runtime_status=1
export ALCARD_BATTERY_TEST_CONTAINER=1 ALCARD_SHELL_BIN=/usr/bin/alcard-shell ALCARD_LOCK_BIN=/usr/bin/alcard-lock
export ALCARD_BATTERY_EVIDENCE="$OUT/battery-sleep-backlight"
run_stage battery-sleep-backlight 480 bash "$ROOT/scripts/check-shell-battery.sh" || runtime_status=1
if [[ $runtime_status != 0 ]]; then
  printf '%s\n' 'FAIL: one or more independent runtime fixtures failed; inspect their separate logs.' >&2
  exit "$runtime_status"
fi
printf '%s\n' 'PASS: installed Alpine session software runtime. Real elogind, kernel sleep and physical devices remain separate acceptance.'
