#!/usr/bin/env bash
# Disposable CI container ONLY: real current-user PAM and real ext-session-lock.
# No host session/socket/device/PAM files are mounted into this container.
set -euo pipefail
[[ ${ALCARD_LOCK_TEST_CONTAINER:-} == 1 && -f /.dockerenv && $(id -u) == 0 ]] || {
  echo 'Refusing lock fixtures outside the disposable CI container' >&2; exit 1;
}
for binary in /test-lock /opt/alcard/alcard-lock /opt/alcard/alcard-shell; do [[ -x "$binary" ]] || exit 1; done
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends libpam0g libpam-modules libpam-modules-bin libpam-runtime libxkbcommon0 libwayland-client0 dbus-daemon dbus-bin libglib2.0-bin labwc wtype grim wlr-randr passwd util-linux python3
for tool in dbus-daemon gdbus timeout; do
  command -v "$tool" >/dev/null || { echo "Missing private-bus fixture tool: $tool" >&2; exit 1; }
done
install -m 644 /work/scripts/pam/alcard-lock.ubuntu /etc/pam.d/alcard-lock
for account in alcard-lock-test alcard-lock-other; do
  useradd -m -s /bin/bash "$account"
  secret="45678$(od -An -N18 -tx1 /dev/urandom | tr -d ' \n')"
  printf '%s:%s\n' "$account" "$secret" | chpasswd
  umask 077
  printf '%s' "$secret" >"/home/$account/.lock-test-secret"
  chown "$account:$account" "/home/$account/.lock-test-secret"
  unset secret
done
install -o alcard-lock-test -g alcard-lock-test -m 600 /home/alcard-lock-other/.lock-test-secret /home/alcard-lock-test/.lock-other-secret
umask 022
if [[ -n ${ALCARD_LOCK_SCREENSHOT_DIR:-} ]]; then
  [[ "$ALCARD_LOCK_SCREENSHOT_DIR" == /* && "$ALCARD_LOCK_SCREENSHOT_DIR" != / ]] || {
    echo 'Lock capture directory must be an explicit absolute path' >&2; exit 1;
  }
  install -d -o alcard-lock-test -g alcard-lock-test -m 755 "$ALCARD_LOCK_SCREENSHOT_DIR"
  {
    printf '%s\n' 'Alcard manual session lock: disposable Ubuntu native container, private labwc, real distribution PAM.'
    printf '%s\n' 'Two headless Pixman outputs, each 320x170 at scale1. No real devices or host sessions.'
    printf '%s\n' 'Main GUI/PAM runtime captures 01-07: Russian, Classic. Negative PAM cases 08-11 and production Shell/helper stages: English, Classic.'
    printf '%s\n' 'A fresh persistent idle virtual keyboard is acknowledged before every Shell start and retained through that process lifetime.'
    printf '%s\n' 'Client PNGs are the actual runtime source raster committed to opaque lock SHM after the real locked event; they are not compositor screencopy.'
    printf '%s\n' 'Only empty masked-input states are captured. Fixture secrets remain in disposable home directories and are never artifact files.'
    printf '%s\n' 'Shell launch staging uses an unchanged binary copy and a test-only sibling wrapper; wrapper observes only canonical public LOCKED/UNLOCKED/FAILED lines and preserves the real helper exit.'
    if [[ ${ALCARD_IDLE_TEST:-} == 1 ]]; then
      printf '%s\n' 'Opt-in idle-only case: real idle notifications, two output-power acknowledgements, PAM and restart. Shell stderr is filtered before storage to public idle/power messages and canonical lifecycle receipts; no keyboard trace is retained.'
    fi
    printf 'Architecture: '; uname -m
    printf 'labwc: '; labwc --version
    dpkg-query -W -f='${binary:Package} ${Version}\n' libpam0g libpam-modules libwlroots12t64 labwc
    sha256sum /opt/alcard/alcard-lock /opt/alcard/alcard-shell /test-lock
  } >"$ALCARD_LOCK_SCREENSHOT_DIR/manifest.txt"
  chown alcard-lock-test:alcard-lock-test "$ALCARD_LOCK_SCREENSHOT_DIR/manifest.txt"
fi
cat >/tmp/alcard-lock-test-session <<'SESSION'
#!/usr/bin/env bash
set -euo pipefail
export XDG_RUNTIME_DIR="$(mktemp -d)"
chmod 700 "$XDG_RUNTIME_DIR"
export XDG_CONFIG_HOME="$XDG_RUNTIME_DIR/config" XDG_DATA_HOME="$XDG_RUNTIME_DIR/data" XDG_DATA_DIRS="$XDG_RUNTIME_DIR/no-system-apps"
mkdir -p "$XDG_CONFIG_HOME/labwc" "$XDG_DATA_HOME" "$XDG_DATA_DIRS" "$XDG_RUNTIME_DIR/staged" "$XDG_RUNTIME_DIR/missing-helper"
cat >"$XDG_CONFIG_HOME/labwc/rc.xml" <<'RC'
<?xml version="1.0"?><labwc_config><focus><followMouse>no</followMouse></focus><keyboard><default/></keyboard></labwc_config>
RC
cp /opt/alcard/alcard-shell "$XDG_RUNTIME_DIR/staged/alcard-shell"
cp /opt/alcard/alcard-shell "$XDG_RUNTIME_DIR/missing-helper/alcard-shell"
cat >"$XDG_RUNTIME_DIR/staged/alcard-lock" <<'WRAPPER'
#!/usr/bin/env bash
set -euo pipefail
# The only observability shim: exact public lifecycle, no stderr or input.
# The producer PID is replaced by the unchanged production executable.
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
      if [[ ${ALCARD_IDLE_TEST:-} == 1 ]]; then
        # Same stderr stream as the Shell protocol trace. This write completes
        # before forwarding the real receipt, so ordering needs no clock guess.
        printf '\nALCARD_IDLE_LIFECYCLE %s\n' "$lifecycle" >&2
      fi
      printf '%s\n' "$lifecycle"
      ;;
    *) exit 97 ;;
  esac
done
WRAPPER
chmod 755 "$XDG_RUNTIME_DIR/staged/alcard-lock"
if [[ ${ALCARD_IDLE_TEST:-} == 1 ]]; then
  # Keep the actual production executable and its fixed sibling locker in the
  # same directory. Only this explicit case gets a pre-storage trace filter.
  mv "$XDG_RUNTIME_DIR/staged/alcard-shell" "$XDG_RUNTIME_DIR/staged/alcard-shell-real"
  cat >"$XDG_RUNTIME_DIR/idle-protocol-filter.py" <<'FILTER'
import re
import sys

protocol = re.compile(
    rb'^\[[0-9 .]+\](?:\[rs\])? (?:<-|->) '
    rb'(?:ext_idle_notifier_v1|ext_idle_notification_v1|'
    rb'zwlr_output_power_manager_v1|zwlr_output_power_v1)'
    rb'[@#][0-9]+\.[a-z_]+,?\s*\([^\r\n]*\)\s*$'
)
receipts = {
    b'ALCARD_IDLE_LIFECYCLE LOCKED',
    b'ALCARD_IDLE_LIFECYCLE UNLOCKED',
    b'ALCARD_IDLE_LIFECYCLE FAILED',
}
for line in sys.stdin.buffer:
    # Never write keyboard, prompt, identity, response or generic stderr lines.
    if line.strip() in receipts or protocol.fullmatch(line):
        sys.stdout.buffer.write(line)
        sys.stdout.buffer.flush()
FILTER
  cat >"$XDG_RUNTIME_DIR/staged/alcard-shell" <<'IDLE_SHELL'
#!/usr/bin/env bash
set -euo pipefail
exec "$XDG_RUNTIME_DIR/staged/alcard-shell-real" "$@" \
  2> >(python3 -u "$XDG_RUNTIME_DIR/idle-protocol-filter.py" >&2)
IDLE_SHELL
  chmod 755 "$XDG_RUNTIME_DIR/staged/alcard-shell"
fi
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=2 WLR_LIBINPUT_NO_DEVICES=1 XKB_DEFAULT_LAYOUT=us
export ALCARD_LOCK_TEST_HELPER=/opt/alcard/alcard-lock
unset DISPLAY WAYLAND_DISPLAY WAYLAND_DEBUG
compositor='' keyboard=''
cleanup() {
  local status=$?
  [[ -z "$keyboard" ]] || kill "$keyboard" 2>/dev/null || true
  [[ -z "$compositor" ]] || kill "$compositor" 2>/dev/null || true
  [[ -z "$keyboard" ]] || wait "$keyboard" 2>/dev/null || true
  [[ -z "$compositor" ]] || wait "$compositor" 2>/dev/null || true
  # Check before emitting any private runtime logs. Never print a leaked token.
  if ! python3 - "$HOME" "$XDG_RUNTIME_DIR" "${ALCARD_LOCK_SCREENSHOT_DIR:-}" <<'PY'
import pathlib, sys
home, runtime = map(pathlib.Path, sys.argv[1:3])
secrets = [p.read_bytes() for p in (home / '.lock-test-secret', home / '.lock-other-secret') if p.is_file()]
roots = [runtime]
if sys.argv[3]: roots.append(pathlib.Path(sys.argv[3]))
for root in roots:
    for path in root.rglob('*'):
        if path.is_file() and path.suffix in {'.log', '.txt', '.png', '.receipts', '.pid'}:
            data = path.read_bytes()
            if any(secret and secret in data for secret in secrets):
                if sys.argv[3]:
                    exported = pathlib.Path(sys.argv[3])
                    for artifact in exported.iterdir():
                        if artifact.is_file(): artifact.unlink()
                    (exported / 'capture-rejected.txt').write_text('Runtime output rejected: fixture token leaked; no output contents exported.\n')
                print('FAIL: fixture credential found in runtime output; contents suppressed', file=sys.stderr)
                raise SystemExit(1)
PY
  then status=1
  else
    if [[ ${ALCARD_IDLE_TEST:-} == 1 && -n ${ALCARD_LOCK_SCREENSHOT_DIR:-} ]]; then
      for public_log in idle-shell-protocol.log idle-restarted-protocol.log idle-foreign-protocol.log shell-lock.receipts shell-lock.identity.txt; do
        [[ ! -f "$XDG_RUNTIME_DIR/$public_log" ]] || \
          cp -- "$XDG_RUNTIME_DIR/$public_log" "$ALCARD_LOCK_SCREENSHOT_DIR/$public_log"
      done
    fi
    [[ ! -f "$XDG_RUNTIME_DIR/test.log" ]] || cat "$XDG_RUNTIME_DIR/test.log"
    if [[ $status != 0 ]]; then tail -60 "$XDG_RUNTIME_DIR/labwc.log" || true; fi
  fi
  rm -rf "$XDG_RUNTIME_DIR"
  exit "$status"
}
trap cleanup EXIT
labwc -C "$XDG_CONFIG_HOME/labwc" >"$XDG_RUNTIME_DIR/labwc.log" 2>&1 & compositor=$!
for _ in $(seq 1 100); do
  for socket in "$XDG_RUNTIME_DIR"/wayland-*; do
    if [[ -S "$socket" ]]; then export WAYLAND_DISPLAY="$(basename "$socket")"; break 2; fi
  done
  kill -0 "$compositor"; sleep .1
done
[[ -n ${WAYLAND_DISPLAY:-} ]]
mapfile -t outputs < <(wlr-randr | awk 'NF && $0 !~ /^[[:space:]]/ { print $1 }')
[[ ${#outputs[@]} == 2 ]]
for index in 0 1; do
  [[ "${outputs[$index]}" == HEADLESS-* ]] || exit 1
  wlr-randr --output "${outputs[$index]}" --custom-mode 320x170@60Hz --scale 1 --pos "$((index * 320)),0"
done
export ALCARD_LOCK_TEST_OUTPUT="${outputs[0]}"
wtype -p Ue000 -s 240000 & keyboard=$!
sleep .2
timeout --kill-after=5 210 /test-lock --ignored --exact --test-threads=1 --nocapture "$1" >"$XDG_RUNTIME_DIR/test.log" 2>&1
grep -Eq 'test result: ok\. 1 passed; 0 failed; 0 ignored;' "$XDG_RUNTIME_DIR/test.log"
SESSION
chmod 755 /tmp/alcard-lock-test-session
if [[ ${ALCARD_IDLE_TEST:-} == 1 ]]; then
  # Deliberate separate mode: existing CI never adds this work automatically,
  # and an idle acceptance request does not repeat the three manual-lock runs.
  runuser -u alcard-lock-test -- dbus-run-session -- bash /tmp/alcard-lock-test-session integration::real_idle_lock_and_output_power
  echo 'PASS: opt-in real idle lock, two-output power acknowledgements, wake without unlock, restart recovery and current-user PAM'
  exit 0
fi
runuser -u alcard-lock-test -- dbus-run-session -- bash /tmp/alcard-lock-test-session integration::real_session_lock_pam_and_fail_closed
runuser -u alcard-lock-test -- dbus-run-session -- bash /tmp/alcard-lock-test-session integration::real_lock_pam_worker_loss_stays_locked
# A fresh isolated compositor under the same UID with a now-expired account
# verifies pam_acct_mgmt after the valid password. setpriv changes only this
# disposable process's credentials; runuser's own PAM would reject the fixture.
chage --expiredate 1 alcard-lock-test
uid="$(id -u alcard-lock-test)" gid="$(id -g alcard-lock-test)"
HOME=/home/alcard-lock-test USER=alcard-lock-test LOGNAME=alcard-lock-test \
  setpriv --reuid "$uid" --regid "$gid" --init-groups -- \
  dbus-run-session -- bash /tmp/alcard-lock-test-session integration::real_lock_denies_expired_account
echo 'PASS: real session lock/current-UID PAM, input isolation, cancel, duplicate, crash, recovery, helper loss and expired-account rejection in disposable native container'
