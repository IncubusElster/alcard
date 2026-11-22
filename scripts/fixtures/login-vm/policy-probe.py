#!/usr/bin/python3
"""Ordinary menu-launched client of the distro policy. No authentication hooks."""
import json
import os
from pathlib import Path
import subprocess
import time


def main():
    if os.getuid() == 0 or not Path('/etc/alcard-ci-vm').is_file():
        raise SystemExit(1)
    state = Path.home() / '.local/state/alcard-ci'
    state.mkdir(parents=True, exist_ok=True, mode=0o700)
    path = state / 'probe.json'
    record = {'pid': os.getpid(), 'uid': os.getuid(), 'result': None}
    child = subprocess.Popen(['/usr/bin/pkexec', '--disable-internal-agent', '/bin/true'],
                             stdin=subprocess.DEVNULL,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    record['request_pid'] = child.pid
    path.write_text(json.dumps(record))
    record['result'] = child.wait(timeout=120)
    path.write_text(json.dumps(record))
    # Keep the ordinary launching ancestry available for a final session query.
    time.sleep(2)


if __name__ == '__main__':
    main()
