#!/usr/bin/env bash
# Existing Alpine aarch64 environment only. No host PAM or package changes.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
[[ "$(apk --print-arch)" == aarch64 ]] || { echo 'Expected Alpine aarch64' >&2; exit 1; }
for tool in cargo rustc rustdoc rustfmt cargo-clippy; do
  [[ -x /usr/bin/$tool ]] || { echo "Missing container /usr/bin/$tool" >&2; exit 1; }
done
export PATH="/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
export RUSTC=/usr/bin/rustc RUSTDOC=/usr/bin/rustdoc RUSTFMT=/usr/bin/rustfmt
export CARGO_HOME="$ROOT/.alcard-local/cargo-home/lock-aarch64-musl"
export CARGO_TARGET_DIR="$ROOT/.alcard-local/cargo-target/lock-aarch64-musl"
unset RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_BUILD_TARGET CARGO_ENCODED_RUSTFLAGS RUSTFLAGS
/usr/bin/rustc -vV
/usr/bin/rustc -vV | grep -Eq '^host: aarch64-(unknown|alpine)-linux-musl$'
pkg-config --modversion xkbcommon wayland-client
cd "$ROOT/rust/apps/alcard-lock"
/usr/bin/cargo fmt --check
/usr/bin/cargo test --locked --all-targets
/usr/bin/cargo clippy --locked --all-targets -- -D warnings
/usr/bin/cargo build --locked
printf '%s\n' 'PASS: independent locker builds/tests on Alpine aarch64 musl; native PAM/compositor integration is a separate disposable-container gate'
