#!/usr/bin/env python3
"""Bounded installed-Shell output-power acceptance on a private headless labwc.

No hardware, PAM, input injection or production override is involved. A separate
ordinary Wayland client holds a real exclusive lease; protocol receipts must
prove Failed, explicit reacquisition and actual wl_output removal/readdition.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import stat
import subprocess
import tempfile
import threading
import time


TRACE = re.compile(
    r'^\[[0-9 .]+\](?:\[rs\])?\s+(?P<direction><-|->)\s+'
    r'(?P<interface>wl_registry|wl_output|zwlr_output_power_manager_v1|zwlr_output_power_v1)'
    r'[@#](?P<object>\d+)\.(?P<method>[a-z_]+),?\s*\((?P<arguments>[^\r\n]*)\)\s*$')


def protocol(line):
    match = TRACE.fullmatch(line.strip())
    if not match:
        return None
    row = match.groupdict()
    row['object'] = int(row['object'])
    interface, method, arguments = row['interface'], row['method'], row['arguments']
    if interface == 'wl_registry':
        if method in {'global', 'bind'}:
            # wayland-backend's Rust logger formats Option<CString> as
            # Some("wl_output"); the C logger uses a plain quoted string.
            match = re.match(r'''^(\d+),\s*(?:Some\(["']wl_output["']\)|["']wl_output["']),\s*(\d+)''', arguments)
            if not match:
                return None
            row['global'] = int(match[1])
            if method == 'bind':
                match = re.search(r'wl_output[@#](\d+)', arguments)
                if not match:
                    return None
                row['output'] = int(match[1])
        elif method == 'global_remove' and arguments.isdecimal():
            row['global'] = int(arguments)
        else:
            return None
    elif interface == 'zwlr_output_power_manager_v1' and method == 'get_output_power':
        match = re.fullmatch(r'zwlr_output_power_v1[@#](\d+),\s*wl_output[@#](\d+)', arguments)
        if not match:
            return None
        row.update(control=int(match[1]), output=int(match[2]))
    elif interface == 'zwlr_output_power_v1' and method in {'mode', 'set_mode'}:
        if arguments not in {'0', '1'}:
            return None
        row['mode'] = int(arguments)
    return row


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


class Runtime:
    def __init__(self, args):
        self.args = args
        self.deadline = time.monotonic() + 55
        self.private = tempfile.TemporaryDirectory(prefix='alcard-output-power-')
        self.root = Path(self.private.name)
        self.children = []
        self.threads = []
        self.streams = []
        self.events = []
        self.guard = threading.Lock()
        self.journal = (args.out / 'receipts.jsonl').open('w', buffering=1)
        self.trace = (args.out / 'shell-protocol.log').open('w', buffering=1)
        self.env = dict(os.environ)
        for name in ('WAYLAND_DISPLAY', 'WAYLAND_SOCKET', 'WAYLAND_DEBUG', 'DISPLAY'):
            self.env.pop(name, None)
        for name, directory in {
            'XDG_RUNTIME_DIR': 'runtime', 'XDG_CONFIG_HOME': 'config',
            'XDG_DATA_HOME': 'data', 'XDG_STATE_HOME': 'state',
            'XDG_CONFIG_DIRS': 'no-config', 'XDG_DATA_DIRS': 'no-data', 'HOME': 'home',
        }.items():
            path = self.root / directory
            path.mkdir(mode=0o700)
            self.env[name] = str(path)
        self.env.update(WLR_BACKENDS='headless', WLR_RENDERER='pixman',
                        WLR_HEADLESS_OUTPUTS='2', WLR_LIBINPUT_NO_DEVICES='1',
                        WLR_XWAYLAND='', XKB_DEFAULT_LAYOUT='us',
                        DBUS_SYSTEM_BUS_ADDRESS='unix:path=' + str(self.root / 'no-system-bus'))
        self.shell = self.helper = self.compositor = None
        self.primary = None

    def event(self, origin, **fields):
        with self.guard:
            row = dict(sequence=len(self.events) + 1, monotonic_ns=time.monotonic_ns(),
                       origin=origin, **fields)
            self.events.append(row)
            self.journal.write(json.dumps(row) + '\n')
            return row

    def rows(self, origin=None, after=0, **fields):
        with self.guard:
            return [row for row in self.events if row['sequence'] > after
                    and (origin is None or row['origin'] == origin)
                    and all(row.get(key) == value for key, value in fields.items())]

    def mark(self, event):
        return self.event('driver', event=event)['sequence']

    def healthy(self):
        if time.monotonic() >= self.deadline:
            raise TimeoutError('Private output-power scenario exceeded its 55-second work limit')
        for name, process in self.children:
            if process.poll() is not None:
                raise AssertionError(f'{name} exited unexpectedly: {process.returncode}')

    def wait(self, description, predicate, seconds=8):
        deadline = min(self.deadline, time.monotonic() + seconds)
        while time.monotonic() < deadline:
            self.healthy()
            result = predicate()
            if result:
                return result
            time.sleep(.03)
        raise AssertionError('Missing real compositor receipt: ' + description)

    def settle(self, seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            self.healthy()
            time.sleep(.03)

    def command(self, *arguments):
        self.healthy()
        result = subprocess.run(arguments, env=self.env, capture_output=True, text=True,
                                timeout=max(.1, min(8, self.deadline - time.monotonic())))
        self.event('driver', event='command', arguments=list(arguments),
                   exit_code=result.returncode, stdout=result.stdout[-4000:], stderr=result.stderr[-4000:])
        if result.returncode:
            raise AssertionError(f'{arguments[0]} returned {result.returncode}')
        return result.stdout

    def spawn(self, name, arguments, *, env=None, receipts=False, shell=False):
        log = (self.args.out / (name + '.log')).open('w')
        self.streams.append(log)
        process = subprocess.Popen(arguments, env=env or self.env, start_new_session=True,
                                   stdin=subprocess.PIPE if receipts else subprocess.DEVNULL,
                                   stdout=subprocess.PIPE if receipts else subprocess.DEVNULL,
                                   stderr=subprocess.PIPE if shell else log, text=True, bufsize=1)
        self.children.append((name, process))
        self.event('driver', event='started', name=name, pid=process.pid, arguments=list(arguments))
        if receipts or shell:
            def collect():
                stream = process.stdout if receipts else process.stderr
                try:
                    for line in stream:
                        if receipts:
                            self.event('helper', **json.loads(line))
                        else:
                            row = protocol(line)
                            if row:
                                with self.guard:
                                    self.trace.write(line)
                                self.event('shell', **row)
                            elif not line.startswith('['):
                                log.write(line)
                                log.flush()
                except (OSError, ValueError) as error:
                    self.event('driver', event='reader_error', name=name, error=str(error))
            thread = threading.Thread(target=collect, daemon=True)
            thread.start()
            self.threads.append(thread)
        return process

    def helper_command(self, command):
        before = self.mark('helper_' + command)
        self.helper.stdin.write(command + '\n')
        self.helper.stdin.flush()
        return self.wait('helper ' + command, lambda: next(iter(self.rows(
            'helper', before, event='synced', command=command)), None))

    def capture(self, name):
        if self.primary and self.compositor is not None and self.compositor.poll() is None:
            try:
                # Preserve the live compositor before cleanup even when Shell
                # exited or the scenario's work deadline was the failure.
                result = subprocess.run(
                    ['grim', '-o', self.primary, str(self.args.out / (name + '.png'))],
                    env=self.env, capture_output=True, text=True, timeout=2)
                if result.returncode:
                    raise AssertionError(f'grim returned {result.returncode}: {result.stderr[-500:]}')
                self.event('driver', event='capture', name=name)
            except (AssertionError, OSError, subprocess.TimeoutExpired) as error:
                self.event('driver', event='capture_unavailable', error=str(error))

    def start(self):
        config = Path(self.env['XDG_CONFIG_HOME'])
        (config / 'labwc').mkdir()
        (config / 'labwc/rc.xml').write_text(
            '<?xml version="1.0"?><labwc_config><focus><followMouse>no</followMouse></focus>'
            '<keyboard><default/></keyboard></labwc_config>\n')
        for name in ('autostart', 'shutdown'):
            path = config / 'labwc' / name
            path.write_text('#!/bin/sh\nexit 0\n')
            path.chmod(0o700)
        (config / 'alcard').mkdir()
        (config / 'alcard/shell.toml').write_text(
            "locale='en'\n[idle]\nlock_after_seconds=3600\nscreen_off_after_seconds=3601\n")
        self.compositor = self.spawn('labwc', ['labwc', '-C', str(config / 'labwc')])
        runtime = Path(self.env['XDG_RUNTIME_DIR'])
        socket = self.wait('new private Wayland socket', lambda: next(
            (path for path in runtime.glob('wayland-*') if stat.S_ISSOCK(path.lstat().st_mode)), None))
        self.env['WAYLAND_DISPLAY'] = socket.name
        outputs = [line.split()[0] for line in self.command('wlr-randr').splitlines()
                   if line and not line[0].isspace()]
        assert len(outputs) == 2 and all(re.fullmatch(r'HEADLESS-[A-Za-z0-9-]+', name) for name in outputs)
        self.primary, target = outputs
        for index, name in enumerate(outputs):
            self.command('wlr-randr', '--output', name, '--custom-mode', '320x170@60Hz',
                         '--scale', '1', '--pos', f'{index * 320},0')
        self.helper = self.spawn('helper', [str(self.args.helper), target], receipts=True,
                                 env=self.env | {'ALCARD_OUTPUT_POWER_FIXTURE': '1'})
        ready = self.wait('accepted exclusive power lease', lambda: next(iter(
            self.rows('helper', event='ready')), None))
        output = next(item for item in ready['outputs'] if item['name'] == target)
        self.shell = self.spawn('shell', [str(self.args.shell)], shell=True,
                                env=self.env | {'WAYLAND_DEBUG': '1'})
        return target, output['global']

    def binding(self, global_name, after=0):
        return next(iter(self.rows('shell', after, interface='wl_registry', method='bind',
                                   direction='->', **{'global': global_name})), None)

    def control(self, output, after=0):
        return next(iter(self.rows('shell', after, interface='zwlr_output_power_manager_v1',
                                   method='get_output_power', direction='->', output=output)), None)

    def power(self, control, method, after=0, **fields):
        return next(iter(self.rows('shell', after, interface='zwlr_output_power_v1',
                                   object=control, method=method, **fields)), None)

    def close(self):
        for _, process in reversed(self.children):
            if process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
        for _, process in reversed(self.children):
            try:
                process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=1)
        for thread in self.threads:
            thread.join(timeout=1)
        for stream in self.streams:
            stream.close()
        self.trace.close()
        self.journal.close()
        self.private.cleanup()


def exercise(runtime, report):
    def passed(name, **evidence):
        report['cases'].append(dict(name=name, status='PASS', **evidence))
        print('PASS: output-power ' + name, flush=True)

    target, old_global = runtime.start()
    binding = runtime.wait('Shell bind of held wl_output', lambda: runtime.binding(old_global))
    old_output = binding['output']
    first = runtime.wait('Shell requested held control', lambda: runtime.control(old_output))
    failed = runtime.wait('Shell received Failed', lambda: runtime.power(first['control'], 'failed', first['sequence']))
    destroyed = runtime.wait('Shell destroyed Failed control', lambda: runtime.power(
        first['control'], 'destroy', failed['sequence'], direction='->'))
    count = len(runtime.rows('shell', interface='zwlr_output_power_manager_v1', method='get_output_power'))
    runtime.settle(.65)
    assert len(runtime.rows('shell', interface='zwlr_output_power_manager_v1', method='get_output_power')) == count, (
        'Shell retried rejected control without explicit reload or hotplug')
    assert not runtime.power(first['control'], 'set_mode', destroyed['sequence'])
    passed('failed', global_name=old_global, failed_sequence=failed['sequence'],
           destroy_sequence=destroyed['sequence'], no_retry_seconds=.65)

    runtime.helper_command('release')
    marker = runtime.mark('explicit_valid_reload')
    runtime.shell.send_signal(signal.SIGHUP)
    reacquired = runtime.wait('new Shell control after reload', lambda: runtime.control(old_output, marker))
    mode = runtime.wait('reacquired control Mode(On)', lambda: runtime.power(
        reacquired['control'], 'mode', reacquired['sequence'], mode=1, direction='<-'))
    assert not runtime.power(reacquired['control'], 'failed', reacquired['sequence'])
    passed('reacquire', creation_sequence=reacquired['sequence'], mode_sequence=mode['sequence'])

    marker = runtime.mark('disable_secondary_output')
    runtime.command('wlr-randr', '--output', target, '--off')
    snapshot = runtime.helper_command('sync')
    assert not any(item['global'] == old_global for item in snapshot['outputs']), (
        'Disabled head retained wl_output; this is not the required hotplug scenario')
    helper_removed = next(iter(runtime.rows('helper', marker, event='global_remove',
                                           **{'global': old_global})), None)
    assert helper_removed is not None, 'Observer did not receive real global_remove'
    removed = runtime.wait('Shell real wl_output global_remove', lambda: next(iter(runtime.rows(
        'shell', marker, interface='wl_registry', method='global_remove', **{'global': old_global})), None))
    retired = runtime.wait('Shell released removed output control', lambda: runtime.power(
        reacquired['control'], 'destroy', marker, direction='->'))
    runtime.settle(.4)
    assert runtime.control(old_output, removed['sequence']) is None
    assert runtime.power(reacquired['control'], 'set_mode', retired['sequence']) is None
    disabled_end = runtime.mark('retired_control_quiet')

    runtime.command('wlr-randr', '--output', target, '--on', '--custom-mode', '320x170@60Hz',
                    '--scale', '1', '--pos', '320,0')
    snapshot = runtime.helper_command('sync')
    added = next(item for item in snapshot['outputs'] if item['name'] == target)
    assert added['global'] != old_global, 'Expected a new wl_output global lifetime'
    helper_added = next(iter(runtime.rows('helper', disabled_end, event='global_add',
                                         **{'global': added['global']})), None)
    assert helper_added is not None, 'Observer did not receive the new output global'
    global_added = runtime.wait('Shell new wl_output global event', lambda: next(iter(runtime.rows(
        'shell', disabled_end, interface='wl_registry', method='global',
        **{'global': added['global']})), None))
    new_binding = runtime.wait('Shell bind of new output', lambda: runtime.binding(added['global'], disabled_end))
    replacement = runtime.wait('Shell new output-power control', lambda: runtime.control(
        new_binding['output'], new_binding['sequence']))
    mode = runtime.wait('new output-power Mode(On)', lambda: runtime.power(
        replacement['control'], 'mode', replacement['sequence'], mode=1, direction='<-'))
    assert not runtime.power(replacement['control'], 'failed', replacement['sequence'])
    passed('hotplug', old_global=old_global, new_global=added['global'],
           removal_sequence=removed['sequence'], helper_removal_sequence=helper_removed['sequence'],
           global_add_sequence=global_added['sequence'], helper_add_sequence=helper_added['sequence'],
           new_binding_sequence=new_binding['sequence'],
           creation_sequence=replacement['sequence'], mode_sequence=mode['sequence'])
    # Protocol IDs can be recycled. The retired interval ends before creation
    # of the replacement lifetime; numeric ID reuse is not a stale request.
    retired_requests = [row for row in runtime.rows('shell', retired['sequence'],
                        interface='zwlr_output_power_v1', object=reacquired['control'], direction='->')
                        if row['sequence'] < replacement['sequence']]
    assert not retired_requests, 'A request targeted the retired power control lifetime'
    passed('retired-control', destroy_sequence=retired['sequence'],
           replacement_sequence=replacement['sequence'], stale_requests=0)
    runtime.capture('01-reacquired-after-hotplug')
    runtime.healthy()
    assert not runtime.rows('driver', event='reader_error')


def self_check():
    examples = [
        ('[1.200][rs] -> wl_registry@2.bind(42, "wl_output", 4, wl_output@18)', 'bind', 42),
        ('[1.200][rs] -> wl_registry@2.bind(42, Some("wl_output"), 4, wl_output@18)', 'bind', 42),
        ('[1.201][rs] <- wl_registry@2.global_remove, (42)', 'global_remove', 42),
        ('[1.202][rs] <- wl_registry@2.global, (57, "wl_output", 4)', 'global', 57),
        ('[1.202][rs] <- wl_registry@2.global, (57, Some("wl_output"), 4)', 'global', 57),
    ]
    for line, method, global_name in examples:
        row = protocol(line)
        assert row['method'] == method and row['global'] == global_name
    row = protocol('[1.203][rs] -> zwlr_output_power_manager_v1@16.get_output_power(zwlr_output_power_v1@26, wl_output@18)')
    assert (row['control'], row['output']) == (26, 18)
    assert protocol('[1.204][rs] <- zwlr_output_power_v1@26.mode, (1)')['mode'] == 1
    assert protocol('[1.205][rs] <- zwlr_output_power_v1@26.failed, ()')['method'] == 'failed'
    assert protocol('[1.206][rs] <- wl_keyboard@20.key, (1, 2, 3, 4)') is None
    assert protocol('arbitrary diagnostic') is None
    print('PASS: output-power public protocol parser, no compositor or processes started')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--helper', type=Path)
    parser.add_argument('--shell', type=Path)
    parser.add_argument('--out', type=Path)
    parser.add_argument('--self-check', action='store_true')
    args = parser.parse_args()
    if args.self_check:
        self_check()
        return
    assert os.getuid() != 0 and os.getuid() == os.geteuid(), 'Requires an ordinary, unchanged UID'
    assert os.environ.get('ALCARD_OUTPUT_POWER_TEST') == '1' and Path('/.dockerenv').is_file()
    assert os.environ.get('DBUS_SESSION_BUS_ADDRESS'), 'Caller must create a private dbus-run-session'
    assert all(path and path.is_absolute() for path in (args.helper, args.shell, args.out))
    assert all(path.is_file() and os.access(path, os.X_OK) for path in (args.helper, args.shell))
    assert not args.out.is_symlink()
    args.out.mkdir(parents=True, exist_ok=True)
    assert not any(args.out.iterdir()), 'Refusing stale acceptance evidence'
    report = dict(schema=1, status='RUNNING', scope='Installed Shell with real headless labwc output-power and wl_output lifecycle',
                  architecture=os.uname().machine, cases=[],
                  shell=dict(path=str(args.shell), sha256=digest(args.shell)),
                  helper=dict(path=str(args.helper), sha256=digest(args.helper)),
                  helper_source_sha256=digest(Path(__file__).resolve().parents[1] /
                      'rust/apps/alcard-shell/examples/output-power-fixture.rs'),
                  script_sha256=digest(__file__), hardware_tested=False)
    destination = args.out / 'result.json'
    destination.write_text(json.dumps(report, indent=2) + '\n')
    runtime = Runtime(args)
    began = time.monotonic()
    try:
        exercise(runtime, report)
        report['status'] = 'PASS'
    except BaseException as error:
        report.update(status='FAIL', error=f'{type(error).__name__}: {error}')
        runtime.capture('99-failure')
        raise
    finally:
        try:
            runtime.close()
        except BaseException as error:
            report.update(status='FAIL', cleanup_error=f'{type(error).__name__}: {error}')
            raise
        finally:
            report['duration_seconds'] = round(time.monotonic() - began, 3)
            destination.write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
