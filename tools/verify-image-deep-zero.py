#!/usr/bin/env python3
"""Deep, read-only Alcard image verification (no loop/mount, no hardware access).

Extracts the fourth MBR partition into a disposable regular file, checks ext4
with e2fsck, reads files with debugfs, and inspects the FAT initramfs archive.
Runs on the Alpine ARM64 image-builder container, not on Cardputer hardware.
"""
import argparse
import gzip
import hashlib
import json
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import uuid

from package_metadata import Catalog, GROUPS

SECTOR = 512
CATALOG = Catalog()
ABI = CATALOG.kernel_abi
KERNEL_PKG = "linux-rpi=" + CATALOG.kernel_version
MODULES = (
    "m5ioe1", "gpio-forwarder", "tca8418_keypad_m5stack",
    "pwm_bl_m5stack", "es8389_m5stack", "cardputerzero-audio",
    "simple-amplifier", "bq27xxx_battery", "bq27xxx_battery_i2c",
    "bmi270_core", "bmi270_i2c", "bmc150_magn_bmi270",
)


class VerificationError(Exception):
    pass


def ensure(condition, message):
    if not condition:
        raise VerificationError(message)


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


def run(args, **kwargs):
    result = subprocess.run(args, capture_output=True, **kwargs)
    if result.returncode:
        message = result.stderr.decode(errors="replace").strip()
        raise VerificationError(
            f"{' '.join(str(a) for a in args[:3])}: {message or 'exit ' + str(result.returncode)}"
        )
    return result.stdout


def get_partitions(image):
    with image.open("rb") as file:
        sector = file.read(SECTOR)
    ensure(len(sector) == SECTOR and sector[510:] == b"\x55\xaa", "Invalid MBR")
    disk_id = struct.unpack_from("<I", sector, 440)[0]
    parts = []
    for i, kind in enumerate((0x0C, 0x0C, 0x0C, 0x83)):
        offset = 446 + 16 * i
        ensure(sector[offset + 4] == kind, f"Wrong partition type #{i + 1}")
        start, length = struct.unpack_from("<II", sector, offset + 8)
        ensure(start >= 2048 and length and start % 2048 == 0,
               f"Invalid partition #{i + 1} alignment")
        parts.append((start * SECTOR, length * SECTOR))
    for i in range(3):
        ensure(parts[i][0] + parts[i][1] <= parts[i + 1][0],
               "Overlapping MBR partitions")
    ensure(parts[-1][0] + parts[-1][1] <= image.stat().st_size,
           "Truncated partition table")
    return disk_id, parts


def extract_partition(image, start, length, target):
    with image.open("rb") as inp, target.open("xb") as output:
        inp.seek(start)
        left = length
        while left:
            chunk = inp.read(min(left, 1024 * 1024))
            ensure(chunk, "Truncated ext4 partition")
            output.write(chunk)
            left -= len(chunk)


def fsck(ext4):
    result = subprocess.run(
        ["e2fsck", "-fn", str(ext4)],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    ensure(result.returncode == 0,
           f"ext4 e2fsck -fn failed (exit {result.returncode}): "
           + (result.stdout + result.stderr).decode(errors="replace")[-2400:])
    print("PASS: ext4 e2fsck -fn (no changes)")


class Ext4:
    """Read ext4 using debugfs, resolving Linux symlinks component-by-component.

    debugfs path lookup is not normal VFS lookup: intermediate symlinks in a
    merged-/usr Alpine filesystem must be expanded explicitly. No writes.
    """
    def __init__(self, path):
        self.path = path
        self._resolved = {}

    def _debugfs(self, command):
        result = subprocess.run(
            ["debugfs", "-R", command, str(self.path)],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        ensure(result.returncode == 0,
               f"debugfs failed for {command}: {result.stderr.decode(errors='replace')}")
        return result.stdout, result.stderr.decode(errors="replace")

    def _raw_stat(self, path):
        stdout, stderr = self._debugfs("stat " + path)
        exists = b"Inode:" in stdout and "not found" not in stderr.lower()
        return (stdout.decode(errors="replace"), exists)

    def _resolve(self, path):
        import posixpath
        ensure(path.startswith("/"), f"Expected absolute ext4 path: {path}")
        if path in self._resolved:
            return self._resolved[path]
        pending = [p for p in posixpath.normpath(path).split("/") if p]
        resolved = []
        links = 0
        while pending:
            component = pending.pop(0)
            candidate = "/" + "/".join(resolved + [component])
            metadata, found = self._raw_stat(candidate)
            if not found:
                return None
            if re.search(r"\bType:\s*symlink\b", metadata):
                links += 1
                ensure(links <= 32, f"Symlink cycle or excessive depth: {path}")
                match = re.search(r'Fast link dest:\s*"([^"]+)"', metadata)
                if match:
                    target = match.group(1)
                else:
                    raw, err = self._debugfs("cat " + candidate)
                    ensure("not found" not in err.lower(),
                           f"Cannot read symlink {candidate}")
                    target = raw.decode("utf-8").rstrip("\x00\n")
                ensure(target != "", f"Empty ext4 symlink: {candidate}")
                if target.startswith("/"):
                    expanded = target
                else:
                    expanded = posixpath.join("/" + "/".join(resolved), target)
                expanded = posixpath.normpath(expanded)
                pending = [p for p in expanded.split("/") if p] + pending
                resolved = []
            else:
                resolved.append(component)
        canonical = "/" + "/".join(resolved)
        self._resolved[path] = canonical
        return canonical

    def stat(self, path, required=True):
        canonical = self._resolve(path)
        if canonical is None:
            if required:
                raise VerificationError(
                    f"Missing ext4 path: {path} (after Linux symlink resolution). "
                    "If this is modules.dep, rebuild the image with the fixed "
                    "depmod -m /usr/lib/modules worker; old images cannot be "
                    "repaired by a read-only verifier.")
            return None
        stdout, found = self._raw_stat(canonical)
        ensure(found, f"Missing ext4 inode: {path}")
        return stdout

    def lstat(self, path, required=True):
        # Resolve only parents, so dangling or substituted final symlinks are
        # visible to service-policy checks instead of appearing absent.
        parent, _, name = path.rpartition("/")
        canonical = self._resolve(parent or "/")
        if canonical is not None:
            metadata, found = self._raw_stat(canonical.rstrip("/") + "/" + name)
            if found:
                return metadata
        if required:
            raise VerificationError(f"Missing ext4 inode: {path}")
        return None

    def cat(self, path):
        canonical = self._resolve(path)
        ensure(canonical is not None, f"Missing ext4 file: {path}")
        data, stderr = self._debugfs("cat " + canonical)
        ensure("not found" not in stderr.lower() and "invalid inode" not in stderr.lower(),
               f"Unable to read ext4 file {path}: {stderr.strip()}")
        return data

    def text(self, path):
        self.stat(path)
        return self.cat(path).decode("utf-8")


def root_metadata(ext4, manifest, disk_id, profile):
    with ext4.open("rb") as inp:
        inp.seek(1024 + 0x68)
        root_uuid = uuid.UUID(bytes=inp.read(16))
        inp.seek(1024 + 0x78)
        label = inp.read(16).rstrip(b"\0")
    ensure(label == b"ALCARDROOT", f"Unexpected ext4 label {label!r}")
    ensure(manifest.get("root_uuid") == str(root_uuid), "ext4 UUID mismatches manifest")
    ensure(manifest.get("root_partuuid") == f"{disk_id:08x}-04",
           "Root PARTUUID mismatches MBR")
    ensure(manifest.get("profile") == profile, "Image profile mismatches manifest")
    ensure(len(manifest.get("partitions", [])) == 4, "Invalid manifest partitions")
    print(f"PASS: ext4 label/UUID, manifest profile and PARTUUID {disk_id:08x}-04")


def fat_text(image, boot_start, path):
    return run(["mtype", "-i", f"{image}@@{boot_start}", f"::{path}"]).decode("utf-8")


def fat_copy(image, boot_start, path, output):
    run(["mcopy", "-n", "-i", f"{image}@@{boot_start}", f"::{path}", str(output)])
    ensure(output.is_file() and output.stat().st_size > 0,
           f"FAT payload extraction failed: {path}")


def profile_valid(text, profile):
    overlays = [s.strip() for s in text.splitlines() if s.strip().startswith("dtoverlay=")]
    ensure(overlays.count("dtoverlay=alcard-zero-v5") == 1,
           "Main LCD/board DTBO not selected exactly once")
    ensure(overlays.count("dtoverlay=alcard-bq27220-v5") == 1,
           "Battery DTBO not selected exactly once")
    imu = "dtoverlay=alcard-bmi270-bmm150"
    if profile == "v5":
        ensure(overlays.count(imu) == 1, "Full v5 must enable BMI270/BMM150")
        ensure("dtoverlay=alcard-bmi270-only" not in overlays,
               "Conflicting v5 IMU overlay")
    else:
        ensure(not any(x.startswith("dtoverlay=alcard-bmi270") or
                       x.startswith("dtoverlay=imx219") for x in overlays),
               "Lite enables absent IMU/camera")


def check_boot(image, parts, profile, disk_id, fs, temp):
    configs = []
    for i, (start, _) in enumerate(parts[:3], 1):
        config = fat_text(image, start, "/config.txt")
        cmdline = fat_text(image, start, "/cmdline.txt")
        board = fat_text(image, start, "/alcard.cfg")
        ensure(config.count("include alcard.cfg") == 1,
               f"FAT #{i}: missing/duplicate Alcard include")
        ensure("tryboot_a_b=1" not in config and "autoboot.txt" not in config,
               f"FAT #{i}: premature A/B enablement")
        ensure(f"root=PARTUUID={disk_id:08x}-04" in cmdline,
               f"FAT #{i}: wrong root PARTUUID")
        ensure("rootfstype=ext4" in cmdline and "rootwait" in cmdline,
               f"FAT #{i}: incomplete ext4/rootwait options")
        profile_valid(board, profile)
        configs.append((config, cmdline, board))
    ensure(configs[0] == configs[1] == configs[2],
           "BOOT FAT partitions have divergent text configuration")
    ensure(fs.text("/boot/alcard.cfg") == configs[0][2],
           "Selected FAT profile does not match APK-owned /boot/alcard.cfg in ROOT")
    initramfs = temp / "initramfs-rpi"
    fat_copy(image, parts[1][0], "/initramfs-rpi", initramfs)
    print("PASS: 3 FAT boot configurations, mutually exclusive board profile, root PARTUUID")
    return initramfs


def inspect_root(fs, profile):
    world = set(fs.text("/etc/apk/world").splitlines())
    ensure(KERNEL_PKG in world, "Kernel not pinned in /etc/apk/world")
    ensure("alpine-base" in world, "Missing alpine-base in world")
    active = "alcard-board-zero-v5" + ("-lite" if profile == "v5-lite" else "")
    inactive = "alcard-board-zero-v5" + ("" if profile == "v5-lite" else "-lite")
    ensure(active in world and inactive not in world,
           "Wrong board profile in /etc/apk/world")
    for name in ("etc/inittab", "etc/fstab", "etc/modules", "etc/passwd",
                 "etc/shadow", "etc/apk/repositories", "sbin/init"):
        fs.stat("/" + name)
    inittab = fs.text("/etc/inittab")
    ensure(inittab.count("tty1::respawn:") == 1,
           "tty1 must have exactly one inittab owner")
    ensure(re.search(r"^tty1::respawn:/usr/libexec/alcard/tty1-dispatch\s*$", inittab, re.M),
           "tty1 must dispatch firstboot then greetd/getty; direct getty is obsolete")
    ensure(not re.search(r"^tty1::respawn:.*(?:getty|greetd).*$", inittab, re.M),
           "Concurrent login process on tty1")
    fstab = fs.text("/etc/fstab")
    ensure(re.search(r"^LABEL=ALCARDROOT\s+/\s+ext4\s+", fstab, re.M),
           "Missing persistent ext4 root in fstab")
    modules = set(fs.text("/etc/modules").splitlines())
    for mod in ("spi_bcm2835", "m5ioe1", "pwm_bl_m5stack",
                "panel_mipi_dbi", "tca8418_keypad_m5stack"):
        ensure(mod in modules, f"Missing /etc/modules entry {mod}")
    # Database is in /lib/apk/db on Alpine; /lib may be a merged-/usr
    # symlink, so check its real /usr/lib representation too.
    db_paths = ("/usr/lib/apk/db/installed", "/lib/apk/db/installed",
                "/var/lib/apk/db/installed")
    db_path = next((p for p in db_paths if fs.stat(p, required=False) is not None), None)
    ensure(db_path is not None, "No installed APK database in ext4 ROOT")
    installed = fs.text(db_path)
    packages = {}
    dependencies = {}
    audio_helpers = {"wpctl": "wireplumber", "pw-dump": "pipewire-tools",
                     "pipewire": "pipewire", "wireplumber": "wireplumber",
                     "pipewire-pulse": "pipewire-pulse", "flock": "flock"}
    audio_owners = {name: [] for name in audio_helpers}
    for entry in re.split(r"\n\s*\n", installed.strip()):
        fields = dict(line.split(":", 1) for line in entry.splitlines()
                      if len(line) > 2 and line[1] == ":")
        if "P" in fields and "V" in fields:
            packages[fields["P"]] = fields["V"]
            dependencies[fields["P"]] = fields.get("D", "").split()
            directory = None
            for line in entry.splitlines():
                if line.startswith("F:"):
                    directory = line[2:]
                elif line.startswith("R:") and directory == "usr/bin" and line[2:] in audio_owners:
                    audio_owners[line[2:]].append(fields["P"])
    expected_packages = {"alpine-base": None, "linux-rpi": CATALOG.kernel_version}
    expected_packages.update(CATALOG.versions((
        "alcard-kmods-rpi", "alcard-firmware-zero", "alcard-boot-zero",
        "alcard-bsp-zero", active, *GROUPS["user"],
    )))
    for package, version in expected_packages.items():
        found = packages.get(package)
        ensure(found is not None and (version is None or found == version),
               f"APK installed DB: expected {package}={version or 'installed'}, got {found!r}")
    for package in ("greetd", "greetd-agreety", "labwc", "foot", "dbus", "elogind",
                    "eudev", "eudev-openrc", "udev-init-scripts-openrc",
                    "linux-pam", "polkit-elogind", "polkit-common", "polkit-elogind-libs",
                    "pipewire", "wireplumber", "pipewire-tools", "pipewire-pulse", "pipewire-alsa", "flock"):
        ensure(package in packages, f"Full image is missing {package}")
    for owner, required in {
        "alcard-shell": {"wireplumber", "pipewire-tools"},
        "alcard-desktop-shell": {"pipewire", "wireplumber", "pipewire-pulse", "pipewire-alsa", "flock"},
    }.items():
        declared = {re.split(r"[<>=~]", dep, maxsplit=1)[0] for dep in dependencies.get(owner, [])}
        ensure(required <= declared, f"Installed {owner} misses audio dependencies: {sorted(required - declared)}")
    for name, owner in audio_helpers.items():
        ensure(audio_owners[name] == [owner],
               f"Expected {owner} to own /usr/bin/{name}, got {audio_owners[name]}")
        metadata = fs.lstat(f"/usr/bin/{name}")
        ensure(re.search(r"\bUser:\s*0\b", metadata)
               and re.search(r"\bGroup:\s*0\b", metadata),
               f"Unsafe {owner} /usr/bin/{name} ownership")
        if name == "pipewire-pulse":
            ensure(re.search(r"\bType:\s*symlink\b", metadata)
                   and re.search(r'Fast link dest:\s*"pipewire"', metadata),
                   "Expected pipewire-pulse -> pipewire vendor alias")
            # The target's package, root ownership and 0755 mode are checked
            # independently in this same loop before following the alias.
        else:
            ensure(re.search(r"\bType:\s*regular\b", metadata)
                   and re.search(r"\bMode:\s*0755\b", metadata),
                   f"Missing or unsafe {owner} /usr/bin/{name} executable")
        data = fs.cat(f"/usr/bin/{name}")
        interpreter, libraries = elf_dependencies(data)
        fs.stat(interpreter)
        for library in libraries:
            ensure(any(fs.stat(folder + library, required=False) is not None
                       for folder in ("/lib/", "/usr/lib/")),
                   f"Missing shared library for {name}: {library}")
        print(f"PASS: {owner} owns {name}, aarch64/musl sha256={hashlib.sha256(data).hexdigest()}")
    ensure(inactive not in packages, f"Inactive board profile installed: {inactive}")
    ensure(len(packages) >= 48, f"Unexpectedly small APK database ({len(packages)} entries)")
    dep_path = f"/usr/lib/modules/{ABI}/modules.dep"
    fs.stat(dep_path)
    dep = fs.text(dep_path)
    base = f"/usr/lib/modules/{ABI}/extra/alcard"
    for mod in MODULES:
        fs.stat(f"{base}/{mod}.ko")
        ensure(f"extra/alcard/{mod}.ko:" in dep,
               f"depmod metadata absent for {mod}")
    firmware_hashes = {
        "/usr/lib/firmware/cardputerzero,st7789v.bin":
            "cb8a403f7f7648622b386f312a5dd3e42d706c3419e1fd5216a36289d399fbe2",
        "/usr/lib/firmware/bmi270-init-data.fw":
            "2d75e68e343a13ff99be98261dfbd99d9e8c6f267da9e3c5aeb883276ad178db",
    }
    for firmware_path, checksum in firmware_hashes.items():
        fs.stat(firmware_path)
        actual = hashlib.sha256(fs.cat(firmware_path)).hexdigest()
        ensure(actual == checksum, f"Firmware SHA256 mismatch: {firmware_path}")
    expected_world = {CATALOG[name].pin for name in GROUPS["session-roots"]}
    expected_world.update({
        "greetd", "greetd-agreety",
        "eudev", "eudev-openrc", "udev-init-scripts-openrc",
    })
    ensure(expected_world <= world,
           f"Full image /etc/apk/world missing: {sorted(expected_world - world)}")

    for path in (
        "/usr/bin/alcard-settings", "/usr/sbin/alcard-setupd",
        "/usr/libexec/alcard/tty1-dispatch",
        "/usr/libexec/alcard/firstboot-init",
        "/usr/libexec/alcard/start-session",
        "/usr/libexec/alcard/wayland-session",
        "/usr/libexec/alcard/shell-session",
        "/usr/libexec/alcard/session-client",
        "/usr/libexec/alcard/audio-session",
        "/usr/bin/alcard-shell", "/usr/bin/alcard-polkit-agent", "/usr/bin/alcard-lock",
        "/etc/alcard/labwc-shell/rc.xml",
        "/etc/alcard/labwc-shell/environment",
        "/etc/alcard/labwc-shell/autostart",
        "/etc/pam.d/alcard-lock",
        "/etc/pam.d/alcard-greetd",
        "/etc/greetd/alcard.toml",
        "/etc/alcard/foot.ini",
        "/usr/share/kbd/consolefonts/AlcardPixel-Regular-core-8x8.psf",
        "/usr/share/fonts/alcard/AlcardPixel-Regular.ttf",
        "/usr/share/fonts/alcard/AlcardPixel-Bold.ttf",
        "/usr/share/X11/xkb/symbols/alcard",
        "/usr/share/alcard/keyboard/layouts.json",
        "/usr/share/alcard/keyboard/console/us.bmap.gz",
        "/usr/share/alcard/keyboard/console/ru.bmap.gz",
        "/usr/share/alcard/keyboard/console/es.bmap.gz",
        "/usr/share/alcard/keyboard/console/de.bmap.gz",
    ):
        fs.stat(path)
    dispatch = fs.text("/usr/libexec/alcard/tty1-dispatch")
    ensure("--needs-hello" in dispatch and "greetd" in dispatch
           and "getty" in dispatch,
           "Firstboot dispatcher cannot choose Hello/greetd/getty")

    passwd = fs.text("/etc/passwd").splitlines()
    greeter = next((row.split(":") for row in passwd
                    if row.startswith("greeter:")), None)
    ensure(greeter is not None and len(greeter) >= 4
           and 0 < int(greeter[2]) < 1000,
           "Unprivileged greetd greeter account missing or is a normal user")
    print(f"PASS: full-image APKs, fonts, Hello/greetd, tty1 dispatcher, "
          f"{active}, module ABI and firmware hashes")

    # A warning is preferable to claiming OpenRC and hardware are healthy.
    warnings = []
    for service in ("/etc/runlevels/sysinit/devfs", "/etc/runlevels/sysinit/dmesg",
                    "/etc/runlevels/sysinit/udev",
                    "/etc/runlevels/sysinit/udev-trigger", "/etc/runlevels/sysinit/udev-settle",
                    "/etc/runlevels/boot/modules",
                    "/etc/runlevels/default/udev-postmount", "/etc/runlevels/default/cgroups",
                    "/etc/runlevels/default/dbus"):
        fs.stat(service)  # mandatory: without these, early device/module setup is incomplete
    for level in ("sysinit", "boot", "default"):
        for service in ("mdev", "mdevd", "mdevd-init", "hwdrivers"):
            ensure(fs.stat(f"/etc/runlevels/{level}/{service}", required=False) is None,
                   f"Conflicting device manager remains enabled: {level}/{service}")
        ensure(fs.lstat(f"/etc/runlevels/{level}/elogind", required=False) is None,
               f"Competing elogind OpenRC autostart: {level}/elogind")
    print("PASS: essential OpenRC services registered "
          "(devfs, dmesg, udev/trigger/settle/postmount, modules, cgroups, dbus)")
    service_name = "org.freedesktop.login1.service"
    for folder in ("/etc/dbus-1/system-services", "/run/dbus-1/system-services",
                   "/usr/local/share/dbus-1/system-services"):
        ensure(fs.lstat(f"{folder}/{service_name}", required=False) is None,
               f"Conflicting elogind activation override: {folder}")
    activation = f"/usr/share/dbus-1/system-services/{service_name}"
    for path, mode in ((activation, "0644"), ("/usr/libexec/elogind/elogind", "0755")):
        metadata = fs.lstat(path)
        ensure(re.search(r"\bType:\s*regular\b", metadata)
               and re.search(r"\bMode:\s*" + mode + r"\b", metadata)
               and re.search(r"\bUser:\s*0\b", metadata)
               and re.search(r"\bGroup:\s*0\b", metadata)
               and re.search(r"\bLinks:\s*1\b", metadata),
               f"Missing or unsafe elogind activation asset: {path}")
    lines = [line.strip() for line in fs.text(activation).splitlines()
             if line.strip() and not line.lstrip().startswith("#")]
    ensure(len(lines) == 4 and lines[0] == "[D-BUS Service]" and set(lines[1:]) == {
        "Name=org.freedesktop.login1",
        "Exec=/usr/libexec/elogind/elogind --daemon",
        "User=root",
    }, "Unexpected vendor elogind D-Bus activation")
    print("PASS: vendor D-Bus activation owns elogind; no competing OpenRC autostart")
    shadow = fs.text("/etc/shadow")
    root_entry = next((line.split(":")[1] for line in shadow.splitlines()
                       if line.startswith("root:")), None)
    if root_entry in (None, "", "!", "*") or (root_entry is not None and
                                              root_entry.startswith("!")):
        warnings.append("root login is locked; no dev password provisioned")
    return warnings


def inspect_initramfs(file, temp):
    # mkinitfs default is gzip of newc CPIO. Insist on a valid archive,
    # rather than simply checking the FAT filename exists.
    with file.open("rb") as inp:
        magic = inp.read(2)
    ensure(magic == b"\x1f\x8b",
           "Initramfs is not gzip (first release requires mkinitfs default gzip)")
    listing = temp / "cpio-files.txt"
    errors = temp / "cpio-errors.txt"
    with gzip.open(file, "rb") as source, listing.open("wb") as out, errors.open("wb") as err:
        proc = subprocess.Popen(["cpio", "-it", "--quiet"], stdin=subprocess.PIPE,
                                stdout=out, stderr=err)
        try:
            shutil.copyfileobj(source, proc.stdin, 1024 * 1024)
        finally:
            proc.stdin.close()
        code = proc.wait()
    ensure(code == 0, "Corrupt initramfs CPIO: " + errors.read_text(errors="replace")[-1500:])
    names = set(name.removeprefix("./").lstrip("/")
                for name in listing.read_text(errors="replace").splitlines())
    ensure("init" in names, "Initramfs lacks /init")
    ensure("bin/busybox" in names or "usr/bin/busybox" in names,
           "Initramfs lacks BusyBox")
    print(f"PASS: gzip+cpio initramfs, {len(names)} archive entries, /init, BusyBox")
    warnings = []
    if not any(name.startswith(f"lib/modules/{ABI}/") or
               name.startswith(f"usr/lib/modules/{ABI}/") for name in names):
        warnings.append("Initramfs contains no kernel modules; verify required storage drivers are built in")
    config = Path(f"/usr/src/linux-headers-{ABI}/.config")
    if config.is_file():
        kconfig = config.read_text(errors="replace")
        match = re.search(r"^CONFIG_EXT4_FS=([ym])$", kconfig, re.M)
        ensure(match is not None, f"Matching Alpine kernel config does not enable EXT4_FS: {config}")
        if match.group(1) == "y":
            print("PASS: CONFIG_EXT4_FS=y (built-in; ext4.ko not required)")
        else:
            ensure(any(name.endswith("/ext4.ko") or name == "ext4.ko" for name in names),
                   "CONFIG_EXT4_FS=m but ext4.ko is missing from initramfs")
            print("PASS: CONFIG_EXT4_FS=m and ext4.ko included in initramfs")
    else:
        warnings.append("Matching linux-rpi-dev .config not available; cannot establish built-in EXT4_FS")
    return warnings


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", required=True, type=Path)
    parser.add_argument("--profile", required=True, choices=("v5", "v5-lite"))
    args = parser.parse_args()
    img = args.image.resolve(strict=True)
    ensure(img.is_file() and not args.image.is_symlink(), "Refusing non-regular image")
    for cmd in ("e2fsck", "debugfs", "mcopy", "mtype", "cpio"):
        ensure(shutil.which(cmd), f"Required verifier tool is missing: {cmd}")
    manifest_path = args.image.with_suffix(".json")
    ensure(manifest_path.is_file() and not manifest_path.is_symlink(),
           "Missing image JSON manifest")
    manifest = json.loads(manifest_path.read_text())
    disk_id, parts = get_partitions(img)
    verify_parent = img.parent.parent / "verify"
    ensure(not verify_parent.is_symlink(), "Unsafe verification directory")
    verify_parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=f"image-{args.profile}-", dir=verify_parent) as work:
        temporary = Path(work)
        ext4 = temporary / "root.ext4"
        extract_partition(img, *parts[3], ext4)
        fsck(ext4)
        root_metadata(ext4, manifest, disk_id, args.profile)
        fs = Ext4(ext4)
        warnings = inspect_root(fs, args.profile)
        initramfs = check_boot(img, parts, args.profile, disk_id, fs, temporary)
        warnings.extend(inspect_initramfs(initramfs, temporary))
        for message in warnings:
            print(f"WARN: {message}")
    print(f"PASS: deep offline verification completed for {args.profile}")
    print("NOTE: This does not prove physical CM0 boot, DRM/keyboard probe or A/B recovery.")


if __name__ == "__main__":
    try:
        main()
    except (VerificationError, OSError, UnicodeError, ValueError, json.JSONDecodeError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        sys.exit(1)
