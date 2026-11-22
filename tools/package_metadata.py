#!/usr/bin/env python3
"""Read current package identities from APKBUILD without executing shell code.

Only literal package fields and the explicitly supported kernel substitutions
are accepted. Builders, verifiers and CI share this reader; the recipes remain
the authority for versions and dependency constraints.
"""

import argparse
from dataclasses import dataclass
import json
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[1]
GROUPS = {
    "base": ("alcard-hello", "alcard-keyboard-zero", "alcard-fonts-console"),
    "graphical": ("alcard-fonts-gui", "alcard-desktop-minimal"),
    "shell": ("alcard-shell", "alcard-polkit-agent", "alcard-lock", "alcard-desktop-shell"),
    "session-roots": ("alcard-hello", "alcard-keyboard-zero", "alcard-fonts-console",
                      "alcard-desktop-minimal", "alcard-desktop-shell"),
}
GROUPS["user"] = GROUPS["base"] + GROUPS["graphical"] + GROUPS["shell"]


def literal(text, key, required=True):
    """Parse one top-level literal assignment, including quoted multi-line lists."""
    starts = list(re.finditer(rf"^{re.escape(key)}=", text, re.M))
    if not starts and not required:
        return ""
    if len(starts) != 1:
        raise ValueError(f"Expected one {key} assignment")
    rest = text[starts[0].end():]
    if rest.startswith(('"', "'")):
        quote = rest[0]
        end = rest.find(quote, 1)
        if end < 0 or rest[end + 1:].split("\n", 1)[0].strip():
            raise ValueError(f"Unsupported quoted {key} assignment")
        value = rest[1:end]
    else:
        value = rest.split("\n", 1)[0].strip()
    if any(token in value for token in ("`", "$(", "\\")):
        raise ValueError(f"Shell expressions are not allowed in {key}")
    return value


@dataclass(frozen=True)
class Package:
    name: str
    pkgver: str
    pkgrel: str
    dependencies: tuple[str, ...]
    build_dependencies: tuple[str, ...]

    @property
    def version(self):
        return f"{self.pkgver}-r{self.pkgrel}"

    @property
    def pin(self):
        return f"{self.name}={self.version}"

    @property
    def filename(self):
        return f"{self.name}-{self.version}.apk"


class Catalog:
    def __init__(self, root=ROOT):
        self.root = Path(root).resolve()
        self.packages = {}
        self.kernel_release = None
        for recipe in sorted((self.root / "packaging/apk").glob("*/APKBUILD")):
            try:
                text = recipe.read_text(encoding="utf-8")
                name, version, revision = (literal(text, key) for key in
                                           ("pkgname", "pkgver", "pkgrel"))
                if name != recipe.parent.name or not re.fullmatch(r"[a-z0-9][a-z0-9+_.-]*", name):
                    raise ValueError("Package name does not match its recipe directory")
                if not re.fullmatch(r"[0-9][A-Za-z0-9.+_~]*", version):
                    raise ValueError("Unsupported package version")
                if not re.fullmatch(r"[0-9]+", revision):
                    raise ValueError("Invalid package revision")
                variables = {"pkgver": version, "pkgrel": revision}
                if name == "alcard-kmods-rpi":
                    self.kernel_release = literal(text, "_kernrel")
                    if not re.fullmatch(r"[0-9]+", self.kernel_release):
                        raise ValueError("Invalid Alpine kernel revision")
                    variables["_kernrel"] = self.kernel_release

                def dependencies(key):
                    value = literal(text, key, required=False)
                    for variable, replacement in variables.items():
                        value = value.replace("${" + variable + "}", replacement)
                    if "$" in value or not re.fullmatch(r"[A-Za-z0-9+_.~<>=!:/\s-]*", value):
                        raise ValueError(f"Unsupported expression in {key}")
                    return tuple(value.split())

                self.packages[name] = Package(name, version, revision,
                                              dependencies("depends"), dependencies("makedepends"))
            except ValueError as error:
                raise ValueError(f"{recipe.relative_to(self.root)}: {error}") from error
        if "alcard-kmods-rpi" not in self.packages or self.kernel_release is None:
            raise ValueError("Missing kernel package metadata")
        for group, names in GROUPS.items():
            if not set(names) <= self.packages.keys():
                raise ValueError(f"Missing recipes for package group {group}")
        self.validate_pins()

    @property
    def kernel_version(self):
        return f"{self['alcard-kmods-rpi'].pkgver}-r{self.kernel_release}"

    @property
    def kernel_abi(self):
        return f"{self['alcard-kmods-rpi'].pkgver}-{self.kernel_release}-rpi"

    def __getitem__(self, name):
        try:
            return self.packages[name]
        except KeyError as error:
            raise ValueError(f"Unknown Alcard package: {name}") from error

    def versions(self, names):
        return {name: self[name].version for name in names}

    def validate_pins(self):
        expected = self.versions(self.packages)
        expected.update({"linux-rpi": self.kernel_version, "linux-rpi-dev": self.kernel_version})
        for package in self.packages.values():
            for dependency in package.dependencies + package.build_dependencies:
                match = re.fullmatch(r"([a-z0-9+_.-]+)=([^=]+)", dependency)
                if match and match[1] in expected and match[2] != expected[match[1]]:
                    raise ValueError(f"{package.name}: stale dependency {dependency}; "
                                     f"expected {match[1]}={expected[match[1]]}")

    def manifest(self):
        return {"schema": 1, "kernel": {"version": self.kernel_version, "abi": self.kernel_abi},
                "packages": {name: {"version": package.version, "pkgver": package.pkgver,
                                     "pkgrel": package.pkgrel, "filename": package.filename,
                                     "dependencies": package.dependencies,
                                     "build_dependencies": package.build_dependencies}
                             for name, package in sorted(self.packages.items())},
                "groups": GROUPS}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT, help="Source checkout containing packaging/apk")
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("version", "pkgver", "filename", "pin"):
        command = commands.add_parser(name)
        command.add_argument("packages", nargs="*")
        command.add_argument("--group", choices=sorted(GROUPS))
    group = commands.add_parser("group")
    group.add_argument("name", choices=sorted(GROUPS))
    for name in ("kernel-version", "kernel-abi", "manifest", "check"):
        commands.add_parser(name)
    args = parser.parse_args()
    try:
        catalog = Catalog(args.root)
        if args.command == "manifest":
            print(json.dumps(catalog.manifest(), indent=2))
        elif args.command == "check":
            print(f"PASS: {len(catalog.packages)} package identities and exact dependency pins")
        elif args.command == "group":
            print("\n".join(GROUPS[args.name]))
        elif args.command == "kernel-version":
            print(catalog.kernel_version)
        elif args.command == "kernel-abi":
            print(catalog.kernel_abi)
        else:
            if bool(args.packages) == bool(args.group):
                raise ValueError("Provide package names or one --group")
            names = GROUPS[args.group] if args.group else args.packages
            print("\n".join(getattr(catalog[name], args.command) for name in names))
    except (OSError, ValueError) as error:
        parser.exit(1, f"ERROR: {error}\n")


if __name__ == "__main__":
    main()
