#!/bin/sh
# Disposable native Alpine CI ONLY. No host sessions, credentials or device mounts.
# Invoke with `sh`: a stock alpine image does not contain bash yet.
set -eu
[ "${ALCARD_SESSION_TEST_CONTAINER:-}" = 1 ] && [ -f /.dockerenv ] && \
    [ "$(id -u)" = 0 ] && [ "$(apk --print-arch)" = aarch64 ] || {
    echo 'Refusing session package tests outside disposable native Alpine aarch64 CI' >&2
    exit 1
}
[ -d /work/scripts ] && [ ! -L /work/.alcard-local ] || exit 1
case "$(cat /etc/alpine-release)" in 3.24.*) ;; *) echo 'Alpine 3.24 is required' >&2; exit 1 ;; esac
EVIDENCE=/work/.alcard-local/ci-packages
SOURCE=/home/alcard-build/alcard
[ ! -L "$EVIDENCE" ] || exit 1
mkdir -p "$EVIDENCE"
collect() {
    result=$?
    trap - EXIT
    if [ -d "$SOURCE/.alcard-local/packages/alcard/aarch64" ]; then
        mkdir -p "$EVIDENCE/packages"
        cp -R "$SOURCE/.alcard-local/packages/alcard/aarch64" "$EVIDENCE/packages/"
    fi
    # Public signing keys are useful to verify these CI-only packages. Private
    # keys and all build-user home/cache files remain inside this container.
    for key in /home/alcard-build/.abuild/*.rsa.pub; do
        [ ! -f "$key" ] || cp "$key" "$EVIDENCE/"
    done
    printf 'exit_status=%s\n' "$result" > "$EVIDENCE/status.txt"
    if [ -d "$EVIDENCE/packages/aarch64" ]; then
        (cd "$EVIDENCE" && sha256sum packages/aarch64/*.apk) > "$EVIDENCE/SHA256SUMS"
    fi
    chmod -R a+rX "$EVIDENCE"
    exit "$result"
}
trap collect EXIT

acceptance_dependencies=''
if [ "${ALCARD_CI_ACCEPTANCE:-}" = 1 ]; then
    # Opt-in sequential acceptance driver; the standalone package gate keeps its scope.
    acceptance_dependencies='rustfmt rust-clippy glib py3-gobject3 flock runuser coreutils'
fi
# These optional package names are a fixed list above, not caller input.
# shellcheck disable=SC2086
apk add --no-cache alpine-sdk cargo rust pkgconf libxkbcommon-dev wayland-dev \
    glib-dev polkit-elogind-dev linux-pam-dev python3 py3-fonttools py3-pillow \
    kbd bash dbus labwc foot grim wlr-randr wtype util-linux $acceptance_dependencies
python3 /work/tools/test-session-client.py --shell /bin/ash > "$EVIDENCE/supervisor.log" 2>&1 || {
    cat "$EVIDENCE/supervisor.log"
    exit 1
}
cat "$EVIDENCE/supervisor.log"
python3 /work/tools/test-audio-session.py --shell /bin/ash > "$EVIDENCE/audio-supervisor.log" 2>&1 || {
    cat "$EVIDENCE/audio-supervisor.log"
    exit 1
}
cat "$EVIDENCE/audio-supervisor.log"
python3 /work/tools/test-tty1-dispatch.py --shell /bin/ash > "$EVIDENCE/tty1-dispatch.log" 2>&1 || {
    cat "$EVIDENCE/tty1-dispatch.log"
    exit 1
}
cat "$EVIDENCE/tty1-dispatch.log"
python3 - "$(/usr/bin/rustc --version)" <<'VERSION'
import re
import sys
match = re.match(r"rustc (\d+)\.(\d+)\.(\d+) ", sys.argv[1])
if not match or tuple(map(int, match.groups())) < (1, 96, 1):
    raise SystemExit("Alpine Rust 1.96.1 or newer is required")
VERSION
{
    printf '%s\n' 'Alcard installed-session APK gate: disposable Alpine 3.24 aarch64.'
    printf '%s\n' 'Packages use an ephemeral CI signing key, not a release/repository key.'
    printf '%s\n' 'No password, PAM bypass, power action or authenticated login fixture is used.'
    uname -m
    cat /etc/alpine-release
    /usr/bin/rustc -vV
    /usr/bin/cargo --version
    apk info -v rust cargo linux-pam polkit-elogind-libs labwc
} > "$EVIDENCE/manifest.txt"

adduser -D -s /bin/sh alcard-build
addgroup alcard-build abuild
mkdir -p "$SOURCE"
tar -C /work --exclude=.git --exclude=.alcard-local --exclude=target \
    --exclude=__pycache__ -cf - . | tar -C "$SOURCE" -xf -
chown -R alcard-build:alcard-build /home/alcard-build
if [ "${ALCARD_CI_ACCEPTANCE:-}" = 1 ]; then
    sh "$SOURCE/scripts/ci/container.sh" prepare
    # Source failure stops before signing/building a fresh package set.
    timeout -k 20 900 su -s /bin/sh alcard-build \
        -c 'sh /home/alcard-build/alcard/scripts/ci/container.sh source'
    export CARGO_NET_OFFLINE=true
fi
su -s /bin/sh alcard-build -c 'abuild-keygen -a -n -q'
for key in /home/alcard-build/.abuild/*.rsa.pub; do
    [ -f "$key" ] || { echo 'No ephemeral public signing key' >&2; exit 1; }
    install -m 644 "$key" /etc/apk/keys/
done
cat > /tmp/alcard-build-session-packages <<'BUILD'
#!/bin/sh
set -eu
cd /home/alcard-build/alcard
bash scripts/build-keyboard-zero.sh build
bash scripts/build-fonts-console.sh build
bash scripts/build-wayland-apk.sh build fonts-gui
bash scripts/build-wayland-apk.sh build desktop
bash scripts/build-hello.sh build
bash scripts/build-shell-apk.sh build all
BUILD
chmod 755 /tmp/alcard-build-session-packages
if ! su -s /bin/sh alcard-build -c /tmp/alcard-build-session-packages > "$EVIDENCE/build.log" 2>&1; then
    tail -160 "$EVIDENCE/build.log"
    exit 1
fi
tail -80 "$EVIDENCE/build.log"
if ! ALCARD_SESSION_CONTAINER_ROOT=1 bash "$SOURCE/scripts/verify-session-install.sh" install \
    > "$EVIDENCE/install.log" 2>&1; then
    cat "$EVIDENCE/install.log"
    exit 1
fi
cat "$EVIDENCE/install.log"
python3 "$SOURCE/tools/verify-production-build.py" installed \
    "$SOURCE/.alcard-local/session-install/rootfs" "$SOURCE/.alcard-local/cargo-target"

# Install the same signed artifacts into this disposable container for a real
# Wayland smoke. The preceding rootfs verification still tests a clean solver.
session_pins="$(python3 "$SOURCE/tools/package_metadata.py" pin --group session-roots)"
# Metadata contains validated APK tokens; split its one-token-per-line output.
# shellcheck disable=SC2086
apk --repositories-file "$SOURCE/.alcard-local/session-install/repositories" add $session_pins
# A successful Shell build is not proof that its separate radio helper was
# staged, signed and installed. Tie both binaries and every radio asset to
# this build, the exact package archive, the clean solver root and runtime.
python3 - "$SOURCE" "$EVIDENCE/shell-install-hashes.json" <<'PY'
import hashlib
import json
from pathlib import Path
import stat
import subprocess
import sys
import tarfile

source, evidence = map(Path, sys.argv[1:])
sys.path.insert(0, str(source / 'tools'))
from package_metadata import Catalog
catalog = Catalog(source)
release = source / '.alcard-local/cargo-target/packages-aarch64-musl/release'
rootfs = source / '.alcard-local/session-install/rootfs'
files = {
    '/usr/bin/alcard-shell': (release / 'alcard-shell', 'alcard-shell', 0o755),
    '/usr/libexec/alcard/alcard-radio-helper': (release / 'alcard-radio-helper', 'alcard-radio-helper', 0o755),
    '/usr/share/polkit-1/actions/org.alcard.radio.policy': (source / 'scripts/radios/org.alcard.radio.policy', 'org.alcard.radio.policy', 0o644),
    '/etc/dbus-1/system-services/org.bluez.service': (source / 'scripts/radios/org.bluez.service', 'org.bluez.service', 0o644),
    '/etc/alcard/bluetooth.conf': (source / 'scripts/radios/bluetooth.conf', 'bluetooth.conf', 0o644),
}
receipts = {}
for installed, (built, staged, mode) in files.items():
    package = 'alcard-hello' if installed.startswith('/etc/') else 'alcard-shell'
    metadata = catalog[package]
    version = metadata.version
    archive_path = source / '.alcard-local/work/aports/alcard' / package / f'{package}-{metadata.pkgver}.tar.gz'
    with tarfile.open(archive_path, 'r:gz') as archive:
        member = archive.getmember(f'{package}-{metadata.pkgver}/' + staged)
        assert member.isfile() and member.mode == mode and member.uid == member.gid == 0, staged
        content = archive.extractfile(member)
        assert content is not None, staged
        hashes = {'staged': hashlib.sha256(content.read()).hexdigest()}
        for label, path in (('built', built), ('clean_rootfs', rootfs / installed.lstrip('/')),
                            ('installed_container', Path(installed))):
            info = path.lstat()
            assert stat.S_ISREG(info.st_mode), (installed, label, 'not a regular file')
            if label != 'built':
                assert info.st_uid == info.st_gid == 0 and stat.S_IMODE(info.st_mode) == mode, (installed, label)
            hashes[label] = hashlib.sha256(path.read_bytes()).hexdigest()
        assert len(set(hashes.values())) == 1, (installed, hashes)
        owner = subprocess.check_output(['apk', 'info', '--who-owns', installed], text=True).strip()
        assert owner.endswith(f' is owned by {package}-{version}'), owner
        receipts[installed] = {'sha256': hashes, 'mode': f'{mode:04o}', 'package': f'{package}-{version}'}
        print(f'PASS: built/staged/clean-rootfs/runtime hash {installed} {hashes["built"]}')
evidence.write_text(json.dumps({'schema': 1, 'status': 'PASS',
    'scope': 'Fresh Shell and radio helper/assets from release inputs through signed APK installation',
    'files': receipts}, indent=2) + '\n')
PY
adduser -D -s /bin/bash alcard-session-test
test_uid=$(id -u alcard-session-test)
install -d -o alcard-session-test -g alcard-session-test -m 700 "/run/user/$test_uid"
install -d -o alcard-session-test -g alcard-session-test -m 755 "$EVIDENCE/runtime"
# This headless container has no real PAM login. Preserve installed Shell,
# profile and supervision checks with an explicitly Shell-only CI copy; the
# unchanged complete launcher is exercised by the separate real-VT VM gate.
# Omit the authentication worker entirely, rather than inventing an identity
# or replacing authentication with a successful dummy.
python3 - <<'PY'
from pathlib import Path
source = Path('/usr/libexec/alcard/session-client').read_text()
block = '''supervise /usr/bin/alcard-polkit-agent &
auth_worker=$!
auth_worker_stamp=$(process_stamp "$auth_worker")'''
assert source.count(block) == 1, 'Production supervision contract changed'
fixture = source.replace(block, '# CI headless layout fixture: no authentication worker.', 1)
path = Path('/tmp/alcard-installed-shell-only-client')
path.write_text(fixture)
path.chmod(0o755)
PY
cat > /tmp/alcard-installed-session-smoke <<'SMOKE'
#!/bin/bash
set -euo pipefail
OUT=/work/.alcard-local/ci-packages/runtime
export XDG_RUNTIME_DIR="/run/user/$(id -u)"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1
export XKB_DEFAULT_LAYOUT=us LANG=C.UTF-8
unset WAYLAND_DISPLAY WAYLAND_SOCKET DISPLAY WAYLAND_DEBUG DBUS_SESSION_BUS_ADDRESS DBUS_SYSTEM_BUS_ADDRESS XDG_SESSION_ID
login_pid='' terminal_pid='' compositor_pid=''
cleanup() {
    result=$?
    trap - EXIT
    for pid in "$terminal_pid" "$compositor_pid" "$login_pid"; do
        [[ -z "$pid" ]] || kill -TERM "$pid" 2>/dev/null || :
    done
    if [[ $result -ne 0 ]]; then tail -120 "$OUT/session.log" 2>/dev/null || :; fi
    exit "$result"
}
trap cleanup EXIT
find_program() {
    python3 - "$1" <<'PY'
import os,pathlib,sys
for p in pathlib.Path('/proc').iterdir():
    if not p.name.isdigit(): continue
    try:
        if p.stat().st_uid == os.getuid() and os.readlink(p/'exe') == sys.argv[1]:
            print(p.name)
            break
    except (FileNotFoundError,PermissionError): pass
PY
}
capture_shell() {
    local output="$1"
    timeout 5 grim "$output" || return 1
    python3 - "$output" <<'PY'
from PIL import Image
import sys
im=Image.open(sys.argv[1]).convert('RGB')
assert im.size == (320,170), f'Unexpected output {im.size}'
pixels=list(im.getdata())
assert pixels.count((0x10,0x1d,0x30)) > 1000, 'Installed Shell background is not mapped'
assert pixels.count((0x75,0xdc,0xeb)) > 20, 'Installed Shell text/accent is missing'
PY
}
wait_shell_frame() {
    local output="$1"
    for _ in $(seq 1 50); do
        if capture_shell "$output" >/dev/null 2>&1; then return 0; fi
        sleep 0.1
    done
    capture_shell "$output"
}
check_foot_config_diagnostics() {
    # Foot can parse deprecated options successfully, then print a diagnostic
    # for every palette entry inside the terminal itself. Reject config
    # diagnostics specifically; controlled terminal shutdown can report HUP.
    if grep -Ei '(^|[[:space:]])(warn|warning|err|error):[[:space:]]+config\.c:' "$@"; then
        echo 'Installed Foot profile emitted configuration warnings/errors' >&2
        return 1
    fi
}
if ! timeout 5 foot --check-config --config=/etc/alcard/foot.ini > "$OUT/foot-config.log" 2>&1; then
    cat "$OUT/foot-config.log" >&2
    exit 1
fi
check_foot_config_diagnostics "$OUT/foot-config.log"
# Without a PAM login, the unchanged production entry must end promptly. Its
# orderly return to greetd may be exit zero; timeout or surviving clients fail.
rejected=0
timeout -k 3 20 /usr/libexec/alcard/shell-session > "$OUT/rejected-login.log" 2>&1 || rejected=$?
[[ "$rejected" != 124 && "$rejected" != 137 ]]
grep -F 'login context lost; returning to greetd' "$OUT/rejected-login.log"
for program in /usr/bin/alcard-shell /usr/bin/alcard-polkit-agent /usr/bin/labwc; do
    [[ -z "$(find_program "$program")" ]]
done
export XDG_SESSION_TYPE=wayland XDG_SESSION_DESKTOP=alcard-shell XDG_CURRENT_DESKTOP=Alcard:labwc
export LIBSEAT_BACKEND=logind ALCARD_FOOT_CONFIG=/etc/alcard/foot.ini
dbus-run-session -- /usr/bin/labwc --config-dir /etc/alcard/labwc-shell \
    --session '/bin/setpriv --pdeathsig TERM /tmp/alcard-installed-shell-only-client' \
    > "$OUT/session.log" 2>&1 & login_pid=$!
for _ in $(seq 1 150); do
    for socket in "$XDG_RUNTIME_DIR"/wayland-*; do
        if [[ -S "$socket" ]]; then export WAYLAND_DISPLAY="${socket##*/}"; break 2; fi
    done
    kill -0 "$login_pid"
    sleep 0.1
done
[[ -n "${WAYLAND_DISPLAY:-}" ]]
output="$(timeout 5 wlr-randr | python3 -c 'import sys; print(next(l.split()[0] for l in sys.stdin if l and not l[0].isspace()))')"
[[ "$output" == HEADLESS-* ]]
timeout 5 wlr-randr --output "$output" --custom-mode 320x170@60Hz --scale 1 --pos 0,0
timeout 5 wlr-randr > "$OUT/output.txt"
# Supply a private virtual keyboard to the actual installed compositor. This
# sends only a modifier, not an application action or a credential.
timeout 5 wtype -k Shift_L
shell_pid=''
for _ in $(seq 1 100); do
    candidate="$(find_program /usr/bin/alcard-shell)"
    if [[ -n "$candidate" ]] && timeout 3 /usr/bin/alcard-shell --home; then
        shell_pid="$candidate"
        break
    fi
    sleep 0.1
done
[[ -n "$shell_pid" ]]
compositor_pid="$(find_program /usr/bin/labwc)"
[[ -n "$compositor_pid" ]]
wait_shell_frame "$OUT/01-installed-home.png"
foot --config=/etc/alcard/foot.ini --app-id=alcard-session-smoke --fullscreen \
    -- sh -c 'sleep 40' > "$OUT/foot.log" 2>&1 & terminal_pid=$!
sleep 0.5
check_foot_config_diagnostics "$OUT/foot.log"
timeout 5 grim "$OUT/02-foot-before-home.png"
python3 - "$OUT/02-foot-before-home.png" <<'PY'
from PIL import Image
import sys
im=Image.open(sys.argv[1]).convert('RGB')
assert list(im.getdata()).count((0x10,0x17,0x20)) > 1000, 'Foot did not take the display'
PY
# Exercise the installed rc.xml release binding itself. The initial readiness
# and supervised restart above/below still exercise the direct Home IPC client.
timeout 5 wtype -k Super_L
wait_shell_frame "$OUT/03-home-key.png"

# Kill only this test user's exact installed Shell child. The Shell-only copy
# of production supervision must preserve the compositor and its private bus.
old_shell="$shell_pid"
kill -TERM "$old_shell"
shell_pid=''
for _ in $(seq 1 150); do
    candidate="$(find_program /usr/bin/alcard-shell)"
    if [[ -n "$candidate" && "$candidate" != "$old_shell" ]] && timeout 3 /usr/bin/alcard-shell --home; then
        shell_pid="$candidate"
        break
    fi
    kill -0 "$compositor_pid"
    sleep 0.1
done
[[ -n "$shell_pid" ]]
wait_shell_frame "$OUT/04-supervised-restart.png"
kill -TERM "$terminal_pid"
wait "$terminal_pid" 2>/dev/null || :
terminal_pid=''
python3 - "$OUT/owned-processes.json" <<'PY'
import json,os,pathlib,sys
owned=[]
for p in pathlib.Path('/proc').iterdir():
    if not p.name.isdigit(): continue
    try:
        uid=int(next(l for l in (p/'status').read_text().splitlines() if l.startswith('Uid:')).split()[1])
        if uid != os.getuid() or int(p.name) in (os.getpid(),os.getppid()): continue
        # Non-dumpable authentication clients can hide /proc/PID/exe even
        # from their same-UID observer; keep their identity/start regardless.
        try: exe=os.readlink(p/'exe')
        except PermissionError: exe='non-dumpable owned process'
        owned.append({'pid':int(p.name),'start':(p/'stat').read_text().rsplit(')',1)[1].split()[19], 'exe':exe})
    except (FileNotFoundError,PermissionError): pass
assert any(x['exe']=='/usr/bin/alcard-shell' for x in owned)
pathlib.Path(sys.argv[1]).write_text(json.dumps(owned,indent=2)+'\n')
PY
kill -TERM "$compositor_pid"
for _ in $(seq 1 100); do
    kill -0 "$login_pid" 2>/dev/null || break
    sleep 0.1
done
! kill -0 "$login_pid" 2>/dev/null
wait "$login_pid" 2>/dev/null || :
login_pid='' compositor_pid=''
python3 - "$OUT/owned-processes.json" <<'PY'
import json,pathlib,sys,time
expected=json.loads(pathlib.Path(sys.argv[1]).read_text())
for _ in range(100):
    alive=[]
    for item in expected:
        try:
            value=pathlib.Path(f"/proc/{item['pid']}/stat").read_text().rsplit(')',1)[1].split()
            if value[19]==item['start'] and value[0]!='Z': alive.append(item)
        except FileNotFoundError: pass
    if not alive: break
    time.sleep(.1)
assert not alive, f'Session left live owned processes: {alive}'
PY
if grep -Ei 'panicked at|protocol error|SHM buffer length mismatch' "$OUT/session.log"; then exit 1; fi
{
    printf '%s\n' 'PASS: unchanged production shell-session rejected a missing PAM login and ended promptly.'
    printf '%s\n' 'PASS: installed Shell and rc.xml mapped at 320x170 using an explicit CI Shell-only supervision fixture.'
    printf '%s\n' 'PASS: the installed Foot profile parsed and started without configuration warnings.'
    printf '%s\n' 'PASS: installed Foot took focus; the installed Super_L release binding invoked Home IPC and restored Shell.'
    printf 'PASS: supervisor restarted Shell %s -> %s without restarting labwc.\n' "$old_shell" "$shell_pid"
    printf '%s\n' 'PASS: compositor exit left no live recorded session-owned processes.'
    printf '%s\n' 'This layout fixture omits the authentication worker; no fake subject, PAM bypass or dummy authorization was used.'
    printf '%s\n' 'Full unchanged production login, supervision and authentication require the separate real-VT VM gate.'
} > "$OUT/manifest.txt"
cat "$OUT/manifest.txt"
SMOKE
chmod 755 /tmp/alcard-installed-session-smoke
timeout -k 10 120 su -s /bin/bash alcard-session-test -c /tmp/alcard-installed-session-smoke
# Also catch any child created after the smoke's recorded snapshot. This UID
# exists solely for the disposable installed-session test, never a host user.
python3 - "$test_uid" <<'PY'
import pathlib,sys,time
uid=int(sys.argv[1])
for _ in range(100):
    alive=[]
    for p in pathlib.Path('/proc').iterdir():
        if not p.name.isdigit(): continue
        try:
            lines=(p/'status').read_text().splitlines()
            actual=int(next(l for l in lines if l.startswith('Uid:')).split()[1])
            state=next(l for l in lines if l.startswith('State:')).split()[1]
            if actual==uid and state!='Z': alive.append(p.name)
        except FileNotFoundError: pass
    if not alive: break
    time.sleep(.1)
assert not alive, f'Installed session left live test-user processes: {alive}'
PY
printf '%s\n' 'PASS: nine signed runtime APKs, clean rootfs installation and installed-session smoke.' >> "$EVIDENCE/manifest.txt"
