#!/usr/bin/env python3
"""Exercise dispatcher exec/FD inheritance using harmless temporary fixtures.

No real greetd, getty, PAM, console or system path is modified. Only fixed paths,
the unrelated setfont invocation and (for an unprivileged host) expected owner
IDs are changed in a temporary copy. Alpine root CI uses real root ownership.
"""
from __future__ import annotations

import argparse
import errno
import json
import os
from pathlib import Path
import pty
import shlex
import signal
import subprocess
import sys
import tempfile


DAEMON = r'''
import json, os, pathlib, signal, sys
root = pathlib.Path(ROOT)
mask = os.umask(0)
os.umask(mask)
record = {'pid': os.getpid(), 'umask': mask, 'stderr': os.readlink('/proc/self/fd/2')}
(root / 'daemon.json.tmp').write_text(json.dumps(record))
os.write(2, b'daemon-private\n')
(root / 'daemon.json.tmp').replace(root / 'daemon.json')
if MODE == 'signal':
    while True: signal.pause()
child = os.fork()
if child == 0:
    inherited = os.readlink('/proc/self/fd/2')
    os.write(2, b'early-worker-private\n')
    terminal = os.open(os.ttyname(0), os.O_RDWR)
    for descriptor in (0, 1, 2): os.dup2(terminal, descriptor)
    os.close(terminal)
    (root / 'worker.json').write_text(json.dumps({'inherited': inherited, 'umask': mask}))
    os.write(2, b'greeter-on-tty\n')
    os._exit(0)
assert os.waitpid(child, 0)[1] == 0
raise SystemExit(37)
'''


def check_case(shell: str, source: str, scenario: str) -> None:
    with tempfile.TemporaryDirectory(prefix='alcard-dispatch-') as directory:
        root = Path(directory)
        parent = root / 'run'
        parent.mkdir(mode=0o755)
        parent.chmod(0o755)
        log_dir = parent / 'alcard-greetd'
        log = log_dir / 'daemon.log'
        victim = root / 'victim'
        victim.write_bytes(b'untouched-private-data\n')
        victim.chmod(0o600)
        config = root / 'alcard.toml'
        config.write_text('# harmless fixture\n')
        daemon = root / 'greetd'
        daemon.write_text(f'#!{sys.executable}\nROOT={str(root)!r}\nMODE={scenario!r}\n' + DAEMON)
        daemon.chmod(0o755)
        replacements = {
            'greetd_log_parent=/run': f'greetd_log_parent={shlex.quote(str(parent))}',
            'greetd_log_owner=0:0': f'greetd_log_owner={os.getuid()}:{os.getgid()}',
            'command -v setfont >/dev/null 2>&1': 'false',
            '/usr/sbin/alcard-setupd': '/bin/false',
            '/etc/greetd/alcard.toml': str(config),
            '/usr/sbin/greetd /usr/bin/greetd': shlex.quote(str(daemon)),
        }
        temporary = source
        for old, new in replacements.items():
            assert old in temporary, f'Fixture transform no longer matches: {old}'
            temporary = temporary.replace(old, new)
        dispatcher = root / 'tty1-dispatch'
        dispatcher.write_text(temporary)

        if scenario not in {'fresh', 'signal'}:
            log_dir.mkdir(mode=0o700)
            log_dir.chmod(0o700)
        if scenario == 'parent-symlink':
            actual = root / 'actual-run'
            parent.rename(actual)
            parent.symlink_to(actual, target_is_directory=True)
        elif scenario == 'parent-mode':
            parent.chmod(0o777)
        elif scenario == 'directory-symlink':
            log_dir.rmdir()
            log_dir.symlink_to(root, target_is_directory=True)
        elif scenario == 'directory-mode':
            log_dir.chmod(0o755)
        elif scenario == 'file-symlink':
            log.symlink_to(victim)
        elif scenario == 'file-hardlink':
            os.link(victim, log)
        elif scenario == 'file-fifo':
            os.mkfifo(log, 0o600)
        elif scenario == 'file-directory':
            log.mkdir(mode=0o700)
        elif scenario in {'file-mode', 'file-owner', 'existing'}:
            log.write_bytes(b'previous-daemon\n')
            log.chmod(0o644 if scenario == 'file-mode' else 0o600)
            if scenario == 'file-owner':
                os.chown(log, 65534, 65534)
        elif scenario == 'directory-owner':
            os.chown(log_dir, 65534, 65534)

        master, slave = pty.openpty()
        process = subprocess.Popen([shell, str(dispatcher), 'tty1'], stdin=slave,
                                   stdout=slave, stderr=subprocess.PIPE, umask=0o027)
        os.close(slave)
        try:
            if scenario == 'signal':
                import time
                deadline = time.monotonic() + 3
                while not (root / 'daemon.json').exists() and time.monotonic() < deadline:
                    time.sleep(.01)
                assert (root / 'daemon.json').exists(), 'Fixture did not exec'
                process.send_signal(signal.SIGTERM)
            _, errors = process.communicate(timeout=5)
            terminal = bytearray()
            while True:
                try:
                    chunk = os.read(master, 4096)
                    if not chunk:
                        break
                    terminal.extend(chunk)
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
                    break
            if scenario in {'fresh', 'existing', 'signal'}:
                assert not errors, errors
                record = json.loads((root / 'daemon.json').read_text())
                assert record['pid'] == process.pid, 'Dispatcher introduced a wrapper PID'
                assert record['umask'] == 0o027, 'Private log umask leaked to greetd'
                assert record['stderr'] == str(log)
                assert log_dir.stat().st_mode & 0o7777 == 0o700
                metadata = log.stat()
                assert metadata.st_mode & 0o7777 == 0o600 and metadata.st_nlink == 1
                assert (metadata.st_uid, metadata.st_gid) == (os.getuid(), os.getgid())
                if scenario == 'signal':
                    assert process.returncode == -signal.SIGTERM
                    assert log.read_bytes() == b'daemon-private\n'
                else:
                    assert process.returncode == 37
                    worker = json.loads((root / 'worker.json').read_text())
                    assert worker == {'inherited': str(log), 'umask': 0o027}
                    assert log.read_bytes() == b'daemon-private\nearly-worker-private\n'
                    assert terminal == b'greeter-on-tty\r\n', terminal
            else:
                assert process.returncode == 1, (scenario, process.returncode)
                assert not (root / 'daemon.json').exists(), 'Unsafe path started the daemon'
                assert b'unsafe or unavailable' in errors
                if scenario == 'file-mode':
                    assert log.read_bytes() == b'previous-daemon\n', 'Unsafe log was truncated'
            assert victim.read_bytes() == b'untouched-private-data\n', 'Unsafe target was modified'
            print(f'PASS [{Path(shell).name}] {scenario}')
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--shell', default='/bin/sh')
    arguments = parser.parse_args()
    source = (Path(__file__).resolve().parents[1] / 'scripts/hello/tty1-dispatch').read_text()
    scenarios = ['fresh', 'existing', 'signal', 'parent-symlink', 'parent-mode',
                 'directory-symlink', 'directory-mode', 'file-symlink',
                 'file-hardlink', 'file-fifo', 'file-directory', 'file-mode']
    if os.getuid() == 0:
        scenarios += ['file-owner', 'directory-owner']
    for scenario in scenarios:
        check_case(arguments.shell, source, scenario)


if __name__ == '__main__':
    main()
