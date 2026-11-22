#!/usr/bin/env python3
"""Check the production shell audio lifecycle using disposable harmless children.

No audio server, compositor, session bus, package install or hardware operation
is performed. The temporary source copy replaces executable paths, the UID-0
guard and socket readiness only; all lifetime, locking, signals and cleanup
logic remain the production code. Readiness uses a regular marker because the
local environment may prohibit AF_UNIX bind. This does not validate PipeWire,
ALSA routing, the real socket ownership check or the installed target packages.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def stamp(pid: int) -> str:
    return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]


def live(pid: int) -> bool:
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
    except FileNotFoundError:
        return False


def wait_for(condition, description: str, timeout: float = 6) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if condition():
            return
        time.sleep(0.025)
    raise RuntimeError("Timed out: " + description)


FIXTURE = r'''
import json, os, pathlib, signal, sys, time
root = pathlib.Path(ROOT)
role = sys.argv[1] if len(sys.argv) > 1 else pathlib.Path(__file__).name
def event(kind):
    with (root / 'events.jsonl').open('a') as stream:
        stream.write(json.dumps({'role': role, 'kind': kind, 'pid': os.getpid(),
            'remote': os.environ.get('PIPEWIRE_REMOTE'),
            'runtime_override': os.environ.get('PIPEWIRE_RUNTIME_DIR'),
            'bus': os.environ.get('DBUS_SESSION_BUS_ADDRESS')}) + '\n')
def stopped(signum, frame):
    event('stopped')
    raise SystemExit(0)
signal.signal(signal.SIGTERM, signal.SIG_IGN if SCENARIO in ('ignore', 'watcher-killed-ignore') else stopped)
signal.signal(signal.SIGHUP, stopped)
event('started')
if role == 'pipewire' and SCENARIO == 'startup-fail':
    raise SystemExit(23)
if role == 'pipewire' and SCENARIO != 'startup-timeout':
    (root / 'runtime' / 'fixture-ready').touch()
if SCENARIO == 'fail-' + role:
    time.sleep(.25)
    raise SystemExit(17)
while True: signal.pause()
'''


def check_case(source: str, shell: str, setpriv: str, flock: str, scenario: str) -> None:
    with tempfile.TemporaryDirectory(prefix="alcard-audio-lifecycle-") as directory:
        root = Path(directory)
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        events_path = root / "events.jsonl"
        events_path.touch()
        for role in ("pipewire", "wireplumber", "pipewire-pulse"):
            path = root / role
            path.write_text(f"#!{sys.executable}\nROOT={str(root)!r}\nSCENARIO={scenario!r}\n" + FIXTURE)
            path.chmod(0o755)
        helper = root / "audio-session"
        replacements = {
            "#!/bin/sh": f"#!{shell}",
            "self=/usr/libexec/alcard/audio-session": "self=" + shlex.quote(str(helper)),
            '[ "$(id -u)" -ne 0 ]': '[ 1 -eq 1 ]',
            'exec "/usr/bin/$role"': f'exec "{root}/$role"',
            "/bin/setpriv": setpriv,
            "/usr/bin/flock": flock,
            "/usr/bin/pipewire-pulse": str(root / "pipewire-pulse"),
            "/usr/bin/wireplumber": str(root / "wireplumber"),
            "/usr/bin/pipewire": str(root / "pipewire"),
        }
        temporary = source
        for old, new in replacements.items():
            require(old in temporary, "Fixture transform no longer matches: " + old)
            temporary = temporary.replace(old, new)
        temporary, count = re.subn(
            r"socket_owned_by\(\) \{.*?\n\}\n\n# Closing",
            'socket_owned_by() {\n    [ -f "$XDG_RUNTIME_DIR/fixture-ready" ]\n}\n\n# Closing',
            temporary, flags=re.S,
        )
        require(count == 1, "Socket-readiness fixture transform no longer matches")
        if scenario == "startup-timeout":
            temporary = temporary.replace('[ "$tick" -lt 60 ]', '[ "$tick" -lt 5 ]')
        helper.write_text(temporary)
        helper.chmod(0o755)
        environment = os.environ.copy()
        environment.update(XDG_RUNTIME_DIR=str(runtime), DBUS_SESSION_BUS_ADDRESS="fixture-no-bus",
                           PIPEWIRE_RUNTIME_DIR="fixture-invalid-override", PIPEWIRE_REMOTE="fixture-remote")
        processes = []
        log = (root / "supervisor.log").open("w+")

        def events() -> list[dict]:
            return [json.loads(line) for line in events_path.read_text().splitlines() if line]

        def owned_pids() -> list[int]:
            return [row["pid"] for row in events() if row["kind"] == "started" and row["role"] != "foreign"]

        def spawn(command=None):
            if command is None:
                command = [setpriv, "--pdeathsig", "TERM", str(helper), str(os.getpid()), stamp(os.getpid())]
            process = subprocess.Popen(command, env=environment, stdin=subprocess.DEVNULL,
                                       stdout=log, stderr=log, start_new_session=True)
            processes.append(process)
            return process

        try:
            if scenario == "foreign-endpoint":
                (runtime / "pipewire-0").write_text("foreign endpoint fixture")
            if scenario == "pulse-symlink":
                (root / "outside").mkdir(mode=0o700)
                (runtime / "pulse").symlink_to(root / "outside", target_is_directory=True)
            if scenario == "wrong-parent":
                process = spawn([setpriv, "--pdeathsig", "TERM", str(helper), "1", "0"])
            elif scenario == "parent-death":
                parent = root / "owning-session.py"
                parent.write_text(
                    "import os, pathlib, signal, subprocess\n"
                    "pid=os.getpid()\n"
                    "stamp=pathlib.Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()[19]\n"
                    f"subprocess.Popen([{setpriv!r}, '--pdeathsig', 'TERM', {str(helper)!r}, str(pid), stamp])\n"
                    "while True: signal.pause()\n"
                )
                process = spawn([sys.executable, str(parent)])
            else:
                process = spawn()

            if scenario in ("wrong-parent", "foreign-endpoint", "pulse-symlink", "startup-fail", "startup-timeout"):
                require(process.wait(timeout=6) == 1, "Invalid startup was accepted")
                wait_for(lambda: all(not live(pid) for pid in owned_pids()), "startup cleanup")
                if scenario in ("wrong-parent", "foreign-endpoint", "pulse-symlink"):
                    require(not owned_pids(), "Rejected startup launched a daemon")
                if scenario == "foreign-endpoint":
                    require((runtime / "pipewire-0").read_text() == "foreign endpoint fixture",
                            "Foreign endpoint was changed")
                if scenario == "pulse-symlink":
                    require(not list((root / "outside").iterdir()), "Followed a foreign Pulse directory")
            else:
                wait_for(lambda: len(owned_pids()) == 3, "three harmless audio roles started")
                starts = [event for event in events() if event["kind"] == "started"]
                require(all(event["remote"] == "pipewire-0" and event["runtime_override"] is None
                            and event["bus"] == "fixture-no-bus" for event in starts),
                        "Audio child environment or existing session bus was not preserved")
                if scenario == "duplicate":
                    lock_inode = (runtime / "alcard-audio.lock").stat().st_ino
                    second = spawn()
                    require(second.wait(timeout=3) == 1, "Duplicate audio session was accepted")
                    require(len(owned_pids()) == 3 and process.poll() is None,
                            "Duplicate startup touched the first session")
                    process.terminate()
                    require(process.wait(timeout=6) == 0, "First session did not exit")
                    third = spawn()
                    wait_for(lambda: len(owned_pids()) == 6, "flock released across logout")
                    require((runtime / "alcard-audio.lock").stat().st_ino == lock_inode,
                            "Lifetime lock was replaced instead of reused")
                    third.terminate()
                    require(third.wait(timeout=6) == 0, "New session did not exit")
                elif scenario.startswith("fail-"):
                    foreign = spawn([str(root / "pipewire"), "foreign"])
                    require(process.wait(timeout=6) == 1, "Unexpected daemon exit was ignored")
                    require(foreign.poll() is None, "Unrelated user process was stopped")
                    foreign.terminate()
                    foreign.wait(timeout=3)
                elif scenario in ("watcher-killed", "watcher-killed-ignore"):
                    daemon = next(event["pid"] for event in starts if event["role"] == "wireplumber")
                    watcher = int(Path(f"/proc/{daemon}/stat").read_text().rsplit(")", 1)[1].split()[1])
                    require(watcher != process.pid, "Expected the separate owned WirePlumber watcher")
                    os.kill(watcher, signal.SIGKILL)
                    require(process.wait(timeout=7) == 1, "Hard-killed watcher left a partial stack")
                elif scenario in ("parent-death", "supervisor-killed"):
                    process.kill()
                    require(process.wait(timeout=3) == -signal.SIGKILL, "Disposable owner was not killed")
                else:
                    process.terminate()
                    require(process.wait(timeout=7) == 0, "Orderly shutdown did not finish")
                wait_for(lambda: all(not live(pid) for pid in owned_pids()), "owned child cleanup", timeout=6)
            print(f"PASS [{Path(shell).name}] {scenario}: owned lifetimes only, no real audio")
        except Exception:
            log.flush()
            print((root / "supervisor.log").read_text(), file=sys.stderr)
            raise
        finally:
            # Only new process groups containing these disposable fixtures.
            for process in processes:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=3)
            log.close()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--shell", default="/bin/sh")
    args = parser.parse_args()
    shell = shutil.which(args.shell)
    setpriv = shutil.which("setpriv")
    flock = shutil.which("flock")
    require(shell is not None and setpriv is not None and flock is not None,
            "The chosen shell, setpriv and flock must already be installed")
    source = (Path(__file__).resolve().parents[1] / "scripts/wayland/audio-session").read_text()
    for scenario in ("wrong-parent", "foreign-endpoint", "pulse-symlink", "startup-fail", "startup-timeout", "graceful",
                     "duplicate", "fail-pipewire", "fail-wireplumber", "fail-pipewire-pulse", "ignore",
                     "watcher-killed", "watcher-killed-ignore", "parent-death", "supervisor-killed"):
        check_case(source, shell, setpriv, flock, scenario)


if __name__ == "__main__":
    main()
