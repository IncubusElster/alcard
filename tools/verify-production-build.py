"""Check the Cargo artifacts used by the APK builders and the installed package files."""

import argparse
import hashlib
import json
from pathlib import Path
import sys


BINARIES = {
    "hello": {"alcard-settings": "usr/bin", "alcard-setupd": "usr/sbin"},
    "shell": {"alcard-shell": "usr/bin", "alcard-radio-helper": "usr/libexec/alcard"},
    "polkit": {"alcard-polkit-agent": "usr/bin"},
    "lock": {"alcard-lock": "usr/bin"},
}
PACKAGES = {"hello": "alcard-hello", "shell": "alcard-shell",
            "polkit": "alcard-polkit-agent", "lock": "alcard-lock"}
ASSETS = {
    "hello": {"etc/alcard/bluetooth.conf", "etc/dbus-1/system-services/org.bluez.service",
              "usr/share/applications/alcard-settings.desktop", "usr/libexec/alcard/tty1-dispatch", "usr/libexec/alcard/firstboot-init",
              "usr/libexec/alcard/start-session", "etc/greetd/alcard.toml", "etc/pam.d/alcard-greetd"},
    "shell": {"usr/share/polkit-1/actions/org.alcard.radio.policy"},
    "polkit": set(),
    "lock": {"etc/pam.d/alcard-lock"},
}


def verify_build(lines, profile, target):
    expected = set(BINARIES[profile])
    found = set()
    finished = False
    for line in lines:
        entry = json.loads(line)
        if entry["reason"] == "build-finished":
            finished = entry["success"]
        if entry["reason"] != "compiler-artifact":
            continue
        kinds = set(entry["target"]["kind"])
        if entry["profile"]["test"] or kinds & {"test", "bench", "example"}:
            raise ValueError("Test or development artifact in production build")
        if "bin" not in kinds:
            continue
        name = entry["target"]["name"]
        path = target / "release" / name
        executable = entry.get("executable")
        if (name not in expected or not executable or Path(executable).resolve() != path
                or not path.is_file() or entry["profile"]["opt_level"] == "0"):
            raise ValueError(f"Unexpected release binary {entry.get('executable')}")
        found.add(name)
    if not finished or found != expected:
        raise ValueError(f"Incomplete release build for {profile}: {sorted(found)}")


def verify_installed(root, targets):
    database = next((root / path for path in ("usr/lib/apk/db/installed", "lib/apk/db/installed",
                                              "var/lib/apk/db/installed") if (root / path).is_file()), None)
    if database is None:
        raise ValueError("Missing installed APK database")
    packages = {}
    for record in database.read_text().strip().split("\n\n"):
        fields = record.splitlines()
        package = next((line[2:] for line in fields if line.startswith("P:")), None)
        if package not in PACKAGES.values():
            continue
        files = set()
        directory = ""
        for line in fields:
            if line.startswith("F:"):
                directory = line[2:]
            elif line.startswith("R:"):
                files.add(str(Path(directory) / line[2:]))
        packages[package] = files
    for profile, binaries in BINARIES.items():
        actual = packages.get(PACKAGES[profile], set())
        expected = set(ASSETS[profile])
        for name, parent in binaries.items():
            rel = f"{parent}/{name}"
            expected.add(rel)
            directory = "hello-aarch64-musl" if profile == "hello" else "packages-aarch64-musl"
            built = targets / directory / "release" / name
            installed = root / rel
            if installed.is_symlink() or digest(built) != digest(installed):
                raise ValueError(f"Installed executable differs from release build: {rel}")
        if actual != expected:
            raise ValueError(f"{PACKAGES[profile]} package file mismatch: {sorted(actual ^ expected)}")


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").digest()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="operation", required=True)
    build = sub.add_parser("build")
    build.add_argument("profile", choices=BINARIES)
    build.add_argument("target", type=Path)
    installed = sub.add_parser("installed")
    installed.add_argument("root", type=Path)
    installed.add_argument("targets", type=Path)
    args = parser.parse_args()
    if args.operation == "build":
        verify_build(sys.stdin, args.profile, args.target.resolve())
    else:
        verify_installed(args.root, args.targets)
