#!/bin/sh
# One disposable native Alpine container: source -> fresh APKs -> runtime.
# Invoked by run.sh; the existing package gate calls the prepare/source hooks.
set -eu
SOURCE=/home/alcard-build/alcard
OUT=/work/.alcard-local/ci/${ALCARD_CI_SCOPE:?}
CACHE=/alcard-cache

guard() {
    [ "${ALCARD_CI_CONTAINER:-}" = 1 ] && [ -f /.dockerenv ] &&
        [ "$(apk --print-arch)" = aarch64 ] || {
        echo 'CI requires the disposable native Alpine aarch64 container' >&2
        exit 1
    }
    case "$(cat /etc/alpine-release)" in 3.24.*) ;; *) exit 1 ;; esac
    [ ! -L /work/.alcard-local ] && [ ! -L "$OUT" ] &&
        [ ! -L "$CACHE" ] || exit 1
}

guard
case "$ALCARD_CI_SCOPE" in source|acceptance) ;; *) exit 2 ;; esac
case "${1:-run}" in
    source-only)
        [ "$(id -u)" = 0 ] && [ "$ALCARD_CI_SCOPE" = source ] || exit 1
        apk add --no-cache cargo rust rustfmt rust-clippy build-base pkgconf \
            libxkbcommon-dev wayland-dev glib-dev polkit-elogind-dev linux-pam-dev \
            python3 bash util-linux coreutils flock runuser
        adduser -D -s /bin/sh alcard-build
        mkdir -p "$SOURCE"
        tar -C /work --exclude=.git --exclude=.alcard-local --exclude=target \
            --exclude=__pycache__ -cf - . | tar -C "$SOURCE" -xf -
        chown -R alcard-build:alcard-build /home/alcard-build
        sh "$SOURCE/scripts/ci/container.sh" prepare
        export ALCARD_CI_SOURCE_ONLY=1
        exec su -s /bin/sh alcard-build -c 'sh /home/alcard-build/alcard/scripts/ci/container.sh source'
        ;;
    run)
        [ "$(id -u)" = 0 ] || exit 1
        export ALCARD_SESSION_TEST_CONTAINER=1
        # The package script records its own result/public APKs even on failure.
        sh /work/scripts/test-session-apk-native.sh
        export ALCARD_CI_RUNTIME_CONTAINER=1
        export ALCARD_CI_RUNTIME_EVIDENCE="$OUT/runtime"
        export ALCARD_LOCK_TEST_BIN
        ALCARD_LOCK_TEST_BIN=$(cat "$OUT/native/lock-test-bin.txt")
        export ALCARD_SHELL_TEST_BIN
        ALCARD_SHELL_TEST_BIN=$(cat "$OUT/native/shell-test-bin.txt")
        export ALCARD_POINTER_BIN
        ALCARD_POINTER_BIN=$(cat "$OUT/native/pointer-bin.txt")
        export ALCARD_OUTPUT_POWER_BIN
        ALCARD_OUTPUT_POWER_BIN=$(cat "$OUT/native/output-power-bin.txt")
        [ -x "$ALCARD_LOCK_TEST_BIN" ] || exit 1
        [ -x "$ALCARD_SHELL_TEST_BIN" ] || exit 1
        [ -x "$ALCARD_POINTER_BIN" ] || exit 1
        [ -x "$ALCARD_OUTPUT_POWER_BIN" ] || exit 1
        timeout -k 20 1080 bash /work/scripts/check-session-runtime.sh
        ;;
    prepare)
        [ "$(id -u)" = 0 ] && [ -d "$SOURCE/rust" ] &&
            [ -d "$CACHE" ] || exit 1
        for directory in "$CACHE/cargo-home" "$CACHE/cargo-target"; do
            [ ! -L "$directory" ] || exit 1
        done
        mkdir -p "$CACHE/cargo-home/registry" "$CACHE/cargo-target" \
            "$SOURCE/.alcard-local/cargo-home" \
            "$SOURCE/.alcard-local/cargo-target" "$OUT/native"
        # Share only native dependency/build caches, never keys, source trees,
        # signed APKs, private VM disks, or an earlier acceptance result.
        for profile in ci-aarch64-musl hello-aarch64-musl packages-aarch64-musl; do
            ln -s "$CACHE/cargo-home" "$SOURCE/.alcard-local/cargo-home/$profile"
            ln -s "$CACHE/cargo-target" "$SOURCE/.alcard-local/cargo-target/$profile"
        done
        chown -R alcard-build:alcard-build "$CACHE" "$OUT/native" \
            "$SOURCE/.alcard-local"
        ;;
    source)
        [ "$(id -u)" != 0 ] && [ "$(id -un)" = alcard-build ] || exit 1
        cd "$SOURCE"
        export PATH=/usr/bin:/bin:/usr/sbin:/sbin
        export RUSTC=/usr/bin/rustc RUSTDOC=/usr/bin/rustdoc RUSTFMT=/usr/bin/rustfmt
        export CARGO_HOME="$CACHE/cargo-home"
        export CARGO_TARGET_DIR="$SOURCE/.alcard-local/cargo-target/ci-aarch64-musl"
        unset RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_BUILD_TARGET \
            CARGO_ENCODED_RUSTFLAGS RUSTFLAGS PKG_CONFIG_PATH PKG_CONFIG_LIBDIR \
            PKG_CONFIG_SYSROOT_DIR LIBRARY_PATH LD_LIBRARY_PATH
        native_target=$(/usr/bin/rustc -vV | sed -n 's/^host: //p')
        # Fetch the complete committed graphs: the architecture auditor runs
        # unfiltered Cargo metadata offline, including non-native cfg entries.
        # Compilation below remains explicitly native; no lockfile update.
        for project in rust rust/crates/alcard-ui rust/apps/alcard-shell \
            rust/apps/alcard-polkit-agent rust/apps/alcard-lock; do
            name=$(printf '%s' "$project" | tr / -)
            /usr/bin/cargo fetch --locked \
                --manifest-path "$project/Cargo.toml" \
                > "$OUT/native/fetch-$name.log" 2>&1 || {
                cat "$OUT/native/fetch-$name.log" >&2
                exit 1
            }
        done
        ALCARD_CARGO_HOME="$CARGO_HOME" \
            bash scripts/check-alpine-source.sh source "$OUT/native/source"
        /usr/bin/rustc -vV > "$OUT/native/rust-toolchain.txt"
        apk info -v | sort > "$OUT/native/alpine-packages.txt"
        python3 tools/package_metadata.py manifest > "$OUT/native/package-recipes.json"
        [ "${ALCARD_CI_SOURCE_ONLY:-}" != 1 ] || exit 0
        # The ignored idle/PAM test uses this exact just-tested musl executable.
        # JSON selects Cargo's output; never guess a stale cached hash filename.
        /usr/bin/cargo test --offline --locked --all-targets --no-run \
            --target "$native_target" --manifest-path rust/apps/alcard-lock/Cargo.toml \
            --message-format=json > "$OUT/native/lock-test-artifacts.jsonl" \
            2> "$OUT/native/lock-test-build.log"
        /usr/bin/cargo test --offline --locked --all-targets --no-run \
            --target "$native_target" --manifest-path rust/apps/alcard-shell/Cargo.toml \
            --message-format=json > "$OUT/native/shell-test-artifacts.jsonl" \
            2> "$OUT/native/shell-test-build.log"
        /usr/bin/cargo build --offline --locked --target "$native_target" \
            --manifest-path rust/apps/alcard-shell/Cargo.toml --example park-pointer \
            --message-format=json > "$OUT/native/pointer-artifacts.jsonl" \
            2> "$OUT/native/pointer-build.log"
        /usr/bin/cargo build --offline --locked --target "$native_target" \
            --manifest-path rust/apps/alcard-shell/Cargo.toml --example output-power-fixture \
            --message-format=json > "$OUT/native/output-power-artifacts.jsonl" \
            2> "$OUT/native/output-power-build.log"
        python3 - "$OUT/native" <<'PY'
import hashlib
import json
from pathlib import Path
import sys

out = Path(sys.argv[1])
artifacts = [json.loads(line) for line in (out / 'lock-test-artifacts.jsonl').read_text().splitlines()]
matches = {item['executable'] for item in artifacts
           if item.get('executable') and item.get('profile', {}).get('test')
           and item.get('target', {}).get('name') == 'alcard-lock'}
assert len(matches) == 1, f'Expected one exact locker test artifact, got {len(matches)}'
binary = Path(matches.pop())
assert binary.is_file()
(out / 'lock-test-bin.txt').write_text(str(binary) + '\n')
(out / 'lock-test-bin.json').write_text(json.dumps({
    'path': str(binary), 'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
    'scope': 'Native Alpine aarch64/musl test executable for explicit idle/PAM acceptance',
}, indent=2) + '\n')
artifacts = [json.loads(line) for line in (out / 'shell-test-artifacts.jsonl').read_text().splitlines()]
matches = {item['executable'] for item in artifacts
           if item.get('executable') and item.get('profile', {}).get('test')
           and item.get('target', {}).get('name') == 'alcard-shell'}
assert len(matches) == 1, f'Expected one exact Shell test artifact, got {len(matches)}'
shell = Path(matches.pop())
assert shell.is_file()
(out / 'shell-test-bin.txt').write_text(str(shell) + '\n')
(out / 'shell-test-bin.json').write_text(json.dumps({
    'path': str(shell), 'sha256': hashlib.sha256(shell.read_bytes()).hexdigest(),
    'scope': 'Native Alpine aarch64/musl test executable for explicit private D-Bus acceptance',
}, indent=2) + '\n')
artifacts = [json.loads(line) for line in (out / 'pointer-artifacts.jsonl').read_text().splitlines()]
matches = {item['executable'] for item in artifacts if item.get('executable')
           and item.get('target', {}).get('name') == 'park-pointer'
           and item.get('target', {}).get('kind') == ['example']}
assert len(matches) == 1, f'Expected one pointer example, got {len(matches)}'
pointer = Path(matches.pop())
assert pointer.is_file()
(out / 'pointer-bin.txt').write_text(str(pointer) + '\n')
(out / 'pointer-bin.json').write_text(json.dumps({
    'path': str(pointer), 'sha256': hashlib.sha256(pointer.read_bytes()).hexdigest(),
    'scope': 'Native Alpine aarch64/musl pointer placement fixture',
}, indent=2) + '\n')
artifacts = [json.loads(line) for line in (out / 'output-power-artifacts.jsonl').read_text().splitlines()]
matches = {item['executable'] for item in artifacts if item.get('executable')
           and item.get('target', {}).get('name') == 'output-power-fixture'
           and item.get('target', {}).get('kind') == ['example']}
assert len(matches) == 1, f'Expected one output-power example, got {len(matches)}'
power = Path(matches.pop())
assert power.is_file()
(out / 'output-power-bin.txt').write_text(str(power) + '\n')
(out / 'output-power-bin.json').write_text(json.dumps({
    'path': str(power), 'sha256': hashlib.sha256(power.read_bytes()).hexdigest(),
    'scope': 'Native Alpine aarch64/musl private output-power protocol fixture',
}, indent=2) + '\n')
PY
        ;;
    *) echo 'Usage: container.sh source-only|run|prepare|source' >&2; exit 2 ;;
esac
