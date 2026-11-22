#!/usr/bin/env bash
# Build signed native runtime APKs in the existing Alpine aarch64 environment.
# No container creation, package installation, keys or privileged host changes.
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
source "$ROOT/scripts/package-metadata.sh"
LOCAL="$ROOT/.alcard-local"
PROFILE="${2:-all}"
case "$PROFILE" in shell|polkit|lock|session|all) ;; *) echo 'Expected shell|polkit|lock|session|all' >&2; exit 2 ;; esac
fail() { echo "ERROR: $*" >&2; exit 1; }

check() {
    [[ "$(apk --print-arch)" == aarch64 ]] || fail 'Requires Alpine aarch64'
    [[ "$(id -u)" != 0 ]] || fail 'Never run Cargo or abuild as root'
    for tool in cargo rustc rustdoc; do
        [[ -x /usr/bin/$tool ]] || fail "Missing native /usr/bin/$tool"
    done
    /usr/bin/rustc -vV | grep -Eq '^host: aarch64-(unknown|alpine)-linux-musl$' ||
        fail 'Requires the native Alpine aarch64 Rust toolchain'
    for tool in abuild python3 pkg-config; do
        command -v "$tool" >/dev/null || fail "Missing $tool"
    done
    apk info -e alpine-sdk >/dev/null || fail 'Install alpine-sdk in the build environment'
    pkg-config --exists xkbcommon || fail 'Install libxkbcommon-dev'
    if [[ "$PROFILE" == polkit || "$PROFILE" == all ]]; then
        pkg-config --exists glib-2.0 gio-2.0 polkit-gobject-1 polkit-agent-1 ||
            fail 'Install glib-dev and polkit-elogind-dev'
    fi
    if [[ "$PROFILE" == lock || "$PROFILE" == all ]]; then
        apk info -e linux-pam-dev >/dev/null || fail 'Install linux-pam-dev'
    fi
    [[ -s "$ROOT/tools/stage-shell-apk.py" ]] || fail 'Missing package staging tool'
    # Audio clients are target runtime helpers, not Cargo build dependencies.
    # The signed-install verifier checks package ownership, ELF and mode.
    # Do not mistake the build host's PATH for the target's dependency closure.
    if [[ "$PROFILE" == shell || "$PROFILE" == all ]]; then
        for dependency in wireplumber pipewire-tools networkmanager-cli bluez polkit-elogind; do
            grep -Eq "^depends=\"([^\" ]+ )*$dependency( [^\" ]+)*\"$" \
                "$ROOT/packaging/apk/alcard-shell/APKBUILD" ||
                fail "Shell must declare the $dependency runtime dependency"
        done
        grep -Fq 'dbus>=1.16.2' "$ROOT/packaging/apk/alcard-shell/APKBUILD" ||
            fail 'Shell requires D-Bus with /etc/dbus-1/system-services precedence'
        for asset in org.alcard.radio.policy org.bluez.service bluetooth.conf; do
            [[ -s "$ROOT/scripts/radios/$asset" ]] || fail "Missing radio asset: $asset"
        done
    fi
    if [[ "$PROFILE" == session || "$PROFILE" == all ]]; then
        for dependency in pipewire wireplumber pipewire-pulse pipewire-alsa flock; do
            grep -Eq "^depends=\"([^\" ]+ )*$dependency( [^\" ]+)*\"$" \
                "$ROOT/packaging/apk/alcard-desktop-shell/APKBUILD" ||
                fail "Graphics must declare the $dependency runtime dependency"
        done
        [[ -s "$ROOT/scripts/wayland/audio-session" ]] || fail 'Missing audio session helper'
    fi
    echo "==> Alcard runtime APK prerequisites OK ($PROFILE)"
}

build_one() {
    local profile="$1" package version apk_file work
    case "$profile" in
        shell) package=alcard-shell ;;
        polkit) package=alcard-polkit-agent ;;
        lock) package=alcard-lock ;;
        session) package=alcard-desktop-shell ;;
    esac
    version="$(alcard_package pkgver "$package")"
    apk_file="$(alcard_package filename "$package")"
    work="$LOCAL/work/aports/alcard/$package"
    [[ ! -L "$work" ]] || fail "Refusing symlinked package work directory: $work"
    mkdir -p "$work" "$LOCAL/packages/alcard/aarch64"
    if [[ "$profile" != session ]]; then
        /usr/bin/cargo build --release --locked --bins --message-format=json-render-diagnostics \
            --manifest-path "$ROOT/rust/apps/$package/Cargo.toml" |
            python3 "$ROOT/tools/verify-production-build.py" build "$profile" "$CARGO_TARGET_DIR"
    fi
    cp "$ROOT/packaging/apk/$package/APKBUILD" "$work/APKBUILD"
    python3 "$ROOT/tools/stage-shell-apk.py" --root "$ROOT" --profile "$profile" \
        --target "$CARGO_TARGET_DIR" --output "$work/$package-$version.tar.gz"
    (
        cd "$work"
        abuild checksum
        /bin/sh -n APKBUILD
        REPODEST="$LOCAL/packages" abuild -d
    )
    [[ -s "$LOCAL/packages/alcard/aarch64/$apk_file" ]] ||
        fail "Signed APK missing for $package"
    echo "==> Built $apk_file"
}

case "${1:-}" in
    check) check ;;
    build)
        check
        # Distrobox shares the host home and PATH; never use its glibc rustup shims.
        export PATH="/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
        export RUSTC=/usr/bin/rustc RUSTDOC=/usr/bin/rustdoc
        unset RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_BUILD_TARGET CARGO_ENCODED_RUSTFLAGS RUSTFLAGS
        export CARGO_HOME="$LOCAL/cargo-home/packages-aarch64-musl"
        export CARGO_TARGET_DIR="$LOCAL/cargo-target/packages-aarch64-musl"
        if [[ "$PROFILE" == all ]]; then
            for profile in shell polkit lock session; do build_one "$profile"; done
        else
            build_one "$PROFILE"
        fi
        ;;
    *) fail 'Usage: build-shell-apk.sh check|build [shell|polkit|lock|session|all]' ;;
esac
