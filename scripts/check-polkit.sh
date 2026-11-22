#!/usr/bin/env bash
# Existing Alpine aarch64 Distrobox only; never installs packages or creates a container.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
[[ "$(apk --print-arch)" == aarch64 ]] || { echo 'Expected Alpine aarch64' >&2; exit 1; }
for tool in cargo rustc rustdoc rustfmt cargo-clippy; do
  [[ -x /usr/bin/$tool ]] || { echo "Missing container /usr/bin/$tool" >&2; exit 1; }
done
export PATH="/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
export RUSTC=/usr/bin/rustc RUSTDOC=/usr/bin/rustdoc RUSTFMT=/usr/bin/rustfmt ALCARD_CARGO=/usr/bin/cargo
unset RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_BUILD_TARGET CARGO_ENCODED_RUSTFLAGS RUSTFLAGS
export CARGO_HOME="$ROOT/.alcard-local/cargo-home/polkit-aarch64-musl"
export CARGO_TARGET_DIR="$ROOT/.alcard-local/cargo-target/polkit-aarch64-musl"
/usr/bin/rustc -vV
/usr/bin/rustc -vV | grep -Eq '^host: aarch64-(unknown|alpine)-linux-musl$'
pkg-config --modversion glib-2.0 gio-2.0 polkit-gobject-1 polkit-agent-1 xkbcommon
for project in rust/crates/alcard-ui rust/apps/alcard-polkit-agent; do
  cd "$ROOT/$project"
  /usr/bin/cargo fmt --check
  /usr/bin/cargo test --locked --all-targets
  /usr/bin/cargo clippy --locked --all-targets -- -D warnings
  /usr/bin/cargo build --locked
done
printf '%s\n' 'PASS: shared UI and graphical polkit agent on Alpine aarch64; login/PAM integration is a separate runtime check'
