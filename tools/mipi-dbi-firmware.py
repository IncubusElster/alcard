#!/usr/bin/env python3
"""Build/verify Linux panel-mipi-dbi (v1) firmware from Alcard TOML.

No nonstandard Python packages required (Python >= 3.11 for tomllib).
The firmware is a command table consumed by Linux panel-mipi-dbi, not
executable microcode. It is intentionally independent of kernel headers.
"""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path
import sys
import tomllib

MAGIC = b"MIPI DBI" + (b"\x00" * 7)
VERSION = 1
ALLOWED_ROOT = {"format_version", "compatible", "panel", "steps"}


class FirmwareError(ValueError):
    pass


def byte(value: object, what: str) -> int:
    if type(value) is not int or not 0 <= value <= 255:
        raise FirmwareError(f"{what}: expected an integer in 0..255")
    return value


def compile_firmware(source: Path) -> tuple[str, bytes, int]:
    with source.open("rb") as stream:
        config = tomllib.load(stream)

    unknown = set(config) - ALLOWED_ROOT
    if unknown:
        raise FirmwareError(f"unexpected TOML fields: {sorted(unknown)}")
    if type(config.get("format_version")) is not int or config["format_version"] != VERSION:
        raise FirmwareError("format_version must be 1")

    compatible = config.get("compatible")
    if not isinstance(compatible, str) or compatible != "cardputerzero,st7789v":
        raise FirmwareError("compatible must equal cardputerzero,st7789v (DT first compatible)")
    if not isinstance(config.get("panel"), str) or not config["panel"]:
        raise FirmwareError("panel description is missing")

    steps = config.get("steps")
    if not isinstance(steps, list) or not steps:
        raise FirmwareError("steps must be a nonempty array of tables")

    commands = bytearray()
    for i, step in enumerate(steps, start=1):
        if not isinstance(step, dict):
            raise FirmwareError(f"step {i}: expected a table")
        if "delay_ms" in step:
            if set(step) != {"delay_ms"}:
                raise FirmwareError(f"step {i}: delay must not specify command or parameters")
            delay = byte(step["delay_ms"], f"step {i} delay_ms")
            if delay == 0:
                raise FirmwareError(f"step {i}: zero-length delay not allowed")
            commands.extend((0x00, 0x01, delay))
        else:
            if set(step) - {"command", "parameters"} or "command" not in step:
                raise FirmwareError(f"step {i}: expected command and optional parameters only")
            cmd = byte(step["command"], f"step {i} command")
            if cmd == 0:
                raise FirmwareError(f"step {i}: command 0x00 reserved for NOP delay encoding")
            params = step.get("parameters", [])
            if not isinstance(params, list) or len(params) > 255:
                raise FirmwareError(f"step {i}: parameters must be a list of at most 255 bytes")
            commands.extend((cmd, len(params)))
            commands.extend(byte(v, f"step {i} parameter") for v in params)

    if len(commands) < 2:
        raise FirmwareError("empty command stream")
    return compatible, MAGIC + bytes((VERSION,)) + commands, len(steps)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("build", "verify"))
    parser.add_argument("toml", type=Path, help="source command table")
    parser.add_argument("firmware", type=Path, help="generated firmware path")
    args = parser.parse_args()
    try:
        compatible, expected, count = compile_firmware(args.toml)
        expected_filename = f"{compatible}.bin"
        if args.firmware.name != expected_filename:
            raise FirmwareError(f"firmware basename must be {expected_filename}")
        if args.operation == "build":
            args.firmware.parent.mkdir(parents=True, exist_ok=True)
            args.firmware.write_bytes(expected)
        elif args.firmware.read_bytes() != expected:
            raise FirmwareError("firmware differs from TOML or is malformed")
        print(f"{args.operation}: {args.firmware} ({len(expected)} bytes, "
              f"{count} steps, sha256 {hashlib.sha256(expected).hexdigest()})")
        return 0
    except (FirmwareError, OSError, tomllib.TOMLDecodeError) as exc:
        print(f"firmware: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())