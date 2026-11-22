#!/bin/sh
# The same just-built signed APKs; private observer changes only in this guest.
set -eu
[ "${ALCARD_LOGIN_VM_BUILD:-}" = 1 ] && [ -f /.dockerenv ] || exit 1
sh /work/scripts/build-login-vm.sh
fixture=/work/scripts/ci/hello
sysroot=/tmp/alcard-login-source/.alcard-local/session-install/rootfs
out=/work/.alcard-local/ci-login
install -m 755 "$sysroot/usr/libexec/alcard-ci/observer.py" \
    "$sysroot/usr/libexec/alcard-ci/observer-login.py"
install -m 755 "$fixture/observer.py" "$sysroot/usr/libexec/alcard-ci/observer.py"
install -m 755 "$fixture/measure-idle-resources.py" \
    "$sysroot/usr/libexec/alcard-ci/measure-idle-resources.py"
install -m 755 /work/scripts/ci/settings/observer.py \
    "$sysroot/usr/libexec/alcard-ci/settings-observer.py"
python3 -m py_compile "$sysroot/usr/libexec/alcard-ci/observer.py" \
    "$sysroot/usr/libexec/alcard-ci/observer-login.py" \
    "$sysroot/usr/libexec/alcard-ci/settings-observer.py" \
    "$sysroot/usr/libexec/alcard-ci/measure-idle-resources.py"
# Only this persistent acceptance guest gets an isolated virtual Ethernet
# device. Do not let NM create a profile before Settings' Device.Connect test.
printf '%s\n' virtio_net >> "$sysroot/etc/modules"
install -d -m 755 "$sysroot/etc/NetworkManager/conf.d"
printf '%s\n' '[main]' 'no-auto-default=*' \
    > "$sysroot/etc/NetworkManager/conf.d/90-alcard-ci-settings.conf"
chroot "$sysroot" /bin/sh -eu -c \
    'set -- /lib/modules/*; [ "$#" = 1 ]; exec modinfo -k "${1##*/}" virtio_net' > /dev/null
# The earlier login VMs have exited. This new writable disk belongs only to the
# Hello scenario and persists across its reboot; no private disk is exported.
mkfs.ext4 -F -L ALCARDHELLO -d "$sysroot" "$out/private/rootfs.ext4" \
    > "$out/evidence/ext4-hello.log" 2>&1
printf '%s\n' \
    'Interactive Hello uses the same current signed APKs as the login scenarios.' \
    'Private observers, PTY Settings driver and resource sampler are added; Hello provisions the account.' \
    'Settings uses real NetworkManager over isolated QEMU usernet Ethernet; Wi-Fi RF is not exercised.' \
    'One writable private disk persists across both Hello boot lifetimes and is never uploaded.' \
    >> "$out/evidence/platform.txt"
# The login modes used the first guest build. Its public receipts are already
# preserved in login-build/; keep this separately resolved virtual-kernel build
# identifiable instead of overwriting the first guest's package/kernel record.
python3 - "$out/evidence" <<'PY'
from pathlib import Path
import sys
evidence = Path(sys.argv[1])
destination = evidence / 'hello-build'
destination.mkdir(exist_ok=False)
for name in ('artifact-hashes.log', 'install.log', 'boot-config.log',
             'kernel-layout.log', 'ext4.log', 'ext4-hello.log', 'platform.txt'):
    source = evidence / name
    assert source.is_file() and not source.is_symlink(), name
    source.rename(destination / name)
PY
chmod 700 "$out/private"
chmod 600 "$out/private/rootfs.ext4"
chown -R "$(stat -c %u:%g /work)" "$out"
echo 'PASS: persistent Hello guest prepared from this run signed APKs.'
