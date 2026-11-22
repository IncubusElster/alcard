#!/usr/bin/env python3
"""Exercise production supervision with harmless children in a temporary copy.

No desktop, PAM, system bus or real user service is started. Only the two fixed
executable paths and the ordinary-user bootstrap check are changed in the copy;
the production lifecycle, traps, backoff and termination code stay unchanged.
The real setpriv parent-death signal is exercised with a disposable parent.
Run with --shell /bin/ash in Alpine CI and /bin/sh for the host POSIX shell.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
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


def wait_for(description: str, condition, timeout: float = 7) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if condition():
            return
        time.sleep(0.025)
    raise RuntimeError(f"Timed out: {description}")


def live_session(session_id: int) -> set[int]:
    result = set()
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            pid = int(entry.name)
            fields = (entry / "stat").read_text().rsplit(")", 1)[1].split()
            if os.getsid(pid) == session_id and fields[0] != "Z":
                result.add(pid)
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue
    return result


FIXTURE = r'''
import json, os, pathlib, signal, subprocess, sys, time
root = pathlib.Path(ROOT)
role = sys.argv[1] if len(sys.argv) > 1 and ROLE != 'audio' else ROLE
scenario = SCENARIO
def event(kind):
    with (root / 'events.jsonl').open('a') as stream:
        stream.write(json.dumps({'role': role, 'kind': kind, 'pid': os.getpid()}) + '\n')
def stopped(signum, frame):
    event('stopped')
    raise SystemExit(0)
signal.signal(signal.SIGTERM, signal.SIG_IGN if (scenario == 'ignore' and role != 'audio') or role == 'locker' else stopped)
signal.signal(signal.SIGINT, signal.SIG_IGN)
event('started')
if scenario == 'audio-failed' and role == 'audio':
    raise SystemExit(4)
if role == 'locker':
    while True: signal.pause()
if scenario == 'graceful' and role == 'shell':
    subprocess.Popen([sys.executable, __file__, 'locker'])
if scenario == 'backoff':
    raise SystemExit(3)
if scenario == 'restart' and role == 'agent':
    marker = root / 'agent-failed-once'
    if not marker.exists():
        marker.touch()
        raise SystemExit(4)
if scenario == 'session-lost' and role == 'agent':
    time.sleep(.2)
    raise SystemExit(75)
while True: signal.pause()
'''


def check_case(shell: str, setpriv: str, source: str, scenario: str) -> None:
    with tempfile.TemporaryDirectory(prefix="alcard-session-supervision-") as directory:
        root = Path(directory)
        events_path = root / "events.jsonl"
        events_path.touch()
        fixtures = {}
        for role in ("shell", "agent", "audio"):
            path = root / f"fixture-{role}"
            path.write_text(
                f"#!{sys.executable}\nROOT={str(root)!r}\nROLE={role!r}\nSCENARIO={scenario!r}\n"
                + FIXTURE
            )
            path.chmod(0o755)
            fixtures[role] = path

        temporary = source
        replacements = {
            '[ "$#" -eq 0 ] && [ "$(id -u)" -ne 0 ] || exit 1': '[ "$#" -eq 0 ] || exit 1',
            "supervise /usr/bin/alcard-polkit-agent &": f"supervise {shlex.quote(str(fixtures['agent']))} &",
            "supervise /usr/bin/alcard-shell &": f"supervise {shlex.quote(str(fixtures['shell']))} &",
            "/usr/libexec/alcard/audio-session": shlex.quote(str(fixtures["audio"])),
            '[ "$1" = /usr/bin/alcard-polkit-agent ]':
                f'[ "$1" = {shlex.quote(str(fixtures["agent"]))} ]',
        }
        for old, new in replacements.items():
            require(temporary.count(old) == 1, f"Fixture transform no longer matches: {old}")
            temporary = temporary.replace(old, new, 1)
        if scenario in ("main-wait-race", "worker-wait-race"):
            gate = shlex.quote(str(root / "signal-now"))
            pause = f'while [ ! -f {gate} ]; do sleep 0.01; done\n'
            if scenario == "main-wait-race":
                marker = 'wait "$shell_worker" || result=$?'
                injected = pause + 'kill -TERM "$$"\n' + marker
            else:
                marker = '        wait "$child" || result=$?'
                injected = (pause + 'IFS= read -r fixture_stat < /proc/self/stat\n'
                            'fixture_pid=${fixture_stat%% *}\nkill -TERM "$fixture_pid"\n' + marker)
            require(temporary.count(marker) == 1, "Wait-race fixture transform no longer matches")
            temporary = temporary.replace(marker, injected, 1)
        client = root / "session-client"
        client.write_text(temporary)

        def events() -> list[dict]:
            return [json.loads(line) for line in events_path.read_text().splitlines() if line]

        def started(role: str) -> list[int]:
            return [event["pid"] for event in events() if event["role"] == role and event["kind"] == "started"]

        environment = os.environ.copy()
        environment.update(
            WAYLAND_DISPLAY="fixture-no-compositor",
            DBUS_SESSION_BUS_ADDRESS="fixture-no-bus",
            LABWC_PID=str(os.getpid()) if scenario != "wrong-parent" else "0",
        )
        command = [shell, str(client)]
        if scenario == "parent-death":
            parent = root / "compositor-parent.py"
            parent.write_text(
                "import os, signal, subprocess\n"
                "os.environ['LABWC_PID'] = str(os.getpid())\n"
                f"subprocess.Popen({[setpriv, '--pdeathsig', 'TERM', shell, str(client)]!r})\n"
                "while True: signal.pause()\n"
            )
            command = [sys.executable, str(parent)]
        with (root / "session.log").open("w+") as log:
            process = subprocess.Popen(
                command, env=environment, stdin=subprocess.DEVNULL,
                stdout=log, stderr=log, start_new_session=True,
            )
            try:
                if scenario == "wrong-parent":
                    require(process.wait(timeout=3) == 1, "Reparented bootstrap was accepted")
                    require(not events(), "Invalid compositor parent started services")
                    require(not live_session(process.pid), "Rejected bootstrap left processes")
                    print(f"PASS [{Path(shell).name}] {scenario}: no services started")
                    return
                wait_for("both harmless services started", lambda: started("shell") and started("agent"))
                if scenario in ("main-wait-race", "worker-wait-race"):
                    (root / "signal-now").touch()
                    require(process.wait(timeout=6) == 0, "Signal before wait left the supervisor blocked")
                    require(not live_session(process.pid), "Signal-before-wait left owned processes")
                    print(f"PASS [{Path(shell).name}] {scenario}: signal closed wait window")
                    return
                if scenario == "graceful":
                    wait_for("independent descendant started", lambda: started("locker"))
                elif scenario == "backoff":
                    wait_for("both supervisors entered backoff", lambda: (root / "session.log").read_text().count("retry in") >= 2)
                elif scenario == "restart":
                    wait_for("failed authentication service restarted", lambda: len(started("agent")) >= 2)
                elif scenario == "audio-failed":
                    wait_for("audio fixture started", lambda: started("audio"))
                    wait_for("audio failure left the desktop usable",
                             lambda: started("audio")[0] not in live_session(process.pid))
                    require(len(started("shell")) == len(started("agent")) == 1,
                            "Audio failure restarted the desktop services")
                elif scenario == "session-lost":
                    require(process.wait(timeout=6) == 0, "Lost real login context did not end the session")
                    require(not live_session(process.pid), "Lost session retained owned services")
                    require(len(started("agent")) == 1, "Lost login context entered agent retry")
                    require(len(started("shell")) == 1, "Lost login context restarted Shell")
                    print(f"PASS [{Path(shell).name}] {scenario}: returned to login without retry")
                    return
                require(process.poll() is None, "Session client exited before shutdown")
                began = time.monotonic()
                if scenario == "parent-death":
                    process.kill()
                    require(process.wait(timeout=3) == -signal.SIGKILL, "Disposable parent was not killed")
                    wait_for("parent-death signal cleaned up owned processes", lambda: not live_session(process.pid), timeout=6)
                else:
                    process.send_signal(signal.SIGTERM)
                    require(process.wait(timeout=6) == 0, "Session client did not complete orderly shutdown")
                elapsed = time.monotonic() - began
                survivors = live_session(process.pid)
                expected = set(started("locker")) if scenario == "graceful" else set()
                require(survivors == expected, f"Unexpected surviving processes: {survivors ^ expected}")
                if scenario in ("graceful", "parent-death"):
                    stopped_roles = {event["role"] for event in events() if event["kind"] == "stopped"}
                    require({"shell", "agent"} <= stopped_roles, "TERM did not reach the direct services")
                if scenario == "graceful":
                    require(expected, "Independent descendant was not exercised")
                if scenario == "restart":
                    require(len(started("shell")) == 1, "Agent restart also restarted Shell")
                print(f"PASS [{Path(shell).name}] {scenario}: shutdown {elapsed:.2f}s; expected survivors={len(expected)}")
            finally:
                # This new session contains only our synthetic fixtures. Cleanup
                # never targets the user's compositor or a production locker.
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=3)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--shell", default="/bin/sh")
    args = parser.parse_args()
    shell = shutil.which(args.shell)
    require(shell is not None, f"Shell is unavailable: {args.shell}")
    setpriv = shutil.which("setpriv")
    require(setpriv is not None, "setpriv is required for the parent-death regression")
    source = (Path(__file__).resolve().parents[1] / "scripts/wayland/session-client").read_text()
    for scenario in ("wrong-parent", "graceful", "ignore", "backoff", "restart", "audio-failed",
                     "main-wait-race", "worker-wait-race", "session-lost", "parent-death"):
        check_case(shell, setpriv, source, scenario)


if __name__ == "__main__":
    main()
