#!/usr/bin/env bash
# One ARM runner: current source, optionally followed by fresh signed APKs,
# installed runtime, three login modes, interactive Hello and Settings.
# Hello continuation reuses verified APK/runtime/login receipts only when
# production inputs are unchanged; it builds one fresh private Hello guest.
# Source mode needs Docker; acceptance also needs QEMU and Python Pillow.
# Cache only dependencies/compiler output. Never upload guest credential disks,
# signing private keys, or the Cargo target/cache tree.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
if [[ ${1:-} == --help ]]; then
    echo 'Usage: bash scripts/ci/run.sh source|acceptance|hello (one disposable ARM64 Actions runner)'
    echo 'Hard limit: source 25 minutes; acceptance 55 minutes; hello 20 minutes, with no parallel runner or VM.'
    echo 'Hello requires ALCARD_CI_BASELINE_RUN and a read-only GH_TOKEN for the trusted baseline artifact.'
    exit 0
fi
mode=${1:-}
case "$mode" in
    source) budget=25m ;;
    acceptance) budget=55m ;;
    hello) budget=20m ;;
    *) echo 'Expected source, acceptance or hello' >&2; exit 2 ;;
esac
[[ ${GITHUB_ACTIONS:-} == true && ${RUNNER_ARCH:-} == ARM64 &&
   $(uname -m) == aarch64 && $(id -u) != 0 ]] || {
    echo 'Use one disposable, nonroot ubuntu-24.04-arm Actions runner' >&2
    exit 1
}
if [[ ${2:-} != --bounded ]]; then
    [[ $# == 1 ]] || { echo 'Unexpected argument' >&2; exit 2; }
    exec timeout --signal=TERM --kill-after=30 "$budget" bash "$0" "$mode" --bounded
fi
[[ $# == 2 ]] || exit 2
cd "$ROOT"
for command in docker python3 timeout git; do
    command -v "$command" >/dev/null
done
if [[ $mode == acceptance || $mode == hello ]]; then
    command -v qemu-system-aarch64 >/dev/null
    python3 -c 'from PIL import Image'
    export ALCARD_LOGIN_VM_TEST=1
fi
if [[ $mode == hello ]]; then
    command -v gh >/dev/null
    [[ ${ALCARD_CI_BASELINE_RUN:-} =~ ^[1-9][0-9]{0,19}$ ]] || {
        echo 'Hello continuation requires a numeric baseline run' >&2
        exit 2
    }
fi
[[ ! -L .alcard-local ]] || exit 1
OUT="$ROOT/.alcard-local/ci/$mode"
CACHE="$ROOT/.alcard-local/ci-cache"
PACKAGES="$ROOT/.alcard-local/ci-packages"
LOGIN="$ROOT/.alcard-local/ci-login"
INPUT="$ROOT/.alcard-local/login-input"
for directory in "$OUT" "$PACKAGES" "$LOGIN" "$INPUT"; do
    [[ ! -e "$directory" && ! -L "$directory" ]] || {
        echo "Refusing old acceptance output: $directory" >&2
        exit 1
    }
done
[[ ! -L "$CACHE" ]] || exit 1
mkdir -p "$OUT/stages" "$OUT/power-supply" "$OUT/backlight" "$CACHE"
CONTAINER="alcard-ci-${GITHUB_RUN_ID:?}-${GITHUB_RUN_ATTEMPT:?}"
[[ $CONTAINER =~ ^alcard-ci-[0-9]+-[0-9]+$ ]] || exit 1
started=$(date +%s)
current_stage=preflight

fingerprint() {
    python3 - "$ROOT" "$1" <<'PY'
import hashlib
import json
from pathlib import Path
import subprocess
import sys
root, target = map(Path, sys.argv[1:])
names = subprocess.check_output(['git', 'ls-files', '-z'], cwd=root).split(b'\0')
files = {name.decode(): hashlib.sha256((root / name.decode()).read_bytes()).hexdigest()
         for name in names if name and (root / name.decode()).is_file()}
target.write_text(json.dumps({'head': subprocess.check_output(
    ['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip(),
    'tracked_status': subprocess.check_output(
        ['git', 'status', '--porcelain', '--untracked-files=no'], cwd=root, text=True),
    'files': files}, indent=2) + '\n')
PY
}

cleanup() {
    result=$?
    trap - EXIT TERM INT
    set +e
    # Docker's daemon can outlive a killed CLI; remove only our two containers.
    timeout -k 5 15 docker rm -f "$CONTAINER" "$CONTAINER-guest" >/dev/null 2>&1
    timeout 5 docker image inspect alpine:3.24 --format '{{json .RepoDigests}}' \
        > "$OUT/alpine-image.json" 2> "$OUT/alpine-image-stderr.log"
    # The existing VM gate starts QEMU in its own session. Its normal finally
    # block cleans up; the bounded fallback below matches our exact kernel path
    # and current UID, so a timeout cannot leave this guest running.
    python3 - "$LOGIN/private/vmlinuz-virt" <<'PY'
import os
from pathlib import Path
import signal
import sys
import time
kernel = os.fsencode(sys.argv[1])
owned = []
for entry in Path('/proc').iterdir():
    if not entry.name.isdigit():
        continue
    try:
        if entry.stat().st_uid != os.getuid():
            continue
        args = (entry / 'cmdline').read_bytes().split(b'\0')
        if not args or b'qemu-system-aarch64' not in args[0]:
            continue
        if b'-kernel' not in args or args[args.index(b'-kernel') + 1] != kernel:
            continue
        descriptor = os.pidfd_open(int(entry.name))
        signal.pidfd_send_signal(descriptor, signal.SIGTERM)
        owned.append(descriptor)
    except (FileNotFoundError, ProcessLookupError, PermissionError, IndexError):
        continue
if owned:
    time.sleep(2)
for descriptor in owned:
    try:
        signal.pidfd_send_signal(descriptor, signal.SIGKILL)
    except ProcessLookupError:
        pass
    finally:
        os.close(descriptor)
PY
    rm -rf -- "$LOGIN/private"
    fingerprint "$OUT/inputs-after.json"
    python3 - "$OUT" "$result" "$started" "$current_stage" "$mode" <<'PY'
import json
from pathlib import Path
import sys
import time
out, result, started, stage, mode = Path(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3]), sys.argv[4], sys.argv[5]
before = json.loads((out / 'inputs-before.json').read_text())
after = json.loads((out / 'inputs-after.json').read_text())
unchanged = before == after
if not unchanged:
    result = 1
scope = {'source': 'Native Alpine aarch64 source checks',
         'acceptance': 'Native Alpine aarch64 source, current signed APKs, installed runtime, login, Hello and Settings',
         'hello': 'Hello/Settings continuation with verified unchanged production APKs'}[mode]
report = {'schema': 1, 'scope': scope,
          'exit_code': result, 'seconds': int(time.time()) - started,
          'last_stage': stage, 'inputs_unchanged': unchanged,
          'stages': [json.loads(path.read_text()) for path in sorted((out / 'stages').glob('*.json'))],
          'not_performed': ['Alpine x86_64', 'Cardputer hardware',
                            'physical Wi-Fi authentication/RF and Bluetooth interoperability',
                            'real bluetoothd pairing and service authorization',
                            'physical battery discharge', 'physical kernel suspend/wake',
                            'hardware RAM/CPU/energy benchmark'] +
                           (['APK build/install', 'native authentication and login VM',
                             'interactive Hello, installed Settings and idle sample',
                             'private-bus NetworkManager/BlueZ integration'] if mode == 'source' else [])}
if mode == 'hello':
    baseline = out / 'baseline/verification.json'
    report['baseline'] = json.loads(baseline.read_text()) if baseline.is_file() else None
    report['not_performed_this_run'] = [
        'Native source and private-bus cases', 'Fresh APK compilation and native runtime',
        'Three installation-mode login scenarios']
(out / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
print(f"{'PASS' if result == 0 else 'FAIL'}: Alpine aarch64 {mode}; last stage {stage}.")
raise SystemExit(0 if result == 0 else 1)
PY
    receipt_result=$?
    [[ $receipt_result == 0 ]] || result=1
    exit "$result"
}
trap cleanup EXIT
trap 'exit 143' TERM
trap 'exit 130' INT
fingerprint "$OUT/inputs-before.json"

stage() {
    local name=$1 limit=$2 began result
    shift 2
    current_stage=$name
    began=$(date +%s)
    printf '==> %s (limit %s seconds)\n' "$name" "$limit"
    set +e
    timeout --signal=INT --kill-after=60 "$limit" "$@" 2>&1 | tee "$OUT/stages/$name.log"
    local codes=("${PIPESTATUS[@]}")
    set -e
    result=${codes[0]}
    [[ ${codes[1]} == 0 ]] || result=1
    python3 - "$OUT/stages/$name.json" "$name" "$began" "$result" "$@" <<'PY'
import json
from pathlib import Path
import sys
import time
path, name, started, result, *command = sys.argv[1:]
Path(path).write_text(json.dumps({'name': name, 'exit_code': int(result),
    'seconds': int(time.time()) - int(started), 'command': command}, indent=2) + '\n')
PY
    return "$result"
}

if [[ $mode == source ]]; then
    stage 01-alpine-source 1440 \
        docker run --rm --init --name "$CONTAINER" \
        -e ALCARD_CI_CONTAINER=1 -e ALCARD_CI_SCOPE=source \
        --mount "type=bind,src=$ROOT,dst=/work" \
        --mount "type=bind,src=$CACHE,dst=/alcard-cache" \
        alpine:3.24 sh /work/scripts/ci/container.sh source-only
    current_stage=complete
    exit 0
fi
if [[ $mode == hello ]]; then
    stage 01-verify-hello-baseline 120 \
        python3 scripts/ci/hello-baseline.py prepare "$ALCARD_CI_BASELINE_RUN" "$OUT/baseline" "$INPUT"
    # The read-only repository token is needed only while retrieving the APKs.
    unset GH_TOKEN
    stage 02-build-hello-guest 300 \
        docker run --rm --init --name "$CONTAINER-guest" -e ALCARD_LOGIN_VM_BUILD=1 \
        --mount "type=bind,src=$ROOT,dst=/work" \
        alpine:3.24 sh /work/scripts/ci/hello/build-guest.sh
    stage 03-interactive-hello-settings 900 \
        python3 scripts/ci/hello/driver.py --directory "$LOGIN"
    current_stage=complete
    exit 0
fi
stage 01-native-source-apk-runtime 1800 \
    docker run --rm --init --name "$CONTAINER" \
    -e ALCARD_CI_CONTAINER=1 -e ALCARD_CI_ACCEPTANCE=1 -e ALCARD_CI_SCOPE=acceptance \
    -e ALCARD_BATTERY_SYSFS_FIXTURE=/work/.alcard-local/ci/acceptance/power-supply \
    -e ALCARD_BACKLIGHT_SYSFS_FIXTURE=/work/.alcard-local/ci/acceptance/backlight \
    --mount "type=bind,src=$ROOT,dst=/work" \
    --mount "type=bind,src=$CACHE,dst=/alcard-cache" \
    --mount "type=bind,src=$OUT/power-supply,dst=/sys/class/power_supply,readonly" \
    --mount "type=bind,src=$OUT/backlight,dst=/sys/class/backlight,readonly" \
    alpine:3.24 sh /work/scripts/ci/container.sh run
stage 02-stage-same-signed-apks 30 python3 - "$PACKAGES" "$INPUT" <<'PY_STAGE'
from pathlib import Path
import shutil
import sys
source, target = map(Path, sys.argv[1:])
assert (source / 'status.txt').read_text().strip() == 'exit_status=0'
assert len(list((source / 'packages/aarch64').glob('*.apk'))) == 9
target.mkdir()
shutil.copytree(source / 'packages', target / 'packages')
for name in ['SHA256SUMS', 'status.txt', *[p.name for p in source.glob('*.rsa.pub')]]:
    shutil.copy2(source / name, target / name)
PY_STAGE
stage 03-build-login-guest 300 \
    docker run --rm --init --name "$CONTAINER-guest" -e ALCARD_LOGIN_VM_BUILD=1 \
    --mount "type=bind,src=$ROOT,dst=/work" \
    alpine:3.24 sh /work/scripts/build-login-vm.sh
stage 04-sequential-login-modes 1080 \
    bash scripts/test-login-vm.sh --modes graphics graphics_minimal console
# Use the same signed APKs for a fresh persistent Hello disk. The earlier VM
# processes have exited; no second runner or concurrent VM is started.
stage 05-preserve-login-build 30 python3 - "$LOGIN/evidence" <<'PY_GUEST'
from pathlib import Path
import sys
evidence = Path(sys.argv[1])
destination = evidence / 'login-build'
destination.mkdir(exist_ok=False)
for name in ('artifact-hashes.log', 'install.log', 'boot-config.log',
             'kernel-layout.log', 'ext4.log', 'platform.txt'):
    source = evidence / name
    assert source.is_file() and not source.is_symlink(), name
    source.rename(destination / name)
PY_GUEST
stage 06-build-hello-guest 300 \
    docker run --rm --init --name "$CONTAINER-guest" -e ALCARD_LOGIN_VM_BUILD=1 \
    --mount "type=bind,src=$ROOT,dst=/work" \
    alpine:3.24 sh /work/scripts/ci/hello/build-guest.sh
stage 07-interactive-hello-settings 900 \
    python3 scripts/ci/hello/driver.py --directory "$LOGIN"
current_stage=complete
