#!/usr/bin/env python3
"""Software acceptance of the unchanged Shell/locker on private runtime inputs.

The fake login1 never calls a host backend. Only the fixture writes the explicit
RW aliases; production processes read their normal, read-only /sys paths.
"""
from __future__ import annotations

import argparse
import contextlib
import errno
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time
import traceback
from types import SimpleNamespace
import xml.etree.ElementTree as ET

MANAGER_PATH = "/org/freedesktop/login1"
MANAGER = "org.freedesktop.login1.Manager"
SESSION_PATH = "/org/freedesktop/login1/session/c33"
SESSION = "org.freedesktop.login1.Session"
SEAT_PATH = "/org/freedesktop/login1/seat/seat0"
SEAT = "org.freedesktop.login1.Seat"
CONTROL_PATH = "/org/alcard/TestLogin1"
CONTROL = "org.alcard.TestLogin1"
SERVICE = "org.freedesktop.login1"
MARKER = "alcard-battery-inhibit-v1"
HERE = Path(__file__).resolve()

XML = """<node>
<interface name="org.freedesktop.login1.Manager">
 <method name="GetSessionByPID"><arg type="u" direction="in"/><arg type="o" direction="out"/></method>
 <method name="CanSuspend"><arg type="s" direction="out"/></method>
 <method name="SuspendWithFlags"><arg type="t" direction="in"/></method>
 <method name="ListInhibitors"><arg type="a(ssssuu)" direction="out"/></method>
 <method name="Inhibit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="h" direction="out"/></method>
 <signal name="PrepareForSleep"><arg type="b"/></signal>
 <property name="PreparingForSleep" type="b" access="read"/>
 <property name="InhibitDelayMaxUSec" type="t" access="read"/>
 <method name="CanReboot"><arg type="s" direction="out"/></method>
 <method name="CanPowerOff"><arg type="s" direction="out"/></method>
 <method name="RebootWithFlags"><arg type="t" direction="in"/></method>
 <method name="PowerOffWithFlags"><arg type="t" direction="in"/></method>
 <method name="TerminateSession"><arg type="s" direction="in"/></method>
</interface>
<interface name="org.freedesktop.login1.Session">
 <property name="Id" type="s" access="read"/>
 <property name="User" type="(uo)" access="read"/>
 <property name="Type" type="s" access="read"/>
 <property name="Class" type="s" access="read"/>
 <property name="State" type="s" access="read"/>
 <property name="Remote" type="b" access="read"/>
 <property name="Active" type="b" access="read"/>
 <property name="Seat" type="(so)" access="read"/>
 <method name="SetBrightness"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="u" direction="in"/></method>
</interface>
<interface name="org.freedesktop.login1.Seat">
 <property name="Id" type="s" access="read"/>
 <property name="CanGraphical" type="b" access="read"/>
 <property name="ActiveSession" type="(so)" access="read"/>
</interface>
<interface name="org.alcard.TestLogin1">
 <method name="SetCase"><arg type="s" direction="in"/><arg type="s" direction="in"/></method>
 <method name="Prepare"><arg type="b" direction="in"/></method>
 <method name="Snapshot"><arg type="s" direction="out"/></method>
</interface>
</node>"""


def gi():
    from gi.repository import Gio, GLib
    return Gio, GLib


def record(journal_path: Path, case: str, event: str, **fields):
    row = dict(case=case, event=event, monotonic_ns=time.monotonic_ns(),
               observer_pid=os.getpid(), **fields)
    data = (json.dumps(row, ensure_ascii=True, sort_keys=True) + "\n").encode()
    assert len(data) < 4096, "Receipt exceeds one bounded append"
    descriptor = os.open(journal_path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
    try:
        assert os.write(descriptor, data) == len(data)
    finally:
        os.close(descriptor)


def rows(path: Path) -> list[dict]:
    if not path.exists():
        return []
    data = path.read_bytes()
    # A simultaneous bounded append may not yet expose its final newline.
    return [json.loads(line) for line in data.split(b"\n")[:-1] if line]


def atomic_text(path: Path, text: str):
    temporary = path.with_name(path.name + ".fixture-new")
    temporary.write_text(text)
    temporary.replace(path)


def sha256(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def mount_rows(text: str) -> list[dict]:
    def unescape(value):
        return re.sub(r"\\([0-7]{3})", lambda match: chr(int(match[1], 8)), value)
    result = []
    for line in text.splitlines():
        fields = line.split()
        separator = fields.index("-")
        result.append(dict(
            id=int(fields[0]), parent=int(fields[1]), device=fields[2],
            root=unescape(fields[3]), target=unescape(fields[4]),
            options=fields[5].split(","), filesystem=fields[separator + 1],
            source=unescape(fields[separator + 2]),
        ))
    return result


def validate_mount(alias: Path, target: Path, empty: bool) -> dict:
    assert alias.is_absolute() and alias.resolve() == alias, "Alias must be ordinary and absolute"
    assert not alias.is_symlink() and alias.is_dir(), "Alias must be an ordinary directory"
    assert not alias.is_relative_to("/sys"), "The write alias must be outside /sys"
    exposed, backing = target.stat(), alias.stat()
    assert (exposed.st_dev, exposed.st_ino) == (backing.st_dev, backing.st_ino), (
        f"{alias} is not the backing directory of {target}"
    )
    matching = [entry for entry in mount_rows(Path("/proc/self/mountinfo").read_text())
                if entry["target"] == str(target)]
    assert len(matching) == 1, f"Expected one explicit bind mount over {target}"
    mounted = matching[0]
    assert "ro" in mounted["options"], f"{target} must be read-only"
    assert mounted["filesystem"] != "sysfs", "A fixture must not alias real kernel sysfs"
    if empty:
        assert not any(alias.iterdir()), f"Fixture directory must initially be empty: {alias}"
    return dict(alias=str(alias), target=str(target), device=backing.st_dev,
                inode=backing.st_ino, mount=mounted)


def check_mounts(power: Path, backlight: Path, empty=False) -> list[dict]:
    assert power.resolve() != backlight.resolve()
    return [
        validate_mount(power, Path("/sys/class/power_supply"), empty),
        validate_mount(backlight, Path("/sys/class/backlight"), empty),
    ]


def connection(address: str):
    Gio, _ = gi()
    return Gio.DBusConnection.new_for_address_sync(
        address, Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT |
        Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION, None, None)


def call(bus, destination, path, interface, method, signature="()", arguments=()):
    Gio, GLib = gi()
    return bus.call_sync(destination, path, interface, method,
                         GLib.Variant(signature, arguments), None,
                         Gio.DBusCallFlags.NONE, 5000, None).unpack()


def login1(args):
    """Only this process implements login1 on the newly created private bus."""
    Gio, GLib = gi()
    assert args.address == os.environ["DBUS_SYSTEM_BUS_ADDRESS"]
    assert args.address.startswith("unix:path=")
    bus = connection(args.address)
    bus.set_exit_on_close(False)
    owner = bus.get_unique_name()
    current = dict(case="startup", mode="allowed", preparing=False, cycle=0, next_delay=0)
    pipes = {}

    def log(event, **fields):
        record(args.journal, current["case"], event, owner=owner, **fields)

    def prepare(value):
        current["preparing"] = value
        if value:
            current["cycle"] += 1
        log("PrepareForSleep", preparing=value, cycle=current["cycle"])
        bus.emit_signal(None, MANAGER_PATH, MANAGER, "PrepareForSleep",
                        GLib.Variant("(b)", (value,)))
        bus.flush_sync(None)

    def property_get(_bus, _sender, path, interface, name):
        if interface == MANAGER:
            values = dict(PreparingForSleep=("b", current["preparing"]),
                          InhibitDelayMaxUSec=("t", 10_000_000))
        elif interface == SESSION:
            values = dict(
                Id=("s", "c33"), User=("(uo)", (os.getuid(), f"/org/freedesktop/login1/user/_{os.getuid()}")),
                Type=("s", "wayland"), Class=("s", "user"), State=("s", "active"),
                Remote=("b", False), Active=("b", True), Seat=("(so)", ("seat0", SEAT_PATH)))
        else:
            values = dict(Id=("s", "seat0"), CanGraphical=("b", True),
                          ActiveSession=("(so)", ("c33", SESSION_PATH)))
        signature, value = values[name]
        return GLib.Variant(signature, value)

    def dispatch(_bus, sender, path, interface, method, parameters, invocation):
        values = list(parameters.unpack())
        if interface == CONTROL:
            if method == "SetCase":
                case, mode = values
                if mode not in {"allowed", "denied", "blocked", "challenge", "unknown", "brightness-denied"}:
                    invocation.return_dbus_error("org.freedesktop.DBus.Error.InvalidArgs", "Unknown fixture mode")
                    return
                current.update(case=case, mode=mode)
                log("SetCase", mode=mode)
                invocation.return_value(GLib.Variant("()", ()))
            elif method == "Prepare":
                prepare(values[0])
                invocation.return_value(GLib.Variant("()", ()))
            else:
                invocation.return_value(GLib.Variant("(s)", (json.dumps(current | {"owner": owner}),)))
            return
        message = invocation.get_message()
        log("Call", method=method, arguments=values, sender=sender,
            destination=message.get_destination(), flags=int(message.get_flags()), path=path)
        if method == "GetSessionByPID":
            invocation.return_value(GLib.Variant("(o)", (SESSION_PATH,)))
        elif method == "CanSuspend":
            value = {"denied": "no", "challenge": "challenge"}.get(current["mode"], "yes")
            log("CapabilityReturned", value=value)
            invocation.return_value(GLib.Variant("(s)", (value,)))
        elif method in {"CanReboot", "CanPowerOff"}:
            invocation.return_value(GLib.Variant("(s)", ("yes",)))
        elif method == "ListInhibitors":
            inhibitors = []
            if current["mode"] == "blocked":
                inhibitors = [("sleep", "Alcard fixture", "Controlled block", "block", os.getuid(), os.getpid())]
            log("InhibitorsReturned", blocked=bool(inhibitors))
            invocation.return_value(GLib.Variant("(a(ssssuu))", (inhibitors,)))
        elif method == "Inhibit":
            if values[0] != "sleep" or values[3] != "delay":
                invocation.return_dbus_error("org.freedesktop.DBus.Error.InvalidArgs", "Only sleep delay is supported")
                return
            read_fd, write_fd = os.pipe2(os.O_NONBLOCK | os.O_CLOEXEC)
            current["next_delay"] += 1
            delay = current["next_delay"]
            pipes[read_fd] = delay
            descriptors = Gio.UnixFDList.new()
            index = descriptors.append(write_fd)
            os.close(write_fd)
            log("DelayAcquired", delay=delay, cycle=current["cycle"])
            # GDBus duplicates the descriptor into the reply; this local list
            # is released on return, so only Shell can prolong the pipe lease.
            invocation.return_value_with_unix_fd_list(GLib.Variant("(h)", (index,)), descriptors)
        elif method == "SuspendWithFlags":
            if current["mode"] == "unknown":
                invocation.return_dbus_error("org.freedesktop.DBus.Error.NoReply", "Controlled unknown result")
            elif current["mode"] in {"denied", "blocked", "challenge"} or values != [1]:
                invocation.return_dbus_error("org.freedesktop.DBus.Error.AccessDenied", "Controlled refusal")
            else:
                # Deliberately no kernel power operation. Emit the same public
                # event contract that the real login1 observer consumes.
                invocation.return_value(GLib.Variant("()", ()))
                prepare(True)
        elif method == "SetBrightness":
            if current["mode"] == "brightness-denied":
                invocation.return_dbus_error("org.freedesktop.DBus.Error.AccessDenied", "Controlled refusal")
                return
            if not args.backlight or values[:2] != ["backlight", "alcard-test-backlight"]:
                invocation.return_dbus_error("org.freedesktop.DBus.Error.InvalidArgs", "Unknown fixture device")
                return
            device = args.backlight / values[1]
            maximum = int((device / "max_brightness").read_text())
            if not 0 <= values[2] <= maximum:
                invocation.return_dbus_error("org.freedesktop.DBus.Error.InvalidArgs", "Out of fixture range")
                return
            atomic_text(device / "brightness", f"{values[2]}\n")
            atomic_text(device / "actual_brightness", f"{values[2]}\n")
            log("BrightnessWritten", value=values[2], device=values[1])
            invocation.return_value(GLib.Variant("()", ()))
        else:
            invocation.return_dbus_error("org.freedesktop.DBus.Error.AccessDenied", "No other mutations exist in this fixture")

    def poll_pipes():
        for descriptor, delay in list(pipes.items()):
            try:
                value = os.read(descriptor, 64)
            except BlockingIOError:
                continue
            if not value:
                log("DelayReleased", delay=delay, cycle=current["cycle"],
                    preparing=current["preparing"])
                os.close(descriptor)
                del pipes[descriptor]
        return True

    node = Gio.DBusNodeInfo.new_for_xml(XML)
    for path, info in zip((MANAGER_PATH, SESSION_PATH, SEAT_PATH, CONTROL_PATH), node.interfaces):
        bus.register_object(path, info, dispatch, property_get, None)
    assert call(bus, "org.freedesktop.DBus", "/org/freedesktop/DBus",
                "org.freedesktop.DBus", "RequestName", "(su)", (SERVICE, 4)) == (1,)
    GLib.timeout_add(25, poll_pipes)
    log("FixtureReady")
    args.ready.write_text(owner)
    GLib.MainLoop().run()


def locker(arguments):
    """Observe real public lifecycle; never synthesize successful lock output."""
    journal = Path(os.environ["ALCARD_BATTERY_JOURNAL"])
    case = os.environ["ALCARD_BATTERY_CASE"]
    binary = os.environ["ALCARD_BATTERY_LOCK_REAL"]
    gate = os.environ.get("ALCARD_BATTERY_LOCK_GATE")
    environment = dict(os.environ)
    environment.pop("WAYLAND_DEBUG", None)
    if arguments[:1] == ["--"]:
        arguments = arguments[1:]
    child = subprocess.Popen([binary, *arguments], stdin=subprocess.DEVNULL,
                             stdout=subprocess.PIPE, env=environment)
    record(journal, case, "LockerStarted", pid=child.pid, wrapper_pid=os.getpid(),
           executable=str(Path(binary).resolve()))
    assert child.stdout is not None
    for line in child.stdout:
        assert line in {b"LOCKED\n", b"UNLOCKED\n", b"FAILED\n"}, "Malformed native lifecycle"
        lifecycle = line.decode().strip()
        record(journal, case, "Native" + lifecycle.title(), pid=child.pid)
        if lifecycle == "LOCKED" and gate:
            deadline = time.monotonic() + 20
            while not Path(gate).exists():
                assert time.monotonic() < deadline, "Fixture lifecycle gate was not released"
                time.sleep(0.02)
        # Record before forwarding: any resulting Shell mutation is causally
        # later than this canonical receipt, without guessing compositor time.
        record(journal, case, "LockReceiptForwarded", lifecycle=lifecycle, pid=child.pid)
        try:
            sys.stdout.buffer.write(line)
            sys.stdout.buffer.flush()
        except BrokenPipeError:
            # Restarting Shell cannot kill or unlock the independent locker.
            pass
    raise SystemExit(child.wait())


def verify_suspend_receipts(events, expected: int):
    mutations = [row for row in events if row["event"] == "Call" and
                 row.get("method") in {"SuspendWithFlags", "RebootWithFlags",
                                       "PowerOffWithFlags", "TerminateSession"}]
    assert len(mutations) == expected, f"Unexpected power/session calls: {mutations}"
    for mutation in mutations:
        assert mutation["method"] == "SuspendWithFlags", mutation
        assert mutation["arguments"] == [1], mutation
        assert mutation["flags"] & 4 == 0, "Automatic/manual Shell sleep must not request interactive auth"
        assert mutation["destination"] == mutation["owner"], "Mutation did not pin login1's unique owner"
        assert any(row["event"] == "NativeLocked" and row["monotonic_ns"] < mutation["monotonic_ns"]
                   for row in events), "Sleep requested before real native LOCKED"
        assert any(row["event"] == "LockReceiptForwarded" and row["lifecycle"] == "LOCKED"
                   and row["monotonic_ns"] < mutation["monotonic_ns"] for row in events), (
                       "Sleep requested before Shell received canonical LOCKED"
                   )
    return mutations


def verify_sleep_refusal(events):
    verify_suspend_receipts(events, 0)
    assert not any(row["event"] in {"LockerStarted", "NativeLocked"} for row in events), (
        "Unavailable sleep started a locker"
    )
    assert all(row["flags"] & 4 == 0 for row in events if row["event"] == "Call"), (
        "Unavailable sleep requested interactive authorization"
    )


class TextObserver:
    """Exact bitmap glyph matching, also used by check-shell-system.sh."""
    def __init__(self, root: Path):
        import tomllib
        palette = tomllib.loads((root / "rust/crates/alcard-settings-core/themes/classic.toml").read_text())["palette"]
        self.colors = {tuple(bytes.fromhex(palette[key][1:])) for key in
                       ("text", "muted", "accent", "disabled", "success", "warning", "error")}
        self.glyphs = {}
        for line in (root / "fonts/alcard-pixel/source/AlcardPixel-Regular.hex").read_text().splitlines():
            if ":" in line:
                code, bits = line.split(":", 1)
                if len(bits) == 16:
                    self.glyphs[chr(int(code, 16))] = bytes.fromhex(bits)

    def contains(self, path: Path, text: str) -> bool:
        from PIL import Image
        image = Image.open(path).convert("RGB")
        assert image.size == (320, 170), image.size
        pixels = bytes(pixel in self.colors for pixel in image.getdata())
        rows_ = [pixels[y * 320:(y + 1) * 320] for y in range(170)]
        template = [bytes(bool(self.glyphs[ch][y] & (0x80 >> x))
                          for ch in text for x in range(8)) for y in range(8)]
        anchor = max(range(8), key=lambda row: sum(template[row]))
        assert sum(template[anchor])
        for y in range(163):
            start = 0
            while (x := rows_[y + anchor].find(template[anchor], start)) != -1:
                if all(rows_[y + line][x:x + len(template[line])] == template[line] for line in range(8)):
                    return True
                start = x + 1
        return False


class Runtime:
    def __init__(self, args, name, *, automatic=False, percent=80, mode="allowed", hold_lock=False, stale=False):
        self.args, self.name = args, name
        self.automatic, self.percent, self.mode = automatic, percent, mode
        self.hold_lock, self.stale = hold_lock, stale
        self.base = args.temporary / name
        self.out = args.out / name
        self.runtime = self.base / "runtime"
        self.journal = self.out / "receipts.jsonl"
        self.processes = []
        self.logs = []
        self.threads = []
        self.shell = None
        self.bus = None
        self.session_bus = None
        self.shell_number = 0
        self.output = None
        self.environment = {}
        self.stop = threading.Event()
        self.fifo_opened = threading.Event()
        self.fifo_release = threading.Event()
        self.fifo_fresh_release = threading.Event()
        self.fifo_done = threading.Event()
        self.observer = TextObserver(args.root)
        self.captures = []
        self.gate = self.base / "forward-native-lock"

    def event(self, event, **fields):
        record(self.journal, self.name, event, **fields)

    def events(self):
        return rows(self.journal)

    def matching(self, event, **fields):
        return [row for row in self.events() if row["event"] == event and
                all(row.get(key) == value for key, value in fields.items())]

    def calls(self, method):
        return self.matching("Call", method=method)

    def spawn(self, command, name, *, environment=None, stderr=None):
        log = (self.out / f"{name}.log").open("wb")
        self.logs.append(log)
        process = subprocess.Popen(command, env=environment or self.environment,
                                   stdin=subprocess.DEVNULL, stdout=log,
                                   stderr=stderr if stderr is not None else subprocess.STDOUT,
                                   start_new_session=True)
        self.processes.append((name, process))
        return process

    def healthy(self):
        for name, process in self.processes:
            if name in {"compositor", "private-bus", "fixture", "keyboard"}:
                assert process.poll() is None, f"{name} exited: {process.returncode}"
        if self.shell:
            assert self.shell.poll() is None, f"Shell exited: {self.shell.returncode}"

    def wait(self, description, predicate, seconds=12):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            self.healthy()
            if predicate():
                return
            time.sleep(0.05)
        raise AssertionError(f"{self.name}: timed out waiting for {description}")

    def settle(self, seconds):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            self.healthy()
            time.sleep(min(0.1, max(0, end - time.monotonic())))

    def control(self, method, signature="()", arguments=()):
        return call(self.bus, SERVICE, CONTROL_PATH, CONTROL, method, signature, arguments)

    def set_mode(self, mode):
        self.control("SetCase", "(ss)", (self.name, mode))

    def set_power(self, percent, charge="Discharging", online=False, present=True):
        for name, values in {
            "BAT0": {"type": "Battery", "scope": "System", "present": int(present),
                     "capacity": percent, "status": charge},
            "AC": {"type": "Mains", "scope": "System", "online": int(online)},
        }.items():
            directory = self.args.power / name
            directory.mkdir(exist_ok=True)
            for key, value in values.items():
                atomic_text(directory / key, f"{value}\n")
        self.event("PowerFixture", percent=percent, charge=charge, online=online, present=present)

    def marker(self):
        path = self.runtime / MARKER
        return path.read_text() if path.exists() else None

    def assert_inhibit(self, kind):
        assert self.marker() == f"alcard-battery-inhibit-v1:{kind}\n", self.marker()

    def screen(self):
        path = self.out / "last-observed.png"
        subprocess.run(["grim", "-o", self.output, str(path)], env=self.environment,
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5)
        return path

    def screen_has(self, *texts):
        image = self.screen()
        return all(self.observer.contains(image, text) for text in texts)

    def wait_text(self, *texts):
        self.wait("visible " + repr(texts), lambda: self.screen_has(*texts))

    def capture(self, name):
        assert not self.matching("NativeLocked"), "No screenshots of the credential client's surface"
        target = self.out / f"{name}.png"
        shutil.copyfile(self.screen(), target)
        self.captures.append(dict(path=str(target.relative_to(self.args.out)),
                                  sha256=sha256(target), width=320, height=170))

    def key(self, raw, symbol):
        # The Shell consumes evdev codes. wtype allocates its temporary keymap
        # sequentially, so these released placeholders reproduce the real code.
        command = ["wtype", "-s", "100"]
        for slot in range(1, raw):
            command.extend(["-p", f"U{0xE000 + slot:04X}"])
        command.extend(["-s", "100", "-P", symbol, "-s", "30", "-p", symbol, "-s", "100"])
        subprocess.run(command, env=self.environment, check=True, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=5)
        self.event("PublicNavigation", evdev=raw, symbol=symbol)

    def quick(self, index, *, activate=True):
        self.key(6, "5")
        self.wait_text("Quick settings")
        for _ in range(8):
            self.key(103, "Up")
        for _ in range(index):
            self.key(108, "Down")
        if activate:
            self.key(5, "4")

    def _protocol(self, stream, log_path, pid):
        allowed = re.compile(
            rb"(?:ext_idle_notifier_v1|ext_idle_notification_v1|"
            rb"zwlr_output_power_manager_v1|zwlr_output_power_v1)[@#][0-9]+\.[a-z_]+,?\s*\(")
        focus = re.compile(rb"wl_keyboard[@#][0-9]+\.(enter|leave),?\s*\(")
        with log_path.open("ab") as public:
            for line in stream:
                if allowed.search(line):
                    self.event("ShellProtocol", pid=pid, line=line.decode(errors="replace").strip()[:1024])
                elif (matched := focus.search(line)):
                    # Keep only focus state. No raw keyboard event, keymap,
                    # identity, prompt or authentication response is stored.
                    self.event("ShellFocus", pid=pid, focused=matched[1] == b"enter")
                elif not re.match(rb"^\s*\[[0-9 .]+\]", line):
                    public.write(line[:4096])
                    public.flush()

    def start_shell(self, *, expect_home=True):
        acquired_before_start = len(self.matching("DelayAcquired"))
        self.shell_number += 1
        environment = dict(self.environment, WAYLAND_DEBUG="1")
        self.shell = self.spawn([str(self.base / "staged/alcard-shell"), "--theme", "classic", "--locale", "en"],
                                f"shell-{self.shell_number}", environment=environment, stderr=subprocess.PIPE)
        process = self.shell
        self.event("ShellStarted", pid=process.pid, generation=self.shell_number,
                   binary_sha256=sha256(self.args.shell))
        thread = threading.Thread(target=self._protocol, args=(
            process.stderr, self.out / f"shell-{self.shell_number}-public.log", process.pid), daemon=True)
        thread.start()
        self.threads.append(thread)

        def registered():
            try:
                return call(self.session_bus, "org.freedesktop.DBus", "/org/freedesktop/DBus",
                            "org.freedesktop.DBus", "GetConnectionUnixProcessID",
                            "(s)", ("org.freedesktop.Notifications",)) == (process.pid,)
            except Exception:
                return False
        self.wait("new Shell notification service", registered)
        self.wait("this Shell's login1 delay lease",
                  lambda: len(self.matching("DelayAcquired")) > acquired_before_start)
        pointer = os.environ.get("ALCARD_POINTER_BIN")
        if pointer and expect_home:
            subprocess.run([pointer], env=dict(self.environment, ALCARD_POINTER_FIXTURE="1"),
                           check=True, timeout=5, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if expect_home:
            self.wait_text("ALCARD", "Quick")
            self.wait("initial Shell keyboard focus",
                      lambda: bool(self.matching("ShellFocus", pid=process.pid, focused=True)))

    def stop_shell(self):
        if self.shell:
            process, self.shell = self.shell, None
            self.terminate(process)
            self.event("ShellStopped", pid=process.pid)

    @staticmethod
    def terminate(process):
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=3)

    def _slow_capacity(self):
        path = self.args.power / "BAT0/capacity"

        def writer():
            while not self.stop.is_set():
                try:
                    return os.open(path, os.O_WRONLY | os.O_NONBLOCK)
                except OSError as error:
                    if error.errno != errno.ENXIO:
                        raise
                    time.sleep(0.02)
            return None

        def next_fifo():
            temporary = path.with_name("capacity.fixture-next-fifo")
            os.mkfifo(temporary, 0o644)
            temporary.replace(path)

        # Each pass gets its own FIFO inode. Reopening one FIFO too quickly
        # could attach a second writer before the first reader has seen EOF.
        # The source identity is the BAT0 directory, which never changes.
        for index in range(3):
            descriptor = writer()
            if descriptor is None:
                return
            try:
                if index == 0:
                    self.event("SlowSampleOpened")
                    self.fifo_opened.set()
                    while not self.stop.is_set() and not self.fifo_release.wait(0.05):
                        pass
                elif index == 2:
                    self.event("FreshSampleOpened")
                    while not self.stop.is_set() and not self.fifo_fresh_release.wait(0.05):
                        pass
                if index < 2:
                    next_fifo()
                else:
                    atomic_text(path, "5\n")
                os.write(descriptor, b"5\n")
            finally:
                os.close(descriptor)
            if index == 1:
                self.event("SlowSampleReleased")
                self.fifo_done.set()
            elif index == 2:
                self.event("FreshSampleReleased")

    def __enter__(self):
        try:
            self.base.mkdir(mode=0o700)
            self.out.mkdir(parents=True, exist_ok=True)
            self.journal.write_text("")
            self.runtime.mkdir(mode=0o700)
            for name in ("config/alcard", "config/labwc", "data/applications", "empty", "staged"):
                (self.base / name).mkdir(parents=True, exist_ok=True)
            self.environment = dict(os.environ,
                XDG_RUNTIME_DIR=str(self.runtime), XDG_CONFIG_HOME=str(self.base / "config"),
                XDG_CONFIG_DIRS=str(self.base / "empty"), XDG_DATA_HOME=str(self.base / "data"),
                XDG_DATA_DIRS=str(self.base / "empty"), XDG_SESSION_ID="not-the-current-session",
                WLR_BACKENDS="headless", WLR_RENDERER="pixman", WLR_HEADLESS_OUTPUTS="1",
                WLR_LIBINPUT_NO_DEVICES="1", WLR_XWAYLAND="", XKB_DEFAULT_LAYOUT="us",
                ALCARD_BATTERY_HELPER=str(HERE), ALCARD_BATTERY_LOCK_REAL=str(self.args.locker),
                ALCARD_BATTERY_JOURNAL=str(self.journal), ALCARD_BATTERY_CASE=self.name)
            for variable in ("WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DISPLAY", "WAYLAND_DEBUG"):
                self.environment.pop(variable, None)
            if self.hold_lock:
                self.environment["ALCARD_BATTERY_LOCK_GATE"] = str(self.gate)
            else:
                self.environment.pop("ALCARD_BATTERY_LOCK_GATE", None)
            for root, allowed in ((self.args.power, {"BAT0", "AC"}),
                                  (self.args.backlight, {"alcard-test-backlight"})):
                for child in root.iterdir():
                    assert child.name in allowed and child.is_dir() and not child.is_symlink(), child
                    shutil.rmtree(child)
            self.set_power(self.percent)
            backlight = self.args.backlight / "alcard-test-backlight"
            backlight.mkdir()
            for attribute, value in (("max_brightness", 100), ("brightness", 50), ("actual_brightness", 50)):
                atomic_text(backlight / attribute, f"{value}\n")
            config = "theme='classic'\nlocale='en'\n[idle]\nlock_after_seconds=300\nscreen_off_after_seconds=360\n"
            if self.automatic:
                config += "[battery]\nsuspend_on_critical=true\n"
            (self.base / "config/alcard/shell.toml").write_text(config)
            shutil.copy2(self.args.shell, self.base / "staged/alcard-shell")
            assert sha256(self.args.shell) == sha256(self.base / "staged/alcard-shell")
            wrapper = self.base / "staged/alcard-lock"
            wrapper.write_text('#!/bin/sh\nexec python3 "$ALCARD_BATTERY_HELPER" locker -- "$@"\n')
            wrapper.chmod(0o755)
            configuration = self.base / "private-bus.conf"
            socket = self.runtime / "system-bus"
            configuration.write_text(f"""<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:path={socket}</listen><auth>EXTERNAL</auth>
<policy context="default"><allow user="{os.getuid()}"/><allow own="*"/>
<allow send_destination="*"/><allow receive_sender="*"/></policy></busconfig>""")
            self.spawn(["dbus-daemon", "--nofork", f"--config-file={configuration}", "--print-address=1"], "private-bus")
            address_file = self.out / "private-bus.log"
            self.wait("private bus socket", lambda: socket.is_socket() and address_file.stat().st_size > 0)
            address = address_file.read_text().splitlines()[0]
            assert address.startswith(f"unix:path={socket},")
            self.environment["DBUS_SYSTEM_BUS_ADDRESS"] = address
            ready = self.base / "fixture-ready"
            self.spawn([sys.executable, str(HERE), "login1", "--address", address,
                        "--journal", str(self.journal), "--ready", str(ready),
                        "--backlight", str(self.args.backlight)], "fixture")
            self.wait("private fixture owner", ready.exists)
            self.bus = connection(address)
            self.session_bus = connection(os.environ["DBUS_SESSION_BUS_ADDRESS"])
            self.set_mode(self.mode)
            labwc = self.base / "config/labwc"
            (labwc / "rc.xml").write_text("""<?xml version="1.0"?>
<labwc_config><focus><followMouse>no</followMouse></focus><keyboard><default/></keyboard>
<windowRules><windowRule identifier="alcard-shell" serverDecoration="no" event="onFirstMap">
<action name="ToggleFullscreen"/></windowRule></windowRules></labwc_config>""")
            for script in ("autostart", "shutdown"):
                path = labwc / script
                path.write_text("#!/bin/sh\nexit 0\n")
                path.chmod(0o700)
            self.spawn(["labwc", "-C", str(labwc)], "compositor")

            def wayland_socket():
                found = [path for path in self.runtime.glob("wayland-*") if path.is_socket()]
                if found:
                    self.environment["WAYLAND_DISPLAY"] = found[0].name
                    return True
                return False
            self.wait("headless Wayland socket", wayland_socket)
            outputs = subprocess.check_output(["wlr-randr"], env=self.environment, text=True, timeout=5)
            names = [line.split()[0] for line in outputs.splitlines() if line and not line[0].isspace()]
            assert len(names) == 1 and names[0].startswith("HEADLESS-"), names
            self.output = names[0]
            subprocess.run(["wlr-randr", "--output", self.output, "--custom-mode", "320x170@60Hz",
                            "--scale", "1", "--pos", "0,0"], env=self.environment, check=True, timeout=5)
            self.spawn(["wtype", "-s", "400000"], "keyboard",
                       environment=dict(self.environment, WAYLAND_DEBUG="1"))

            def keyboard_ready():
                log = (self.out / "keyboard.log").read_text()
                keymap = re.search(r"zwp_virtual_keyboard_v1[^\n]*\.keymap\(", log)
                return keymap and re.search(r"wl_callback[^\n]*\.done\(", log[keymap.end():])
            self.wait("persistent virtual keyboard keymap acknowledgement", keyboard_ready)
            if self.stale:
                capacity = self.args.power / "BAT0/capacity"
                capacity.unlink()
                os.mkfifo(capacity, 0o644)
                thread = threading.Thread(target=self._slow_capacity, daemon=True)
                thread.start()
                self.threads.append(thread)
            self.start_shell()
            return self
        except BaseException:
            self.close()
            raise

    def check_no_sleep(self, seconds=6, *, allow_lock=False):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            self.healthy()
            verify_suspend_receipts(self.events(), 0)
            if not allow_lock:
                assert not self.matching("LockerStarted"), "Unexpected locker launch"
            time.sleep(0.1)

    def await_suspend(self):
        self.wait("one guarded SuspendWithFlags", lambda: len(self.calls("SuspendWithFlags")) >= 1)
        verify_suspend_receipts(self.events(), 1)
        self.wait("delay lease released after PrepareForSleep(true)",
                  lambda: bool(self.matching("DelayReleased", preparing=True)))

    def protocol_count(self, method):
        return sum(method in row["line"] for row in self.matching("ShellProtocol"))

    def resume(self):
        acquired = len(self.matching("DelayAcquired"))
        power_objects = self.protocol_count(".get_output_power")
        idle_objects = self.protocol_count(".get_idle_notification")
        assert power_objects > 0 and idle_objects > 0, "Expected actual idle/output-power protocol bindings"
        self.control("Prepare", "(b)", (False,))
        self.wait("new post-resume delay inhibitor",
                  lambda: len(self.matching("DelayAcquired")) > acquired)
        self.wait("fresh post-resume output-power binding",
                  lambda: self.protocol_count(".get_output_power") > power_objects)
        self.wait("fresh post-resume idle interval",
                  lambda: self.protocol_count(".get_idle_notification") > idle_objects)
        assert len(self.matching("LockerStarted")) == 1, "Resume spawned another locker"
        assert not self.matching("NativeUnlocked"), "Resume unlocked the real compositor"
        self.assert_native_alive()

    def assert_native_alive(self):
        native = self.matching("LockerStarted")
        assert len(native) == 1, native
        pid = native[0]["pid"]
        os.kill(pid, 0)
        # The hardened locker is nondumpable; /proc/<pid>/exe may be unreadable.
        # Its actual canonical LOCKED receipt plus process liveness is required.
        assert self.matching("NativeLocked", pid=pid)
        assert not self.matching("NativeUnlocked", pid=pid)

    def close(self):
        self.stop.set()
        self.fifo_release.set()
        self.fifo_fresh_release.set()
        if self.gate.parent.exists():
            self.gate.touch()
        self.stop_shell()
        # Closing this test compositor ends only this virtual lock session.
        for name, process in reversed(self.processes):
            if name not in {"fixture", "private-bus"}:
                self.terminate(process)
        for row in self.matching("LockerStarted"):
            # These exact children were spawned in this fresh case. Stop them
            # only after its compositor is gone, never during restart checks.
            for pid in (row["pid"], row["wrapper_pid"]):
                with contextlib.suppress(ProcessLookupError):
                    os.kill(pid, signal.SIGTERM)
        for bus in (self.bus, self.session_bus):
            if bus:
                with contextlib.suppress(Exception):
                    bus.close_sync(None)
        for _, process in reversed(self.processes):
            self.terminate(process)
        for thread in self.threads:
            thread.join(timeout=2)
        for log in self.logs:
            log.close()

    def __exit__(self, exc_type, error, _traceback):
        if error:
            self.event("CaseFailed", message=str(error))
        else:
            self.event("CasePassed")
        self.close()


def default_warnings(runtime):
    runtime.capture("01-home-above-threshold")
    runtime.set_power(20)
    runtime.wait_text("Low battery")
    runtime.capture("02-default-low")
    runtime.set_power(5)
    runtime.wait_text("Critical battery")
    runtime.capture("03-default-critical")
    runtime.check_no_sleep()
    assert runtime.marker() is None, "Default warnings consumed an automatic-action guard"


def automatic_resume_restart(runtime):
    runtime.set_power(5)
    runtime.await_suspend()
    runtime.assert_inhibit("attempt")
    runtime.resume()
    runtime.settle(6)
    verify_suspend_receipts(runtime.events(), 1)
    runtime.stop_shell()
    runtime.start_shell(expect_home=False)
    runtime.settle(6)
    verify_suspend_receipts(runtime.events(), 1)
    runtime.assert_inhibit("attempt")
    runtime.assert_native_alive()


def stale_measurement(runtime):
    runtime.wait("status worker pinned the slow capacity FIFO", runtime.fifo_opened.is_set)
    runtime.check_no_sleep(16.2)
    runtime.fifo_release.set()
    runtime.wait("old measurement completed", runtime.fifo_done.is_set)
    opened = runtime.matching("SlowSampleOpened")[0]["monotonic_ns"]
    released = runtime.matching("SlowSampleReleased")[0]["monotonic_ns"]
    assert released - opened > 15_000_000_000
    # A third FIFO holds the next sample even if startup queued a refresh.
    # Only the completed old sample can reach the UI during this assertion.
    runtime.check_no_sleep(1)
    assert not runtime.screen_has("Critical battery"), "Stale sample emitted a warning"
    runtime.capture("01-stale-sample-rejected")
    runtime.fifo_fresh_release.set()
    runtime.await_suspend()
    runtime.assert_inhibit("attempt")
    runtime.resume()


def charging_cancel(runtime):
    runtime.set_power(5)
    runtime.wait("real lock held before its canonical Shell receipt",
                 lambda: bool(runtime.matching("NativeLocked")))
    runtime.assert_inhibit("attempt")
    runtime.set_power(5, "Charging", online=True)
    # The journal is cleared only after policy observes confirmed charging.
    runtime.wait("charging observation rearms the journal", lambda: runtime.marker() is None, seconds=7)
    runtime.gate.touch()
    runtime.check_no_sleep(1, allow_lock=True)
    runtime.assert_native_alive()
    # A real new discharge episode may reuse the still-locked session once.
    runtime.set_power(5)
    runtime.await_suspend()
    assert len(runtime.matching("LockerStarted")) == 1
    runtime.resume()


def unknown_cancel(runtime):
    runtime.set_power(5)
    runtime.wait("real lock before delayed lifecycle forwarding",
                 lambda: bool(runtime.matching("NativeLocked")))
    runtime.assert_inhibit("attempt")
    runtime.set_power("invalid", "Unknown")
    runtime.settle(6)
    runtime.gate.touch()
    runtime.check_no_sleep(1, allow_lock=True)
    runtime.set_power(5)
    runtime.check_no_sleep(6, allow_lock=True)
    runtime.assert_inhibit("attempt")
    runtime.assert_native_alive()


def failed_attempt(runtime):
    runtime.set_power(5)
    runtime.wait("one attempted guarded mutation", lambda: len(runtime.calls("SuspendWithFlags")) == 1)
    verify_suspend_receipts(runtime.events(), 1)
    runtime.assert_inhibit("attempt")
    runtime.set_power("invalid", "Unknown")
    runtime.settle(6)
    runtime.set_mode("allowed")
    runtime.set_power(5)
    runtime.settle(6)
    verify_suspend_receipts(runtime.events(), 1)
    runtime.assert_inhibit("attempt")
    runtime.assert_native_alive()


def refused_sleep(runtime):
    capabilities = len(runtime.calls("CanSuspend"))
    runtime.set_power(5)
    # Exercise the normal explicit reload/refresh path instead of adding a
    # five-second wait to each negative case. The valid config is unchanged.
    runtime.shell.send_signal(signal.SIGHUP)
    runtime.event("ProductionReloadRequested", signal="SIGHUP")
    runtime.wait("automatic attempt guard", lambda: runtime.marker() ==
                 "alcard-battery-inhibit-v1:attempt\n")
    runtime.wait("fresh automatic capability check",
                 lambda: len(runtime.calls("CanSuspend")) > capabilities)
    runtime.check_no_sleep(0.3)
    if runtime.mode == "challenge":
        assert runtime.matching("CapabilityReturned", value="challenge")
        diagnostic = "Authorization is required."
    else:
        assert runtime.mode == "blocked"
        assert runtime.matching("InhibitorsReturned", blocked=True)
        diagnostic = "The system or another"
    # This is a new Shell with its initial menu selection. Navigate directly
    # to Suspend; unavailable capability must open explanation, not consent.
    runtime.key(6, "5")
    runtime.wait_text("Quick settings")
    for _ in range(8):
        runtime.key(108, "Down")
    runtime.key(5, "4")
    runtime.wait_text("Suspend", diagnostic)
    assert not runtime.screen_has("Suspend the device?"), "Unavailable sleep offered confirmation"
    runtime.capture("01-manual-suspend-refused")
    runtime.key(28, "Return")
    runtime.check_no_sleep(0.3)
    runtime.assert_inhibit("attempt")
    verify_sleep_refusal(runtime.events())


def manual_suspend(runtime):
    runtime.quick(8)
    runtime.wait_text("Suspend the device?")
    runtime.capture("01-manual-confirmation")
    verify_suspend_receipts(runtime.events(), 0)
    assert not runtime.matching("LockerStarted"), "Confirmation opened the locker before consent"
    runtime.key(28, "Return")
    runtime.await_suspend()
    runtime.resume()
    verify_suspend_receipts(runtime.events(), 1)


def external_suspend(runtime):
    runtime.capture("01-original-home")
    initial_focus = len(runtime.matching("ShellFocus", pid=runtime.shell.pid, focused=True))
    left = len(runtime.matching("ShellFocus", pid=runtime.shell.pid, focused=False))
    command = ["foot", "--app-id=alcard-battery-foreground", "--title=Battery fixture"]
    if Path("/etc/alcard/foot.ini").is_file():
        command.append("--config=/etc/alcard/foot.ini")
    command.extend(["--", "/bin/sh", "-c", "printf 'Private battery acceptance window\\n'; exec sleep 180"])
    foreground = runtime.spawn(command, "foreground")
    runtime.wait("ordinary application takes focus",
                 lambda: len(runtime.matching("ShellFocus", pid=runtime.shell.pid, focused=False)) > left)
    assert foreground.poll() is None
    runtime.capture("02-unfocused-shell")
    for cycle in (1, 2):
        released = len(runtime.matching("DelayReleased", preparing=True))
        runtime.control("Prepare", "(b)", (True,))
        runtime.wait("native lock before external delay release",
                     lambda: bool(runtime.matching("NativeLocked")))
        if cycle == 1:
            # Native presence alone is advisory. Keep its canonical receipt
            # away from Shell for many fixture EOF-poll intervals and prove
            # the delay lease is still held, then forward the real receipt.
            runtime.settle(1)
            assert len(runtime.matching("DelayReleased", preparing=True)) == released, (
                "External delay was released before canonical LOCKED reached Shell"
            )
            runtime.gate.touch()
        runtime.wait("external-cycle inhibitor EOF",
                     lambda: len(runtime.matching("DelayReleased", preparing=True)) > released)
        events = runtime.events()
        current_release = runtime.matching("DelayReleased", preparing=True)[-1]
        assert any(row["event"] == "NativeLocked" and
                   row["monotonic_ns"] < current_release["monotonic_ns"] for row in events)
        assert any(row["event"] == "LockReceiptForwarded" and row["lifecycle"] == "LOCKED" and
                   row["monotonic_ns"] < current_release["monotonic_ns"] for row in events)
        assert current_release["cycle"] == cycle
        verify_suspend_receipts(events, 0)
        runtime.resume()
    runtime.settle(0.3)
    assert len(runtime.matching("ShellFocus", pid=runtime.shell.pid, focused=True)) == initial_focus, (
        "External sleep/resume activated the unfocused Shell"
    )
    assert len(runtime.matching("LockerStarted")) == 1
    verify_suspend_receipts(runtime.events(), 0)


def brightness_controls(runtime):
    runtime.quick(0, activate=False)
    runtime.wait_text("Brightness", "50%")
    runtime.capture("01-initial-brightness")
    runtime.key(106, "Right")
    runtime.wait_text("Brightness", "55%")
    runtime.capture("02-readback-55")
    writes = runtime.calls("SetBrightness")
    assert len(writes) == 1 and writes[0]["arguments"] == ["backlight", "alcard-test-backlight", 55], writes
    assert writes[0]["destination"] == writes[0]["owner"] and writes[0]["flags"] & 4 == 0
    assert (Path("/sys/class/backlight/alcard-test-backlight/brightness").read_text().strip()) == "55"
    runtime.set_mode("brightness-denied")
    runtime.key(106, "Right")
    runtime.wait_text("No permission to change")
    runtime.capture("03-denied-unchanged")
    assert len(runtime.calls("SetBrightness")) == 2
    assert len(runtime.matching("BrightnessWritten")) == 1
    assert (runtime.args.backlight / "alcard-test-backlight/brightness").read_text().strip() == "55"
    runtime.set_mode("allowed")
    runtime.key(8, "7")
    runtime.wait("explicit brightness refresh restores permission",
                 lambda: runtime.screen_has("Brightness", "55%") and
                 not runtime.screen_has("No permission to change"))
    runtime.key(106, "Right")
    runtime.wait_text("Brightness", "60%")
    runtime.capture("04-explicit-refresh-recovery")
    assert [row["value"] for row in runtime.matching("BrightnessWritten")] == [55, 60]
    verify_suspend_receipts(runtime.events(), 0)


def verify_current_session(events):
    pids = {row["pid"] for row in events if row["event"] == "ShellStarted"}
    probes = [row for row in events if row["event"] == "Call" and row.get("method") == "GetSessionByPID"]
    assert probes and all(row["arguments"] in [[pid] for pid in pids] for row in probes), probes
    delays = [row for row in events if row["event"] == "Call" and row.get("method") == "Inhibit"]
    assert delays and all(row["arguments"][0] == "sleep" and row["arguments"][3] == "delay" for row in delays)
    assert all(row["flags"] & 4 == 0 for row in delays)


def run(args):
    assert os.getuid() != 0 and os.getuid() == os.geteuid(), "Runtime must run as an ordinary user"
    assert os.environ.get("ALCARD_BATTERY_TEST_CONTAINER") == "1" and Path("/.dockerenv").exists()
    assert os.environ.get("DBUS_SESSION_BUS_ADDRESS"), "A new private session bus is required"
    mounts = check_mounts(args.power, args.backlight, empty=True)
    args.out.mkdir(parents=True, exist_ok=True)
    report = dict(
        schema=1, status="running", scope="Alpine software runtime with simulated login1/sysfs",
        utc=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        architecture=os.uname().machine, alpine_release=Path("/etc/alpine-release").read_text().strip(),
        shell=dict(path=str(args.shell), sha256=sha256(args.shell)),
        locker=dict(path=str(args.locker), sha256=sha256(args.locker)),
        helper_sha256=sha256(HERE), mounts=mounts, cases=[], captures=[],
        kernel_suspend_executed=False, physical_hardware_tested=False,
        pam_authentication_tested=False,
        limitations=[
            "login1 replies, battery values and backlight writes are controlled fixture data.",
            "The production Shell and native ext-session-lock locker execute unchanged.",
            "A sibling wrapper observes/forwards only real canonical locker lifecycle lines.",
            "Cancellation cases delay forwarding a real LOCKED receipt; no LOCKED receipt is invented.",
            "The stale sample blocks a real sysfs read for over 15 seconds using a fixture FIFO.",
            "No password is entered. Post-resume native lock and focus are observed; post-authentication view restoration belongs to the separate PAM gate.",
            "No real kernel suspend, physical discharge curve, board wake source or Cardputer result is asserted.",
        ])
    destination = args.out / "result.json"
    destination.write_text(json.dumps(report, indent=2) + "\n")
    cases = [
        ("default-warnings", default_warnings, dict(percent=21)),
        ("automatic-resume-restart", automatic_resume_restart, dict(automatic=True)),
        ("stale-measurement", stale_measurement, dict(automatic=True, percent=5, stale=True)),
        ("charging-cancel-rearm", charging_cancel, dict(automatic=True, hold_lock=True)),
        ("unknown-cancel-no-retry", unknown_cancel, dict(automatic=True, hold_lock=True)),
        ("failed-attempt-no-retry", failed_attempt, dict(automatic=True, mode="unknown")),
        ("blocking-inhibitor-refusal", refused_sleep, dict(automatic=True, mode="blocked")),
        ("authorization-challenge-refusal", refused_sleep, dict(automatic=True, mode="challenge")),
        ("manual-suspend", manual_suspend, {}),
        ("external-suspend", external_suspend, dict(hold_lock=True)),
        ("brightness-controls", brightness_controls, {}),
    ]
    try:
        for name, exercise, options in cases:
            started = time.monotonic()
            runtime = Runtime(args, name, **options)
            case = dict(name=name, status="running")
            report["cases"].append(case)
            try:
                with runtime:
                    exercise(runtime)
                    verify_current_session(runtime.events())
                case["status"] = "passed"
                print(f"PASS: {name}", flush=True)
            except BaseException as error:
                case.update(status="failed", error=str(error))
                raise
            finally:
                case["duration_seconds"] = round(time.monotonic() - started, 3)
                report["captures"].extend(runtime.captures)
                destination.write_text(json.dumps(report, indent=2) + "\n")
        report["status"] = "passed"
    except BaseException as error:
        report.update(status="failed", error=str(error))
        raise
    finally:
        destination.write_text(json.dumps(report, indent=2) + "\n")
    print("PASS: production Shell/locker software runtime; fixture sleep only, no kernel/hardware claim.", flush=True)


def self_check(with_gio=False):
    """Offline fixture validation only. It is explicitly not runtime acceptance."""
    node = ET.fromstring(XML)
    manager = node.find(f"./interface[@name='{MANAGER}']")
    assert manager is not None
    required = {
        "GetSessionByPID": ("u", "o"), "CanSuspend": ("", "s"),
        "SuspendWithFlags": ("t", ""), "ListInhibitors": ("", "a(ssssuu)"),
        "Inhibit": ("ssss", "h"),
    }
    for name, signatures in required.items():
        method = manager.find(f"./method[@name='{name}']")
        assert method is not None
        observed = tuple("".join(arg.attrib["type"] for arg in method.findall("arg")
                                 if arg.attrib.get("direction", "in") == direction)
                         for direction in ("in", "out"))
        assert observed == signatures, (name, observed)
    mount = mount_rows("42 1 0:8 /fixture /sys/class/power_supply ro,relatime - overlay overlay rw\n")[0]
    assert mount["target"] == "/sys/class/power_supply" and mount["options"] == ["ro", "relatime"]
    base = [
        dict(event="NativeLocked", monotonic_ns=1),
        dict(event="LockReceiptForwarded", lifecycle="LOCKED", monotonic_ns=2),
        dict(event="Call", method="SuspendWithFlags", monotonic_ns=3,
             arguments=[1], flags=0, destination=":1.42", owner=":1.42"),
    ]
    verify_suspend_receipts(base, 1)
    broken = [base[2:], base[:1] + base[2:], base + [base[-1]],
              base[:-1] + [base[-1] | {"arguments": [0]}],
              base[:-1] + [base[-1] | {"flags": 4}],
              base[:-1] + [base[-1] | {"destination": SERVICE}]]
    for events in broken:
        try:
            verify_suspend_receipts(events, 1)
        except AssertionError:
            pass
        else:
            raise AssertionError("Receipt checker accepted an invalid sequence")
    verify_sleep_refusal([dict(event="Call", method="CanSuspend", flags=0)])
    for events in ([base[-1]], [dict(event="LockerStarted")],
                   [dict(event="Call", method="CanSuspend", flags=4)]):
        try:
            verify_sleep_refusal(events)
        except AssertionError:
            pass
        else:
            raise AssertionError("Refusal checker accepted mutation, locking or interactive auth")
    # Exercise only the fixture's FIFO handoff, with ordinary local files and
    # threads. No application, compositor, D-Bus socket or system path is used.
    with tempfile.TemporaryDirectory(prefix="alcard-battery-fifo-check-") as temporary:
        power = Path(temporary)
        journal = power / "calls.jsonl"
        record(journal, "fixture-check", "Call", path=MANAGER_PATH, method="GetSessionByPID")
        assert rows(journal)[0]["path"] == MANAGER_PATH, "D-Bus object path collided with journal path"
        (power / "BAT0").mkdir()
        capacity = power / "BAT0/capacity"
        os.mkfifo(capacity, 0o600)
        runtime = object.__new__(Runtime)
        runtime.args = SimpleNamespace(power=power)
        runtime.stop = threading.Event()
        runtime.fifo_opened = threading.Event()
        runtime.fifo_release = threading.Event()
        runtime.fifo_fresh_release = threading.Event()
        runtime.fifo_done = threading.Event()
        runtime.event = lambda *_args, **_kwargs: None
        observed = []
        old_done, fresh_done = threading.Event(), threading.Event()

        def read_samples():
            for index in range(3):
                observed.append(capacity.read_text())
                if index == 1:
                    old_done.set()
            fresh_done.set()

        producer = threading.Thread(target=runtime._slow_capacity, daemon=True)
        reader = threading.Thread(target=read_samples, daemon=True)
        producer.start()
        reader.start()
        try:
            assert runtime.fifo_opened.wait(2), "First FIFO did not rendezvous"
            assert observed == [], "Slow sample was not held"
            runtime.fifo_release.set()
            assert old_done.wait(2), "Both coherent old passes did not finish"
            assert observed == ["5\n", "5\n"]
            assert not fresh_done.is_set(), "Queued refresh bypassed the fresh-sample gate"
            runtime.fifo_fresh_release.set()
            assert fresh_done.wait(2), "Fresh sample did not finish"
            assert observed == ["5\n"] * 3 and capacity.read_text() == "5\n"
        finally:
            runtime.stop.set()
            runtime.fifo_release.set()
            runtime.fifo_fresh_release.set()
            producer.join(timeout=2)
            reader.join(timeout=2)
    if with_gio:
        Gio, GLib = gi()
        assert len(Gio.DBusNodeInfo.new_for_xml(XML).interfaces) == 4
        assert GLib.Variant("(a(ssssuu))", ([],)).unpack() == ([],)
        assert GLib.Variant("(so)", ("c33", SESSION_PATH)).unpack() == ("c33", SESSION_PATH)
        assert GLib.Variant("(uo)", (os.getuid(), "/org/freedesktop/login1/user/_1")).unpack()[0] == os.getuid()
        read_fd, write_fd = os.pipe2(os.O_NONBLOCK | os.O_CLOEXEC)
        try:
            descriptors = Gio.UnixFDList.new()
            assert descriptors.append(write_fd) == 0 and descriptors.get_length() == 1
            duplicate = descriptors.get(0)
            assert stat.S_ISFIFO(os.fstat(duplicate).st_mode)
            os.close(duplicate)
            assert hasattr(Gio.DBusMethodInvocation, "return_value_with_unix_fd_list")
        finally:
            os.close(write_fd)
            os.close(read_fd)
    print("PASS: offline fixture XML/signatures/receipt rejection/FIFO handoff" +
          ("/GIO FD bindings" if with_gio else "") +
          "; production runtime NOT RUN.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    mounts = subparsers.add_parser("mounts", help="Validate pre-mounted fixture identity and readonly exposure")
    mounts.add_argument("--power", type=Path, required=True)
    mounts.add_argument("--backlight", type=Path, required=True)
    mounts.add_argument("--empty", action="store_true")
    fixture = subparsers.add_parser("login1", help="Private simulated login1; no host backend")
    fixture.add_argument("--address", required=True)
    fixture.add_argument("--journal", type=Path, required=True)
    fixture.add_argument("--ready", type=Path, required=True)
    fixture.add_argument("--backlight", type=Path)
    native = subparsers.add_parser("locker", help="Observe only canonical production locker lifecycle")
    native.add_argument("arguments", nargs=argparse.REMAINDER)
    runtime = subparsers.add_parser("run", help="Execute all software runtime cases as an ordinary user")
    for option in ("root", "temporary", "out", "shell", "locker", "power", "backlight"):
        runtime.add_argument("--" + option, type=Path, required=True)
    offline = subparsers.add_parser("self-check", help="Offline fixture checks; never claim runtime acceptance")
    offline.add_argument("--gio", action="store_true")
    args = parser.parse_args()
    if args.command == "mounts":
        print(json.dumps(check_mounts(args.power, args.backlight, args.empty), indent=2))
    elif args.command == "login1":
        login1(args)
    elif args.command == "locker":
        locker(args.arguments)
    elif args.command == "self-check":
        self_check(args.gio)
    else:
        def interrupted(_signal, _frame):
            raise RuntimeError("Runtime acceptance interrupted before completion")
        signal.signal(signal.SIGTERM, interrupted)
        run(args)


if __name__ == "__main__":
    try:
        main()
    except Exception:
        traceback.print_exc()
        raise SystemExit(1)
