#!/usr/bin/env python3
"""Verify actual signed-package installation without starting a desktop or PAM."""

import argparse
import hashlib
import os
import pathlib
import re
import stat
import struct

from package_metadata import Catalog, GROUPS


def ensure(condition, message):
    if not condition:
        raise SystemExit(f"FAIL: {message}")


def elf_dependencies(data):
    ensure(len(data) >= 64 and data[:6] == b"\x7fELF\x02\x01", "Expected little-endian ELF64")
    ensure(struct.unpack_from("<H", data, 18)[0] == 183, "Executable is not aarch64")
    phoff = struct.unpack_from("<Q", data, 32)[0]
    size, count = struct.unpack_from("<HH", data, 54)
    ensure(size >= 56 and phoff + size * count <= len(data), "Invalid ELF program headers")
    headers = [struct.unpack_from("<IIQQQQQQ", data, phoff + i * size) for i in range(count)]
    interp = next((data[h[2]:h[2] + h[5]].rstrip(b"\0").decode()
                   for h in headers if h[0] == 3), None)
    ensure(interp == "/lib/ld-musl-aarch64.so.1", f"Unexpected ELF interpreter: {interp}")
    dynamic = next((h for h in headers if h[0] == 2), None)
    ensure(dynamic is not None, "Missing dynamic dependencies")
    tags = []
    for pos in range(dynamic[2], dynamic[2] + dynamic[5], 16):
        tag, value = struct.unpack_from("<qQ", data, pos)
        if tag == 0:
            break
        tags.append((tag, value))
    strings = next((v for t, v in tags if t == 5), None)
    ensure(strings is not None, "Missing ELF dynamic string table")
    load = next((h for h in headers if h[0] == 1 and h[3] <= strings < h[3] + h[5]), None)
    ensure(load is not None, "Unmapped ELF dynamic string table")
    offset = load[2] + strings - load[3]
    needed = []
    for tag, value in tags:
        if tag == 1:
            start = offset + value
            end = data.find(b"\0", start)
            ensure(0 <= start < end <= len(data), "Invalid ELF library name")
            name = data[start:end].decode("ascii")
            ensure("/" not in name and name not in (".", ".."), "Unexpected dependency path")
            needed.append(name)
    return interp, needed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=pathlib.Path, required=True)
    parser.add_argument("--mode", choices=("graphics", "graphics_minimal", "console"), default="graphics")
    parser.add_argument("--source", type=pathlib.Path, default=pathlib.Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    ensure(root != pathlib.Path("/"), "An isolated target rootfs is required")

    def target(name, follow_final=True):
        # Resolve links as the target rootfs would, including /lib -> /usr/lib.
        # pathlib.resolve() would incorrectly follow an absolute guest link on
        # the build host. Never read outside the explicitly supplied root.
        pending = list(pathlib.PurePosixPath(name).parts)
        current = root
        links = 0
        while pending:
            part = pending.pop(0)
            if part in ("/", "."):
                continue
            if part == "..":
                ensure(current != root, f"Path escapes isolated root: {name}")
                current = current.parent
                continue
            candidate = current / part
            if candidate.is_symlink() and (pending or follow_final):
                links += 1
                ensure(links <= 40, f"Too many target symlinks: {name}")
                link = os.readlink(candidate)
                if link.startswith("/"):
                    current = root
                pending = list(pathlib.PurePosixPath(link).parts) + pending
            else:
                current = candidate
        return current

    def owned(name, mode):
        path = target(name, follow_final=False)
        info = path.lstat()
        ensure(stat.S_ISREG(info.st_mode), f"Not a regular file: {name}")
        ensure(info.st_uid == 0 and info.st_gid == 0, f"Not root-owned: {name}")
        ensure(stat.S_IMODE(info.st_mode) == mode, f"Wrong permissions for {name}")
        return path

    database = next((target(p) for p in ("/usr/lib/apk/db/installed", "/lib/apk/db/installed",
                                         "/var/lib/apk/db/installed") if target(p).is_file()), None)
    ensure(database is not None, "Installed APK database is missing")
    packages = {}
    dependencies = {}
    audio_helpers = {"wpctl": "wireplumber", "pw-dump": "pipewire-tools",
                     "pipewire": "pipewire", "wireplumber": "wireplumber",
                     "pipewire-pulse": "pipewire-pulse", "flock": "flock"}
    audio_owners = {name: [] for name in audio_helpers}
    radio_helper = "/usr/libexec/alcard/alcard-radio-helper"
    radio_assets = {
        "/usr/share/polkit-1/actions/org.alcard.radio.policy": ("scripts/radios/org.alcard.radio.policy", 0o644),
        "/etc/dbus-1/system-services/org.bluez.service": ("scripts/radios/org.bluez.service", 0o644),
        "/etc/alcard/bluetooth.conf": ("scripts/radios/bluetooth.conf", 0o644),
    }
    base_radio = {path for path in radio_assets if path.startswith("/etc/")}
    radio_owners = {name: [] for name in (radio_helper, *radio_assets)}
    for entry in re.split(r"\n\s*\n", database.read_text().strip()):
        fields = dict(line.split(":", 1) for line in entry.splitlines() if len(line) > 2 and line[1] == ":")
        if "P" in fields and "V" in fields:
            packages[fields["P"]] = fields["V"]
            dependencies[fields["P"]] = fields.get("D", "").split()
            # APK's installed-file database pairs F:directory with R:filename.
            # A binary present outside its vendor package is not sufficient.
            directory = None
            for line in entry.splitlines():
                if line.startswith("F:"):
                    directory = line[2:]
                elif line.startswith("R:") and directory is not None:
                    if directory == "usr/bin" and line[2:] in audio_owners:
                        audio_owners[line[2:]].append(fields["P"])
                    filename = "/" + directory.strip("/") + "/" + line[2:]
                    if filename in radio_owners:
                        radio_owners[filename].append(fields["P"])
    catalog = Catalog(args.source)
    expected = catalog.versions(GROUPS["base"])
    graphical = catalog.versions(GROUPS["graphical"])
    shell = catalog.versions(GROUPS["shell"])
    if args.mode != "console":
        expected.update(graphical)
        for name in ("labwc", "foot", "dbus", "elogind", "greetd", "greetd-agreety",
                     "eudev", "eudev-openrc", "udev-init-scripts-openrc"):
            ensure(name in packages, f"Runtime package missing: {name}")
    if args.mode == "graphics":
        expected.update(shell)
        ensure("setpriv" in packages, "Session parent-death helper package missing: setpriv")
        for owner, required in {
            "alcard-shell": {"wireplumber", "pipewire-tools", "networkmanager-cli", "bluez", "dbus", "polkit-elogind"},
            "alcard-desktop-shell": {"pipewire", "wireplumber", "pipewire-pulse", "pipewire-alsa", "flock"},
        }.items():
            declared = {re.split(r"[<>=~]", dep, maxsplit=1)[0] for dep in dependencies.get(owner, [])}
            ensure(required <= declared, f"Installed {owner} misses runtime dependencies: {sorted(required - declared)}")
            ensure(required <= packages.keys(), f"Installed runtime packages missing: {sorted(required - packages.keys())}")
        ensure("dbus>=1.16.2" in dependencies.get("alcard-shell", []),
               "Shell must require D-Bus with /etc/system-services precedence")
        for name, owner in audio_helpers.items():
            ensure(audio_owners[name] == [owner],
                   f"Expected {owner} to own /usr/bin/{name}, got {audio_owners[name]}")
        for name, owners in radio_owners.items():
            owner = "alcard-hello" if name in base_radio else "alcard-shell"
            ensure(owners == [owner], f"Expected {owner} to own {name}, got {owners}")
        owned("/usr/bin/nmcli", 0o755)
        owned("/usr/lib/bluetooth/bluetoothd", 0o755)
        for name in ("linux-pam", "polkit-elogind", "polkit-elogind-libs", "polkit-common"):
            ensure(name in packages, f"Authentication package missing: {name}")
    if args.mode == "console":
        ensure(not (set(shell) | set(graphical) | {"greetd", "greetd-agreety"}) & packages.keys(),
               "Dedicated graphics packages remain in Console")
    for name, version in expected.items():
        ensure(packages.get(name) == version, f"Expected {name}={version}, got {packages.get(name)}")
    print("PASS: installed versions " + ", ".join(f"{p}={v}" for p, v in sorted(expected.items())))
    for binary in ("/usr/bin/alcard-settings", "/usr/sbin/alcard-setupd"):
        owned(binary, 0o700 if binary.endswith("setupd") else 0o755)
    for pkg in ("dbus", "networkmanager", "networkmanager-cli", "networkmanager-wifi", "networkmanager-openrc", "bluez"):
        ensure(pkg in packages, f"Missing shared settings runtime: {pkg}")
    owned("/usr/bin/nmcli", 0o755)
    for path in base_radio:
        source, mode = radio_assets[path]
        ensure(radio_owners[path] == ["alcard-hello"] and owned(path, mode).read_bytes() == (args.source/source).read_bytes(), f"Invalid base radio asset: {path}")
    if args.mode == "console":
        print("PASS: Console retains Hello without dedicated graphical packages")
        return

    # Distribution APK scripts create messagebus/greetd/polkitd; Alcard's
    # offline helper adds the separate greeter. Installing desktop packages
    # before baselayout can silently skip their adduser/addgroup calls while
    # APK still reports success.
    passwd = [row.split(":") for row in target("/etc/passwd").read_text().splitlines()]
    groups = [row.split(":") for row in target("/etc/group").read_text().splitlines()]
    service_accounts = ["messagebus", "greetd", "greeter"]
    if args.mode == "graphics":
        service_accounts.append("polkitd")
    for name in service_accounts:
        users = [row for row in passwd if row[0] == name]
        matching_groups = [row for row in groups if row[0] == name]
        ensure(len(users) == 1 and len(users[0]) == 7,
               f"Distribution service account missing or ambiguous: {name}")
        ensure(len(matching_groups) == 1 and len(matching_groups[0]) == 4,
               f"Distribution service group missing or ambiguous: {name}")
        user, group = users[0], matching_groups[0]
        ensure(user[2].isdigit() and user[3].isdigit() and group[2].isdigit()
               and 0 < int(user[2]) < 1000 and int(user[3]) == int(group[2]) > 0,
               f"Invalid distribution service UID/GID: {name}")
    print("PASS: login service accounts " + ", ".join(service_accounts))
    greeter_shadow = [row.split(":") for row in target("/etc/shadow").read_text().splitlines()
                      if row.startswith("greeter:")]
    ensure(len(greeter_shadow) == 1 and len(greeter_shadow[0]) == 9,
           "Greeter shadow entry missing or ambiguous")
    entry = greeter_shadow[0]
    ensure(entry[1].startswith(("!", "*")) and entry[2] in ("", "-1")
           and entry[7] in ("", "-1"),
           "Greeter must be password-locked with password aging and account expiry disabled")
    ensure(next(row for row in passwd if row[0] == "greeter")[6] == "/sbin/nologin",
           "Greeter must have a non-login shell")
    ensure(not any(row[0] in ("wheel", "video", "input") and "greeter" in row[3].split(",")
                   for row in groups if len(row) == 4),
           "Greeter must not have privileged supplementary groups")
    print("PASS: locked greeter is eligible for PAM account management")

    assets = {
        "/usr/libexec/alcard/wayland-session": ("scripts/wayland/wayland-session", 0o755),
        "/etc/alcard/foot.ini": ("scripts/wayland/foot.ini", 0o644),
        "/etc/greetd/alcard.toml": ("scripts/hello/alcard-greetd.toml", 0o644),
        "/etc/pam.d/alcard-greetd": ("scripts/pam/alcard-greetd.alpine", 0o644),
    }
    for stack in ("greetd", "base-auth", "base-account", "base-password",
                  "base-session", "base-session-noninteractive"):
        ensure(any(target(folder + stack).is_file() for folder in ("/etc/pam.d/", "/usr/lib/pam.d/")),
               f"Distribution login PAM stack missing: {stack}")
    ensure(any(target(path).is_file() for path in ("/usr/lib/security/pam_elogind.so",
                                                   "/lib/security/pam_elogind.so")),
           "The required elogind PAM module is missing")
    for name in ("udev", "udev-trigger", "udev-settle", "udev-postmount", "cgroups", "dbus"):
        info = target(f"/etc/init.d/{name}").stat()
        ensure(stat.S_ISREG(info.st_mode) and info.st_uid == 0
               and not info.st_mode & 0o022 and info.st_mode & 0o111,
               f"Missing or unsafe device/login service: {name}")
    # An installed elogind-openrc subpackage is harmless, but enabling it as
    # well as the vendor activation command creates two competing launchers.
    for level in ("sysinit", "boot", "default"):
        link = target(f"/etc/runlevels/{level}/elogind", follow_final=False)
        ensure(not link.exists() and not link.is_symlink(),
               f"Competing elogind OpenRC autostart: {level}/elogind")
    service_name = "org.freedesktop.login1.service"
    for folder in ("/etc/dbus-1/system-services", "/run/dbus-1/system-services",
                   "/usr/local/share/dbus-1/system-services"):
        override = target(f"{folder}/{service_name}", follow_final=False)
        ensure(not override.exists() and not override.is_symlink(),
               f"Conflicting elogind activation override: {folder}")
    activation = owned(f"/usr/share/dbus-1/system-services/{service_name}", 0o644)
    daemon = owned("/usr/libexec/elogind/elogind", 0o755)
    ensure(activation.stat().st_nlink == daemon.stat().st_nlink == 1,
           "Elogind activation assets must not have hard links")
    lines = [line.strip() for line in activation.read_text().splitlines()
             if line.strip() and not line.lstrip().startswith("#")]
    ensure(len(lines) == 4 and lines[0] == "[D-BUS Service]" and set(lines[1:]) == {
        "Name=org.freedesktop.login1",
        "Exec=/usr/libexec/elogind/elogind --daemon",
        "User=root",
    }, "Unexpected vendor elogind D-Bus activation")
    print("PASS: vendor D-Bus activation owns elogind; no competing OpenRC autostart")
    if args.mode == "graphics":
        owned("/bin/setpriv", 0o755)
        assets.update({
            "/usr/libexec/alcard/shell-session": ("scripts/wayland/shell-session", 0o755),
            "/usr/libexec/alcard/session-client": ("scripts/wayland/session-client", 0o755),
            "/usr/libexec/alcard/audio-session": ("scripts/wayland/audio-session", 0o755),
            "/etc/alcard/labwc-shell/rc.xml": ("scripts/wayland/rc.xml", 0o644),
            "/etc/alcard/labwc-shell/environment": ("scripts/wayland/environment", 0o644),
            "/etc/alcard/labwc-shell/autostart": ("scripts/wayland/autostart", 0o755),
            "/etc/pam.d/alcard-lock": ("scripts/pam/alcard-lock.alpine", 0o644),
        })
        assets.update(radio_assets)
        for name in ("alcard-shell", "alcard-radio-helper", "alcard-polkit-agent", "alcard-lock", *audio_helpers):
            if name == "pipewire-pulse":
                # Upstream 1.6 installs this alias; Alpine moves the link to
                # pipewire-pulse while its target remains owned by pipewire.
                alias = target("/usr/bin/pipewire-pulse", follow_final=False)
                info = alias.lstat()
                ensure(stat.S_ISLNK(info.st_mode) and info.st_uid == 0 and info.st_gid == 0
                       and os.readlink(alias) == "pipewire",
                       "Expected root-owned pipewire-pulse -> pipewire vendor alias")
                path = owned("/usr/bin/pipewire", 0o755)
            else:
                path = owned(radio_helper if name == "alcard-radio-helper" else f"/usr/bin/{name}", 0o755)
            data = path.read_bytes()
            interp, needed = elf_dependencies(data)
            ensure(target(interp).is_file(), f"Missing interpreter for {name}")
            for library in needed:
                ensure(any(target(folder + library).is_file() for folder in ("/lib/", "/usr/lib/")),
                       f"Missing shared library for {name}: {library}")
            print(f"PASS: {name} aarch64/musl sha256={hashlib.sha256(data).hexdigest()} libraries={','.join(needed)}")
        for stack in ("base-auth", "base-account"):
            ensure(any(target(folder + stack).is_file() for folder in ("/etc/pam.d/", "/usr/lib/pam.d/")),
                   f"Distribution PAM stack missing: {stack}")
        pam_helper = next((target(p) for p in ("/usr/sbin/unix_chkpwd", "/sbin/unix_chkpwd")
                           if target(p).is_file()), None)
        ensure(pam_helper is not None, "Distribution unix_chkpwd helper missing")
        helper_info = pam_helper.stat()
        shadow_gid = next((int(row.split(":")[2]) for row in target("/etc/group").read_text().splitlines()
                           if row.startswith("shadow:")), None)
        # Alpine grants the helper access through setgid shadow; other
        # distribution revisions may use setuid root. Preserve its shipped
        # privilege transition instead of demanding one implementation.
        transition = bool(helper_info.st_mode & stat.S_ISUID) or (
            bool(helper_info.st_mode & stat.S_ISGID) and shadow_gid is not None
            and helper_info.st_gid == shadow_gid)
        ensure(helper_info.st_uid == 0 and not helper_info.st_mode & 0o022 and transition,
               "Distribution unix_chkpwd lost its root/setgid-shadow transition")
        print(f"PASS: unix_chkpwd mode={stat.S_IMODE(helper_info.st_mode):04o} uid={helper_info.st_uid} gid={helper_info.st_gid}")
        owned("/usr/lib/polkit-1/polkit-agent-helper-1", 0o4755)
        ensure(target("/usr/lib/polkit-1/polkitd").is_file(), "polkitd missing")
        ensure(target("/etc/pam.d/polkit-1").is_file(), "Distribution polkit PAM policy missing")
    for installed, (source, mode) in assets.items():
        data = owned(installed, mode).read_bytes()
        ensure(data == (args.source / source).read_bytes(),
               f"Installed asset differs from current package source: {installed}")
        if installed in radio_assets:
            print(f"PASS: {installed} package={radio_owners[installed][0]} mode={mode:04o} sha256={hashlib.sha256(data).hexdigest()}")
    print(f"PASS: {len(assets)} installed session/PAM assets and ownership; mode={args.mode}")
    print("No compositor, PAM authentication, login or power action was started.")


if __name__ == "__main__":
    main()
