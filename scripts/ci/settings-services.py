#!/usr/bin/env python3
"""Run explicit Settings protocol acceptance in the disposable Alpine container."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time


CASES = (
    ('networkmanager', 'network::activation::integration::isolated_network_activation_lifecycle'),
    ('bluez', 'pairing::integration::isolated_bluez_pairing_lifecycle'),
)


def main():
    if (len(sys.argv) != 3 or os.geteuid() == 0 or
            os.environ.get('ALCARD_CI_CONTAINER') != '1' or
            os.environ.get('ALCARD_CI_SCOPE') != 'acceptance' or
            not Path('/.dockerenv').is_file() or os.uname().machine != 'aarch64'):
        raise SystemExit('Requires the ordinary build user in the native Alpine acceptance container')
    out, target = Path(sys.argv[1]), sys.argv[2]
    if target != 'aarch64-unknown-linux-musl':
        raise SystemExit('Expected the native Alpine aarch64 musl target')
    out.mkdir(exist_ok=False)
    report = {
        'schema': 1,
        'scope': 'Production Settings clients and explicitly simulated service peers on private D-Bus',
        'real_participants': ['alcard-settings-core client/agent', 'dbus-daemon'],
        'simulated_participants': ['NetworkManager', 'BlueZ', 'Wi-Fi/Bluetooth devices'],
        'not_performed': ['system service policy', 'Wi-Fi authentication/RF',
                          'physical Bluetooth pairing', 'installed TUI'],
        'cases': [],
        'exit_code': 1,
    }
    started = time.monotonic()
    try:
        command = ['/usr/bin/cargo', 'test', '--offline', '--locked', '--lib',
                   '--manifest-path', 'rust/Cargo.toml', '-p', 'alcard-settings-core',
                   '--features', 'pairing', '--target', target, '--no-run',
                   '--message-format=json']
        with (out / 'build-artifacts.jsonl').open('w') as stdout, \
                (out / 'build.log').open('w') as stderr:
            subprocess.run(command, stdout=stdout, stderr=stderr, check=True, timeout=180)
        artifacts = [json.loads(line) for line in
                     (out / 'build-artifacts.jsonl').read_text().splitlines()]
        candidates = {item['executable'] for item in artifacts
                      if item.get('executable') and item.get('profile', {}).get('test')
                      and item.get('target', {}).get('name') == 'alcard_settings_core'}
        if len(candidates) != 1:
            raise RuntimeError(f'Expected one exact Settings library test executable, got {len(candidates)}')
        binary = Path(candidates.pop())
        report['binary'] = {'path': str(binary),
                            'sha256': hashlib.sha256(binary.read_bytes()).hexdigest()}
        for name, test in CASES:
            case_started = time.monotonic()
            case = {'name': name, 'test': test, 'exit_code': 1}
            report['cases'].append(case)
            try:
                listed = subprocess.run([str(binary), '--ignored', '--exact', test, '--list'],
                                        capture_output=True, text=True, check=True, timeout=10)
                names = [line.removesuffix(': test') for line in listed.stdout.splitlines()
                         if line.endswith(': test')]
                if names != [test]:
                    raise RuntimeError(f'Expected exactly the requested ignored test, got {names!r}')
                with (out / f'{name}.log').open('w') as log:
                    result = subprocess.run([str(binary), '--ignored', '--exact', test,
                                             '--nocapture', '--test-threads=1'],
                                            stdout=log, stderr=subprocess.STDOUT, timeout=90)
                output = (out / f'{name}.log').read_text()
                passed = re.search(r'test result: ok\. 1 passed; 0 failed; 0 ignored;', output)
                if result.returncode != 0 or not passed:
                    raise RuntimeError(f'Integration test did not pass exactly once (exit {result.returncode})')
                case['exit_code'] = 0
            except (OSError, RuntimeError, subprocess.SubprocessError) as error:
                case['error'] = str(error)
            finally:
                case['seconds'] = round(time.monotonic() - case_started, 2)
                print(f"{'PASS' if case['exit_code'] == 0 else 'FAIL'}: private-bus {name}", flush=True)
        report['exit_code'] = int(any(case['exit_code'] != 0 for case in report['cases']))
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        report['error'] = str(error)
    finally:
        report['seconds'] = round(time.monotonic() - started, 2)
        (out / 'result.json').write_text(json.dumps(report, indent=2) + '\n')
    if report['exit_code']:
        print(json.dumps(report, indent=2), file=sys.stderr)
        for log in sorted(out.glob('*.log')):
            print(f'--- {log.name} ---\n{log.read_text()}', file=sys.stderr)
    return report['exit_code']


if __name__ == '__main__':
    sys.exit(main())
