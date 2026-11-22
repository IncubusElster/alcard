#!/usr/bin/env python3
"""Ensure a dedicated unprivileged greetd greeter account in an offline Alpine rootfs.

Never run against '/': this edits only the image builder's disposable rootfs.
Do not replace an existing account or group, and never grant greeter any privileges.
"""
from pathlib import Path
import sys


def entries(path):
    return path.read_text().splitlines()


def ensure(root):
    root = root.resolve(strict=True)
    if root == Path("/"):
        raise SystemExit("Refusing to modify the host root filesystem")
    etc = root / "etc"
    passwd = etc / "passwd"
    group = etc / "group"
    shadow = etc / "shadow"
    gshadow = etc / "gshadow"
    if not all(path.is_file() for path in (passwd, group, shadow)):
        raise SystemExit("Alpine account database missing")
    pw = entries(passwd)
    gp = entries(group)
    sh = entries(shadow)
    if any(row.split(":")[0] == "greeter" for row in pw):
        if not any(row.split(":")[0] == "greeter" for row in gp):
            raise SystemExit("Existing greeter account has no matching group")
        print("==> greetd greeter account already present")
        return
    if any(row.split(":")[0] == "greeter" for row in gp):
        raise SystemExit("Existing greeter group without account: manual review required")
    used_uids = set()
    for row in pw:
        fields = row.split(":")
        if len(fields) >= 4 and fields[2].isdigit():
            used_uids.add(int(fields[2]))
    used_gids = set()
    for row in gp:
        fields = row.split(":")
        if len(fields) >= 3 and fields[2].isdigit():
            used_gids.add(int(fields[2]))
    uid = next((n for n in range(200, 999) if n not in used_uids and n not in used_gids), None)
    if uid is None:
        raise SystemExit("No free system UID/GID for greeter")
    # No group membership beyond the dedicated matching group.
    pw.append(f"greeter:x:{uid}:{uid}:greetd greeter:/var/empty:/sbin/nologin")
    gp.append(f"greeter:x:{uid}:")
    # This locked service account has no password lifecycle. lastchg=0 would
    # require a password change: greetd runs pam_acct_mgmt even for its greeter,
    # so pam_unix would reject it before agreety can display a login prompt.
    # Empty aging fields disable expiration without enabling password login.
    sh.append("greeter:!:::::::")
    passwd.write_text("\n".join(pw) + "\n")
    group.write_text("\n".join(gp) + "\n")
    shadow.write_text("\n".join(sh) + "\n")
    if gshadow.is_file():
        gs = entries(gshadow)
        gs.append("greeter:!::")
        gshadow.write_text("\n".join(gs) + "\n")
    (root / "var" / "empty").mkdir(parents=True, exist_ok=True)
    print(f"==> Created offline greetd greeter system UID/GID {uid}")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("Usage: ensure-greetd-user.py /path/to/disposable/rootfs")
    ensure(Path(sys.argv[1]))
