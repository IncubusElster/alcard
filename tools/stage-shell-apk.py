#!/usr/bin/env python3
"""Create deterministic APK inputs from native release binaries and session assets."""

import argparse
import gzip
import io
import pathlib
import struct
import tarfile

from package_metadata import Catalog


def native_binary(path):
    if path.is_symlink() or not path.is_file():
        raise SystemExit(f"Missing or symlinked package binary: {path}")
    data = path.read_bytes()
    if (len(data) < 64 or data[:6] != b"\x7fELF\x02\x01"
            or struct.unpack_from("<H", data, 18)[0] != 183
            or b"/lib/ld-musl-aarch64.so.1\0" not in data):
        raise SystemExit(f"Expected a native dynamically linked aarch64/musl ELF: {path}")
    return path, 0o755


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=pathlib.Path, required=True)
    parser.add_argument("--profile", choices=("shell", "polkit", "lock", "session"), required=True)
    parser.add_argument("--target", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    names = {"shell": "alcard-shell", "polkit": "alcard-polkit-agent",
             "lock": "alcard-lock", "session": "alcard-desktop-shell"}
    package = names[args.profile]
    version = Catalog(args.root)[package].pkgver
    files = {}
    if args.profile == "session":
        for name in ("shell-session", "session-client", "audio-session", "rc.xml", "environment", "autostart"):
            mode = 0o755 if name in ("shell-session", "session-client", "audio-session", "autostart") else 0o644
            files[name] = (args.root / "scripts/wayland" / name, mode)
    else:
        binary = args.target / "release" / package
        files[package] = native_binary(binary)
        if args.profile == "shell":
            files["alcard-radio-helper"] = native_binary(args.target / "release/alcard-radio-helper")
            policy = "org.alcard.radio.policy"
            files[policy] = (args.root / "scripts/radios" / policy, 0o644)
        if args.profile == "lock":
            files["alcard-lock.pamd"] = (args.root / "scripts/pam/alcard-lock.alpine", 0o644)
    if args.output.is_symlink():
        raise SystemExit(f"Refusing symlinked archive: {args.output}")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("wb") as raw:
        with gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=0) as zipped:
            with tarfile.open(fileobj=zipped, mode="w|", format=tarfile.USTAR_FORMAT) as archive:
                for name, (path, mode) in sorted(files.items()):
                    if path.is_symlink() or not path.is_file():
                        raise SystemExit(f"Missing or symlinked package input: {path}")
                    content = path.read_bytes()
                    item = tarfile.TarInfo(f"{package}-{version}/{name}")
                    item.size = len(content)
                    item.mode = mode
                    item.uid = item.gid = item.mtime = 0
                    archive.addfile(item, io.BytesIO(content))
    print(f"Prepared {package}: {len(files)} native package inputs")


if __name__ == "__main__":
    main()
