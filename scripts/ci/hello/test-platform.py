#!/usr/bin/env python3
"""Exercise probe protocol failures using a tiny pipe peer, without QEMU."""
import importlib.util
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location('hello_qemu_platform',
                                            Path(__file__).with_name('qemu_platform.py'))
PLATFORM = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PLATFORM)

PEER = r'''
import json, sys
mode = sys.argv[1]
if mode == 'missing-rom':
    print('failed to find romfile "efi-virtio.rom"', file=sys.stderr, flush=True)
    sys.exit(1)
if mode == 'stall':
    sys.stdin.read()
    sys.exit(1)
if mode == 'flood':
    print('startup diagnostic ' * 2048, flush=True)
    sys.exit(1)
def send(value):
    print(json.dumps(value), flush=True)
send({'QMP': {'version': {'qemu': {'major': 8, 'minor': 2, 'micro': 0}}, 'capabilities': []}})
for line in sys.stdin:
    command = json.loads(line)
    name, identifier = command['execute'], command['id']
    if mode == 'early-eof' and name == 'qmp_capabilities':
        sys.exit(0)
    if mode == 'wrong-id':
        identifier += 10
    value = {}
    if name == 'query-status':
        value = {'running': mode == 'running',
                 'status': 'prelaunch' if mode == 'quit-eof' else mode}
    if name == 'quit':
        send({'event': 'SHUTDOWN', 'data': {'guest': False}})
        if mode == 'quit-eof':
            sys.exit(0)
    send({'return': value, 'id': identifier})
    if name == 'quit':
        break
'''


class PlatformProbeTests(unittest.TestCase):
    def probe(self, mode, **options):
        processes = []
        original = subprocess.Popen

        def start(*args, **kwargs):
            process = original(*args, **kwargs)
            processes.append(process)
            return process

        try:
            with patch.object(PLATFORM.subprocess, 'Popen', side_effect=start):
                return PLATFORM._probe([sys.executable, '-c', PEER, mode], **options)
        finally:
            self.assertEqual(len(processes), 1)
            self.assertIsNotNone(processes[0].poll(), 'Probe left its child running')
            self.assertTrue(processes[0].stdin.closed)
            self.assertTrue(processes[0].stdout.closed)

    def test_stopped_platform_with_quit_reply_or_documented_eof(self):
        for mode in ('prelaunch', 'paused', 'quit-eof'):
            with self.subTest(mode=mode):
                result = self.probe(mode)
                self.assertEqual(result['state'], 'paused' if mode == 'paused' else 'prelaunch')
                self.assertIs(result['running'], False)

    def test_startup_failure_preserves_qemu_diagnostic(self):
        with self.assertRaisesRegex(RuntimeError, 'failed to find romfile "efi-virtio.rom"'):
            self.probe('missing-rom')

    def test_running_guest_unrelated_reply_or_early_eof_is_not_success(self):
        for mode in ('running', 'wrong-id', 'early-eof'):
            with self.subTest(mode=mode), self.assertRaises(RuntimeError):
                self.probe(mode)

    def test_timeout_and_output_limit_kill_and_reap_peer(self):
        with self.assertRaisesRegex(RuntimeError, 'deadline exceeded'):
            self.probe('stall', timeout=.2)
        with self.assertRaisesRegex(RuntimeError, 'output exceeded') as failure:
            self.probe('flood', output_limit=1024)
        self.assertLess(len(str(failure.exception)), 1500)


if __name__ == '__main__':
    unittest.main()
