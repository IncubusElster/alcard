#!/usr/bin/env python3
"""Build/inspect Alcard MBR + FAT/FAT/ext4 images without loop or host mounts.

Only accepts files and directories created by the dedicated image worker.
Not a general-purpose disk writer. Never accepts a block-device destination.
"""
import argparse
import json
import os
from pathlib import Path
import secrets
import shutil
import struct
import subprocess
import sys
import uuid

SECTOR = 512
SPM = 2048
NAMES = ("BOOT-SELECT", "BOOT-A", "BOOT-B", "ROOT")
KINDS = (0x0C, 0x0C, 0x0C, 0x83)


def run(*args):
    subprocess.run([str(arg) for arg in args], check=True)


def check_regular(path):
    if path.is_symlink() or (path.exists() and not path.is_file()):
        raise ValueError(f"Refusing non-regular output: {path}")


def layout(sizes):
    if len(sizes) != 4 or any(n < 32 for n in sizes[:3]) or sizes[3] < 256:
        raise ValueError("FAT sizes >= 32 MiB and ROOT >= 256 MiB are required")
    start = SPM
    partitions = []
    for label, kind, size in zip(NAMES, KINDS, sizes):
        nsec = size * SPM
        partitions.append(dict(name=label, type=kind, start=start, sectors=nsec))
        start += nsec
    return partitions, start + SPM


def mbr(parts, disk_id):
    data = bytearray(SECTOR)
    struct.pack_into("<I", data, 440, disk_id)
    for index, part in enumerate(parts):
        off = 446 + index * 16
        data[off] = 0x80 if index == 0 else 0x00
        data[off + 1:off + 4] = b"\xfe\xff\xff"
        data[off + 4] = part["type"]
        data[off + 5:off + 8] = b"\xfe\xff\xff"
        struct.pack_into("<II", data, off + 8, part["start"], part["sectors"])
    data[510:512] = b"\x55\xaa"
    return data


def stage_boot(source, destination):
    """Copy boot payload to FAT staging without symlinks.

    FAT has no symlinks. Directory aliases (including Alpine's /boot/boot)
    must not be traversed because they can recurse forever. File aliases are
    flattened only when their real target stays inside the boot payload.
    """
    source = Path(source).resolve(strict=True)
    destination = Path(destination)
    if not source.is_dir():
        raise ValueError(f"Boot payload is not a directory: {source}")
    if destination.is_symlink() or (destination.exists() and not destination.is_dir()):
        raise ValueError(f"Unsafe FAT staging directory: {destination}")
    destination.mkdir(parents=True, exist_ok=True)
    if any(destination.iterdir()):
        raise ValueError(f"FAT staging must be empty before copying: {destination}")

    def copy_dir(src, dst):
        for entry in sorted(src.iterdir()):
            target = dst / entry.name
            if entry.is_symlink():
                resolved = entry.resolve(strict=True)
                if resolved.is_dir():
                    print(f"==> Skip FAT-incompatible directory symlink: {entry.relative_to(source)} -> {entry.readlink()}")
                    continue
                if not resolved.is_file():
                    raise ValueError(f"Invalid boot file symlink: {entry}")
                if not resolved.is_relative_to(source):
                    raise ValueError(f"Boot file symlink escapes /boot: {entry} -> {resolved}")
                shutil.copy2(resolved, target)
                print(f"==> Materialized boot file symlink: {entry.relative_to(source)}")
            elif entry.is_dir():
                target.mkdir()
                copy_dir(entry, target)
            elif entry.is_file():
                shutil.copy2(entry, target)
            else:
                raise ValueError(f"Unsupported FAT boot entry: {entry}")

    copy_dir(source, destination)


def copy_to_fat(image, directory):
    entries = sorted(directory.iterdir())
    if not entries:
        raise ValueError(f"Empty FAT staging: {directory}")
    for entry in entries:
        if entry.is_symlink():
            raise ValueError(f"Symlink in FAT staging: {entry}")
        run("mcopy", "-s", "-i", image, entry, "::")


def pack(args):
    work = Path(args.work).resolve()
    stages = [work / p for p in ("select", "boot-a", "boot-b", "rootfs")]
    for stage in stages:
        if not stage.is_dir() or stage.is_symlink():
            raise ValueError(f"Missing or unsafe staging directory: {stage}")

    destination = Path(args.output).absolute()
    check_regular(destination)
    if str(destination).startswith("/dev/"):
        raise ValueError("Refusing block-device destination")
    sizes = [args.select_mib, args.boot_mib, args.boot_mib, args.root_mib]
    partitions, total = layout(sizes)
    disk_id = int(args.disk_id, 16)
    if not (0 < disk_id <= 0xFFFFFFFF):
        raise ValueError("Disk ID must be a nonzero 32-bit hex value")
    ext4_uuid = uuid.UUID(args.root_uuid)
    temporary = work / "disk.tmp.img"
    check_regular(temporary)
    parts = []
    try:
        for i, (partition, stage, size) in enumerate(zip(partitions, stages, sizes)):
            file = work / f"part{i + 1}.img"
            check_regular(file)
            with file.open("wb") as output:
                output.truncate(size * 1024 * 1024)
            if i < 3:
                run("mkfs.vfat", "-F", "32", "-n", ("ALCARDSEL", "ALCARDA", "ALCARDB")[i], file)
                copy_to_fat(file, stage)
            else:
                run("mkfs.ext4", "-q", "-F", "-L", "ALCARDROOT", "-U", str(ext4_uuid), "-d", stage, file)
            parts.append(file)

        with temporary.open("wb") as result:
            result.truncate(total * SECTOR)
            result.seek(0)
            result.write(mbr(partitions, disk_id))
            for partition, path in zip(partitions, parts):
                result.seek(partition["start"] * SECTOR)
                with path.open("rb") as source:
                    shutil.copyfileobj(source, result, 1024 * 1024)
            result.flush()
            os.fsync(result.fileno())
        verify_file(temporary, expected_disk_id=disk_id)
        destination.parent.mkdir(parents=True, exist_ok=True)
        os.replace(temporary, destination)
        metadata = {
            "format": "Alcard-image-v1",
            "profile": args.profile,
            "disk_id": f"{disk_id:08x}",
            "root_partuuid": f"{disk_id:08x}-04",
            "root_uuid": str(ext4_uuid),
            "partitions": partitions,
            "note": "BOOT-SELECT holds a bootable copy of BOOT-A until CM0 A/B is validated; automatic tryboot disabled",
        }
        manifest = destination.with_suffix(".json")
        check_regular(manifest)
        manifest.write_text(json.dumps(metadata, indent=2) + "\n")
        print(f"Image: {destination} ({destination.stat().st_size} bytes)")
        print(f"PARTUUID: {metadata['root_partuuid']}")
    finally:
        temporary.unlink(missing_ok=True)
        for path in parts:
            path.unlink(missing_ok=True)


def fat_exists(image, partition, name):
    offset = partition["start"] * SECTOR
    proc = subprocess.run(
        ["mdir", "-i", f"{image}@@{offset}", f"::/{name}"],
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
    )
    if proc.returncode:
        raise ValueError(f"Missing boot file {name}: {proc.stderr.decode(errors='replace').strip()}")


def fat_file(image, partition, name):
    offset = partition["start"] * SECTOR
    proc = subprocess.run(
        ["mtype", "-i", f"{image}@@{offset}", f"::{name}"],
        capture_output=True, check=True,
    )
    return proc.stdout


def verify_file(image, expected_disk_id=None):
    if not image.is_file() or image.is_symlink():
        raise ValueError(f"Not a regular image: {image}")
    with image.open("rb") as inp:
        boot = inp.read(SECTOR)
        if len(boot) != SECTOR or boot[510:512] != b"\x55\xaa":
            raise ValueError("Invalid MBR signature")
        disk_id = struct.unpack_from("<I", boot, 440)[0]
        if expected_disk_id is not None and disk_id != expected_disk_id:
            raise ValueError("Disk ID mismatch")
        parts = []
        for i, kind in enumerate(KINDS):
            off = 446 + i * 16
            found = boot[off + 4]
            start, sectors = struct.unpack_from("<II", boot, off + 8)
            if found != kind or start == 0 or sectors == 0 or start % SPM:
                raise ValueError(f"Bad partition {i + 1}")
            parts.append(dict(start=start, sectors=sectors))
        if parts[0]["start"] != SPM:
            raise ValueError("First FAT not at 1 MiB")
        for left, right in zip(parts, parts[1:]):
            if left["start"] + left["sectors"] > right["start"]:
                raise ValueError("Overlapping partitions")
        if (parts[-1]["start"] + parts[-1]["sectors"]) * SECTOR > image.stat().st_size:
            raise ValueError("Truncated image")
        inp.seek(parts[3]["start"] * SECTOR + 1024 + 56)
        if inp.read(2) != b"\x53\xef":
            raise ValueError("No ext4 superblock signature in ROOT")
    required = (
        "bootcode.bin", "start.elf", "fixup.dat", "vmlinuz-rpi",
        "initramfs-rpi", "bcm2710-rpi-cm0.dtb",
        "overlays/alcard-zero-v5.dtbo", "alcard.cfg", "config.txt", "cmdline.txt",
    )
    for index in (0, 1, 2):
        for name in required:
            fat_exists(image, parts[index], name)
        cfg = fat_file(image, parts[index], "config.txt").decode()
        cmd = fat_file(image, parts[index], "cmdline.txt").decode()
        profile_cfg = fat_file(image, parts[index], "alcard.cfg").decode()
        if cfg.count("include alcard.cfg") != 1:
            raise ValueError("config.txt must include alcard.cfg exactly once")
        if "tryboot_a_b=1" in cfg or "autoboot.txt" in cfg:
            raise ValueError("A/B controls unexpectedly enabled")
        if f"root=PARTUUID={disk_id:08x}-04" not in cmd:
            raise ValueError(f"Wrong root PARTUUID in FAT partition {index + 1}")
        if "dtoverlay=alcard-zero-v5" not in profile_cfg:
            raise ValueError("Missing main board overlay")
    print(f"PASS: MBR, 4 partitions, 3 FAT boot payloads, ext4 signature, root PARTUUID={disk_id:08x}-04")
    return disk_id


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    p = commands.add_parser("pack")
    p.add_argument("--work", required=True)
    p.add_argument("--output", required=True)
    p.add_argument("--disk-id", required=True)
    p.add_argument("--root-uuid", required=True)
    p.add_argument("--profile", choices=("v5", "v5-lite"), required=True)
    p.add_argument("--select-mib", type=int, default=64)
    p.add_argument("--boot-mib", type=int, default=128)
    p.add_argument("--root-mib", type=int, default=768)
    v = commands.add_parser("verify")
    v.add_argument("image", type=Path)
    staging = commands.add_parser("stage-boot")
    staging.add_argument("--source", type=Path, required=True)
    staging.add_argument("--destination", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "pack":
            pack(args)
        elif args.command == "stage-boot":
            stage_boot(args.source, args.destination)
        else:
            verify_file(args.image)
    except (OSError, ValueError, subprocess.CalledProcessError) as err:
        print(f"ERROR: {err}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
