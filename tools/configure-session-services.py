#!/usr/bin/env python3
"""Configure the shared device/login services in an offline Alpine image root."""

import argparse
import os
import pathlib
import stat


SERVICES = {
    "sysinit": ("devfs", "dmesg", "udev", "udev-trigger", "udev-settle"),
    "boot": ("modules",),
    "default": ("udev-postmount", "cgroups", "dbus", "networkmanager"),
}
OLD_DEVICE_SERVICES = ("mdev", "mdevd", "mdevd-init", "hwdrivers")
ELOGIND_SERVICE = "org.freedesktop.login1.service"
ELOGIND_ACTIVATION = {
    "Name=org.freedesktop.login1",
    "Exec=/usr/libexec/elogind/elogind --daemon",
    "User=root",
}


def require(condition, message):
    if not condition:
        raise SystemExit(f"FAIL: {message}")


def configure(root):
    root = root.resolve(strict=True)
    require(root != pathlib.Path("/"), "An offline image root is required")

    def target(name, follow_final=True):
        # Interpret absolute links inside the guest, never on the build host.
        pending = list(pathlib.PurePosixPath(name).parts)
        current = root
        links = 0
        while pending:
            part = pending.pop(0)
            if part in ("/", "."):
                continue
            if part == "..":
                require(current != root, f"Path escapes image root: {name}")
                current = current.parent
                continue
            candidate = current / part
            if candidate.is_symlink() and (pending or follow_final):
                links += 1
                require(links <= 40, f"Too many target symlinks: {name}")
                link = os.readlink(candidate)
                if link.startswith("/"):
                    current = root
                pending = list(pathlib.PurePosixPath(link).parts) + pending
            else:
                current = candidate
        return current

    require(target("/etc/alpine-release").read_text().strip().startswith("3.24."),
            "Alpine 3.24 is required")
    # Keep elogind's upstream activation path as its only default launcher.
    # pam_elogind creates the real session over this bus before the user starts.
    for folder in ("/etc/dbus-1/system-services", "/run/dbus-1/system-services",
                   "/usr/local/share/dbus-1/system-services"):
        override = target(f"{folder}/{ELOGIND_SERVICE}", follow_final=False)
        require(not override.exists() and not override.is_symlink(),
                f"Conflicting elogind activation override: {folder}")
    activation = target(f"/usr/share/dbus-1/system-services/{ELOGIND_SERVICE}",
                        follow_final=False)
    for path, mode in ((activation, 0o644),
                       (target("/usr/libexec/elogind/elogind", follow_final=False), 0o755)):
        info = path.lstat()
        require(stat.S_ISREG(info.st_mode) and info.st_uid == info.st_gid == 0
                and stat.S_IMODE(info.st_mode) == mode and info.st_nlink == 1,
                f"Missing or unsafe elogind activation asset: {path}")
    lines = [line.strip() for line in activation.read_text().splitlines()
             if line.strip() and not line.lstrip().startswith("#")]
    require(len(lines) == 4 and lines[0] == "[D-BUS Service]"
            and set(lines[1:]) == ELOGIND_ACTIVATION,
            "Unexpected vendor elogind D-Bus activation")
    links = []
    remove = []
    for level, names in SERVICES.items():
        for name in names:
            init = target(f"/etc/init.d/{name}")
            info = init.stat()
            require(stat.S_ISREG(info.st_mode) and info.st_uid == 0
                    and not info.st_mode & 0o022 and info.st_mode & 0o111,
                    f"Missing or unsafe OpenRC service: {name}")
            relative = f"/etc/runlevels/{level}/{name}"
            link = target(relative, follow_final=False)
            if link.is_symlink():
                require(target(relative) == init, f"Conflicting service link: {relative}")
            else:
                require(not link.exists(), f"Conflicting service file: {relative}")
            links.append((link, init))
    # Alpine's setup-devd removes the old device services when switching to
    # eudev. Elogind must also have no competing OpenRC autostart. Remove only
    # exact offline init-script links, preserving other services and files.
    for level in ("sysinit", "boot", "default"):
        for name in (*OLD_DEVICE_SERVICES, "elogind", "networking", "wpa_supplicant"):
            relative = f"/etc/runlevels/{level}/{name}"
            link = target(relative, follow_final=False)
            if link.is_symlink():
                require(target(relative) == target(f"/etc/init.d/{name}"),
                        f"Conflicting service link: {relative}")
                remove.append(link)
            else:
                require(not link.exists(), f"Conflicting service file: {relative}")
    for link, _ in links:
        parent = link.parent
        if parent.exists():
            info = parent.stat()
            require(stat.S_ISDIR(info.st_mode) and info.st_uid == 0
                    and not info.st_mode & 0o022, f"Unsafe runlevel directory: {parent}")
    for link in remove:
        link.unlink()
    for link, init in links:
        link.parent.mkdir(parents=True, exist_ok=True, mode=0o755)
        if not link.is_symlink():
            link.symlink_to(os.path.relpath(init, link.parent))
    print("PASS: offline dmesg, eudev, cgroups and D-Bus runlevels; "
          "vendor D-Bus activation owns elogind")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=pathlib.Path)
    args = parser.parse_args()
    configure(args.root)


if __name__ == "__main__":
    main()
