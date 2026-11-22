#!/usr/bin/env bash
# Called through build.sh in the existing Alpine aarch64 Distrobox.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
[[ "$(apk --print-arch)" == aarch64 ]] || { echo 'Expected Alpine aarch64 container' >&2; exit 1; }
for tool in /usr/bin/cargo /usr/bin/rustc /usr/bin/rustfmt /usr/bin/cargo-clippy; do
  [[ -x "$tool" ]] || { echo "Missing $tool; install cargo/rust/rustfmt/rust-clippy in the container" >&2; exit 1; }
done
# Distrobox shares HOME. Cargo also searches CARGO_HOME/bin for subcommands,
# so PATH alone cannot isolate cargo-fmt/clippy from the host's glibc shims.
export PATH="/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
export CARGO_HOME="${ALCARD_CARGO_HOME:-$ROOT/.alcard-local/cargo-home/shell-aarch64-musl}"
mkdir -p "$CARGO_HOME"
export RUSTC=/usr/bin/rustc
export RUSTDOC=/usr/bin/rustdoc
export RUSTFMT=/usr/bin/rustfmt
export ALCARD_CARGO=/usr/bin/cargo
unset RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_BUILD_TARGET CARGO_ENCODED_RUSTFLAGS RUSTFLAGS
export CARGO_TARGET_DIR="$ROOT/.alcard-local/cargo-target/shell-aarch64-musl"
/usr/bin/rustc -vV
/usr/bin/rustc -vV | grep -Eq '^host: aarch64-(unknown|alpine)-linux-musl$'
pkg-config --modversion xkbcommon
cd "$ROOT/rust/apps/alcard-shell"
/usr/bin/cargo fmt --check
/usr/bin/cargo test --locked --all-targets
/usr/bin/cargo clippy --locked --all-targets -- -D warnings
/usr/bin/cargo build --locked
python3 "$ROOT/tools/audit-shell.py"
