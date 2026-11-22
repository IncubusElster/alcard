#!/usr/bin/env bash
# Disposable CI container ONLY. Does not change PAM configuration or root's
# password. The two test actions authorize no real system operation.
set -euo pipefail
[[ ${ALCARD_AUTH_TEST_CONTAINER:-} == 1 && -f /.dockerenv && $(id -u) == 0 ]] || {
  echo 'Refusing to install fixtures outside the disposable CI container' >&2; exit 1;
}
[[ -x /test-agent ]] || exit 1
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends polkitd pkexec dbus-daemon dbus-bin libglib2.0-0t64 libpolkit-agent-1-0 libxkbcommon0 libwayland-client0 labwc wtype grim wlr-randr passwd util-linux
mkdir -p /run/dbus /etc/polkit-1/rules.d /usr/share/polkit-1/actions
dbus-uuidgen --ensure=/etc/machine-id
for user in alcard-tester alcard-one alcard-two; do
  useradd -m -s /bin/sh "$user"
  secret="$(od -An -N18 -tx1 /dev/urandom | tr -d ' \n')"
  printf '%s:%s\n' "$user" "$secret" | chpasswd
  umask 077
  printf '%s' "$secret" >"/home/$user/.auth-test-secret"
  chown "$user:$user" "/home/$user/.auth-test-secret"
  unset secret
done
# Driver can read only the disposable second identity's fixture credential.
install -o alcard-tester -g alcard-tester -m 600 /home/alcard-two/.auth-test-secret /home/alcard-tester/.auth-test-second
umask 022
if [[ -n ${ALCARD_AUTH_SCREENSHOT_DIR:-} ]]; then
  [[ "$ALCARD_AUTH_SCREENSHOT_DIR" == /* && "$ALCARD_AUTH_SCREENSHOT_DIR" != / ]] || {
    echo 'Authentication screenshot directory must be an explicit absolute path' >&2; exit 1;
  }
  # This optional mount receives only the empty-form PNG. Fixture credentials
  # remain in the disposable home directories, outside the exported directory.
  install -d -o alcard-tester -g alcard-tester -m 755 "$ALCARD_AUTH_SCREENSHOT_DIR"
fi
cat >/usr/share/polkit-1/actions/org.alcard.test.policy <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE policyconfig PUBLIC "-//freedesktop//DTD PolicyKit Policy Configuration 1.0//EN" "http://www.freedesktop.org/standards/PolicyKit/1/policyconfig.dtd">
<policyconfig>
<action id="org.alcard.test.self"><description>Alcard isolated test</description><message>Authenticate a no-operation CI test</message><defaults><allow_any>auth_self</allow_any><allow_inactive>auth_self</allow_inactive><allow_active>auth_self</allow_active></defaults></action>
<action id="org.alcard.test.admin"><description>Alcard identity test</description><message>Select an identity for a no-operation CI test</message><defaults><allow_any>auth_admin</allow_any><allow_inactive>auth_admin</allow_inactive><allow_active>auth_admin</allow_active></defaults></action>
</policyconfig>
EOF
cat >/etc/polkit-1/rules.d/10-alcard-test.rules <<'EOF'
polkit.addAdminRule(function(action, subject) {
    if (action.id == 'org.alcard.test.admin') return ['unix-user:alcard-one', 'unix-user:alcard-two'];
});
EOF
dbus-daemon --system --fork
POLKITD=''
for path in /usr/lib/polkit-1/polkitd /usr/libexec/polkitd; do [[ ! -x "$path" ]] || POLKITD="$path"; done
[[ -n "$POLKITD" ]]
"$POLKITD" --no-debug >/tmp/polkitd.log 2>&1 &
cat >/tmp/alcard-auth-test-session <<'EOF'
#!/bin/bash
set -euo pipefail
export XDG_RUNTIME_DIR="$(mktemp -d)"
chmod 700 "$XDG_RUNTIME_DIR"
mkdir "$XDG_RUNTIME_DIR/config"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=1 WLR_LIBINPUT_NO_DEVICES=1
unset DISPLAY WAYLAND_DISPLAY WAYLAND_DEBUG
compositor='' keyboard=''
cleanup() {
  local status=$?
  [[ -z "$keyboard" ]] || kill "$keyboard" 2>/dev/null || true
  [[ -z "$compositor" ]] || kill "$compositor" 2>/dev/null || true
  if [[ $status != 0 ]]; then tail -80 "$XDG_RUNTIME_DIR/labwc.log" || true; fi
  rm -rf "$XDG_RUNTIME_DIR"
  exit "$status"
}
trap cleanup EXIT
labwc -C "$XDG_RUNTIME_DIR/config" >"$XDG_RUNTIME_DIR/labwc.log" 2>&1 & compositor=$!
for _ in $(seq 1 100); do
  for socket in "$XDG_RUNTIME_DIR"/wayland-*; do
    if [[ -S "$socket" ]]; then export WAYLAND_DISPLAY="$(basename "$socket")"; break 2; fi
  done
  kill -0 "$compositor"; sleep .1
done
[[ -n ${WAYLAND_DISPLAY:-} ]]
if [[ -n ${ALCARD_AUTH_SCREENSHOT_DIR:-} ]]; then
  auth_output="$(wlr-randr | awk 'NF && $0 !~ /^[[:space:]]/ { print $1; exit }')"
  [[ "$auth_output" == HEADLESS-* ]] || { echo 'Expected headless auth output' >&2; exit 1; }
  wlr-randr --output "$auth_output" --custom-mode 320x170@60Hz --scale 1 --pos 0,0
fi
wtype -s 120000 & keyboard=$!
sleep .2
timeout --kill-after=5 150 /test-agent --ignored --test-threads=1 --nocapture integration::real_polkit_pam_graphical_roundtrips
EOF
chmod 755 /tmp/alcard-auth-test-session
if ! runuser -u alcard-tester -- dbus-run-session -- bash /tmp/alcard-auth-test-session; then
  tail -120 /tmp/polkitd.log || true
  exit 1
fi
echo 'PASS: isolated real polkit/PAM graphical authentication; system PAM configuration unchanged'

