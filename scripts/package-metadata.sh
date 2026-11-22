#!/usr/bin/env bash
# Source from a build/verification script after ROOT points to this checkout.
# APKBUILD is parsed as data by the shared Python reader, never sourced as shell.
alcard_package() {
    python3 "$ROOT/tools/package_metadata.py" "$@"
}

# Capture a failed metadata lookup before mapfile, instead of losing its status
# through process substitution. Call with an array name, followed by CLI args.
alcard_package_array() {
    local array_name="$1" values
    shift
    values="$(alcard_package "$@")" || return
    mapfile -t "$array_name" <<< "$values"
}
