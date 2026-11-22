#!/usr/bin/env bash
# Host side is an unprivileged QEMU process, with no NIC or host device mounts.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
[[ ${ALCARD_LOGIN_VM_TEST:-} == 1 && $(id -u) != 0 ]] || {
    echo 'Use the isolated nonroot CI runner for login VM acceptance' >&2; exit 1;
}
for program in qemu-system-aarch64 python3; do command -v "$program" >/dev/null; done
ulimit -c 0
exec python3 "$ROOT/tools/test-login-vm.py" \
    --directory "$ROOT/.alcard-local/ci-login" "$@"
