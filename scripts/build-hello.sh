#!/usr/bin/env bash
# Build signed Alcard Hello APK in the existing Alpine aarch64 Distrobox.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
source "$ROOT/scripts/package-metadata.sh"
LOCAL="$ROOT/.alcard-local"
PKG=alcard-hello
VER="$(alcard_package pkgver "$PKG")"
WORK="$LOCAL/work/aports/alcard/$PKG"
REPO="$LOCAL/packages/alcard/aarch64"

fail() { echo "ERROR: $*" >&2; exit 1; }

check() {
    [[ "$(apk --print-arch)" == aarch64 ]] ||
        fail "Requires Alpine aarch64 Distrobox"
    [[ "$(id -u)" != 0 ]] ||
        fail "Do not run abuild or cargo as root"

    # Distrobox shares the host's home and PATH; rustup shims from the
    # host can be found by command -v but cannot run inside Alpine musl.
    # Require and execute Alpine's native aarch64 Rust toolchain explicitly.
    [[ -x /usr/bin/cargo && -x /usr/bin/rustc ]] ||
        fail "Missing native Alpine Rust toolchain: install cargo and rust"
    /usr/bin/cargo --version >/dev/null 2>&1 ||
        fail "Alpine cargo cannot execute"
    /usr/bin/rustc --version >/dev/null 2>&1 ||
        fail "Alpine rustc cannot execute"

    for cmd in abuild python3 apk tar gzip grep; do
        command -v "$cmd" >/dev/null || fail "Missing $cmd"
    done
    apk info -e alpine-sdk >/dev/null 2>&1 || fail "Install alpine-sdk"
    [[ -s "$ROOT/rust/Cargo.toml" && -s "$ROOT/rust/Cargo.lock" ]] ||
        fail "Missing Rust workspace or Cargo.lock"

    if ! grep -qx 'name = "zeroize"' "$ROOT/rust/Cargo.lock"; then
        fail "Cargo.lock is stale: regenerate and commit rust/Cargo.lock with the supported toolchain before building; APK builds use --locked"
    fi
    [[ -s "$LOCAL/packages/alcard/aarch64/$(alcard_package filename alcard-keyboard-zero)" ]] ||
        fail "Build alcard-keyboard-zero first: ./build.sh keyboard"
    [[ -s "$ROOT/packaging/apk/$PKG/APKBUILD" ]] ||
        fail "Missing APKBUILD"
    for name in tty1-dispatch firstboot-init start-session alcard-greetd.toml alcard-settings.desktop; do
        [[ -s "$ROOT/scripts/hello/$name" ]] || fail "Missing Hello asset: $name"
    done
    [[ -s "$ROOT/scripts/pam/alcard-greetd.alpine" ]] || fail 'Missing Alcard login PAM profile'

    echo "==> $PKG build prerequisites OK"
}

build() {
    check
    # Keep native aarch64/musl artifacts and dependency cache away from shared
    # host rustup/Cargo state, just like the independent graphical workspaces.
    export PATH="/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
    export RUSTC=/usr/bin/rustc RUSTDOC=/usr/bin/rustdoc
    unset RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_BUILD_TARGET CARGO_ENCODED_RUSTFLAGS RUSTFLAGS
    export CARGO_HOME="$LOCAL/cargo-home/hello-aarch64-musl"
    export CARGO_TARGET_DIR="$LOCAL/cargo-target/hello-aarch64-musl"
    (
        cd "$ROOT/rust"
        /usr/bin/cargo build --release --locked --bins --message-format=json-render-diagnostics \
            -p alcard-settings -p alcard-setupd |
            python3 "$ROOT/tools/verify-production-build.py" build hello "$CARGO_TARGET_DIR"
    )

    mkdir -p "$WORK" "$REPO"
    cp "$ROOT/packaging/apk/$PKG/APKBUILD" "$ROOT/packaging/apk/$PKG/$PKG.post-upgrade" "$WORK/"

    python3 - "$ROOT" "$WORK/$PKG-$VER.tar.gz" "$PKG-$VER" "$CARGO_TARGET_DIR" <<'PY'
import gzip
import io
import pathlib
import sys
import tarfile

root = pathlib.Path(sys.argv[1])
output = pathlib.Path(sys.argv[2])
prefix = sys.argv[3]
target = pathlib.Path(sys.argv[4])
files = {
    "alcard-settings": target / "release/alcard-settings",
    "alcard-setupd": target / "release/alcard-setupd",
    "org.bluez.service": root / "scripts/radios/org.bluez.service",
    "bluetooth.conf": root / "scripts/radios/bluetooth.conf",
    "alcard-settings.desktop": root / "scripts/hello/alcard-settings.desktop",
    "tty1-dispatch": root / "scripts/hello/tty1-dispatch",
    "firstboot-init": root / "scripts/hello/firstboot-init",
    "start-session": root / "scripts/hello/start-session",
    "alcard-greetd.toml": root / "scripts/hello/alcard-greetd.toml",
    "alcard-greetd.pamd": root / "scripts/pam/alcard-greetd.alpine",
}
with output.open("wb") as target:
    with gzip.GzipFile(fileobj=target, mode="wb", filename="", mtime=0) as gz:
        with tarfile.open(fileobj=gz, mode="w|", format=tarfile.USTAR_FORMAT) as tar:
            for name, path in sorted(files.items()):
                content = path.read_bytes()
                entry = tarfile.TarInfo(f"{prefix}/{name}")
                entry.size = len(content)
                entry.mode = 0o644 if name.endswith((".toml", ".pamd", ".desktop", ".conf", ".service")) else 0o755
                entry.uid = entry.gid = entry.mtime = 0
                tar.addfile(entry, io.BytesIO(content))
PY

    (
        cd "$WORK"
        abuild checksum
        /bin/sh -n APKBUILD || fail "Invalid APKBUILD"
        REPODEST="$LOCAL/packages" abuild -d
    )

    [[ -s "$REPO/$(alcard_package filename "$PKG")" ]] ||
        fail "Missing signed APK: $REPO/$(alcard_package filename "$PKG")"
    echo "==> Built: $REPO/$(alcard_package filename "$PKG")"
}

case "${1:-}" in
    check) check ;;
    build) build ;;
    *) fail "Usage: $0 check|build" ;;
esac
