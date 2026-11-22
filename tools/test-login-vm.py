#!/usr/bin/env python3
"""Installed Alpine login acceptance using real QEMU VT, PAM, elogind and input.

Neither observer RPC nor QMP traffic is logged. Random credentials remain only
in process memory and the disposable QEMU snapshot; exported files are scanned
before the caller can upload the evidence directory.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess
import tempfile
import time
import tomllib

from PIL import Image


RESOURCE_PHASE = 'first_graphics_login_after_readiness_cancel_before_policy_lock_faults'
RESOURCE_CONTEXT = dict(guest_ram_mib=1536, guest_vcpus=2, acceleration='tcg',
                        hardware_measurement=False,
                        comparison_scope='standalone VM baseline; no A/B comparison with the former 512 MiB guest')


class ShellFrames:
    # Exact RGB footer from the real English/Classic 320x170 Shell in native
    # run6 and three installed VM run5 Home captures. Clock/status are outside
    # this region. Source glyphs also pin the header and the selected menu item.
    HOME_FOOTER = 'efb346134a43405ac8a3c8925093fe59191ff43627bad6209abaf44a760b3aea'
    STATES = {'home', 'probe-menu', 'quick-menu', 'quick-lock', 'quick-logout'}

    def __init__(self, root):
        palette = tomllib.loads((root / 'rust/crates/alcard-settings-core/themes/classic.toml').read_text())['palette']
        self.colors = {name: tuple(bytes.fromhex(palette[name][1:])) for name in ('text', 'accent')}
        self.glyphs = {}
        for line in (root / 'fonts/alcard-pixel/source/AlcardPixel-Regular.hex').read_text().splitlines():
            if ':' in line:
                code, bits = line.split(':', 1)
                if len(bits) == 16:
                    self.glyphs[chr(int(code, 16))] = bytes.fromhex(bits)

    def text_matches(self, image, x, y, text, color):
        foreground = self.colors[color]
        return all((image.getpixel((x + index * 8 + column, y + row)) == foreground)
                   == bool(self.glyphs[character][row] & (0x80 >> column))
                   for index, character in enumerate(text) for row in range(8) for column in range(8))

    def classify(self, image):
        if image.size != (320, 170):
            return 'unknown'
        if (hashlib.sha256(image.crop((0, 154, 320, 170)).tobytes()).hexdigest() == self.HOME_FOOTER
                and self.text_matches(image, 8, 5, 'ALCARD', 'accent')):
            return 'home'
        if (self.text_matches(image, 8, 5, 'Applications', 'accent')
                and self.text_matches(image, 8, 20, '000 CI Policy Probe', 'text')
                and self.text_matches(image, 16, 158, 'Open', 'text')):
            return 'probe-menu'
        if self.text_matches(image, 16, 22, 'Quick settings', 'accent'):
            # Quick keeps four rows visible; a selected index >= 3 is the
            # last row, whose title starts at y=120. Match the real primary
            # Select softkey too before allowing a system operation.
            if self.text_matches(image, 8, 158, 'Select', 'text'):
                if self.text_matches(image, 16, 120, 'Lock screen', 'text'):
                    return 'quick-lock'
                if self.text_matches(image, 16, 120, 'Log out', 'text'):
                    return 'quick-logout'
            return 'quick-menu'
        return 'unknown'


class Lines:
    def __init__(self, path, process, timeout=30):
        self.socket = socket.socket(socket.AF_UNIX)
        deadline = time.monotonic() + timeout
        while True:
            try:
                self.socket.connect(str(path))
                break
            except (FileNotFoundError, ConnectionRefusedError):
                if process.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError('Private VM channel was not created')
                time.sleep(.1)
        self.buffer = b''
        self.identifier = 0

    def receive(self, timeout=10):
        deadline = time.monotonic() + timeout
        while b'\n' not in self.buffer:
            self.socket.settimeout(max(.01, deadline - time.monotonic()))
            chunk = self.socket.recv(65536)
            if not chunk:
                raise RuntimeError('Private VM channel closed')
            self.buffer += chunk
            if len(self.buffer) > 1024 * 1024:
                raise RuntimeError('Oversized VM response')
        line, self.buffer = self.buffer.split(b'\n', 1)
        return json.loads(line)

    def request(self, value, timeout=30):
        self.identifier += 1
        identifier = self.identifier
        # Includes secret input for setup/send-key: intentionally never log it.
        self.socket.sendall(json.dumps({'id': identifier, **value}).encode() + b'\n')
        deadline = time.monotonic() + timeout
        while True:
            reply = self.receive(max(.1, deadline - time.monotonic()))
            if reply.get('id') == identifier:
                if 'error' in reply:
                    raise RuntimeError(str(reply['error']))
                return reply.get('result', reply.get('return'))

    def close(self):
        self.socket.close()


class Machine:
    def __init__(self, directory, mode):
        self.directory = directory
        self.mode = mode
        self.evidence = directory / 'evidence' / mode
        self.evidence.mkdir(parents=True, exist_ok=True)
        self.private = tempfile.TemporaryDirectory(prefix='alcard-vt-')
        self.work = Path(self.private.name)
        self.password = secrets.token_hex(12)
        self.bad_password = secrets.token_hex(12)
        self.credentials_entered = False
        self.events = []
        self.qmp = None
        self.observer = None
        self.process = None
        self.serial = None
        self.stderr = None
        self.frames = ShellFrames(Path(__file__).resolve().parents[1])

    def note(self, name, **details):
        self.events.append({'stage': name, **details})
        print(f'[{self.mode}] {name}', flush=True)

    def start(self):
        private = self.directory / 'private'
        qmp_socket, observer_socket = self.work / 'qmp', self.work / 'observer'
        self.serial = (self.work / 'serial.log').open('wb')
        self.stderr = (self.work / 'qemu.log').open('wb')
        command = [
            'qemu-system-aarch64', '-machine', 'virt', '-accel', 'tcg,thread=multi',
            '-cpu', 'max', '-smp', '2', '-m', '1536', '-snapshot',
            '-kernel', str(private / 'vmlinuz-virt'),
            '-initrd', str(private / 'initramfs-virt'),
            '-append', 'root=/dev/vda rw rootfstype=ext4 modules=virtio_pci,virtio_blk,ext4 '
                       'console=ttyAMA0 console=tty0 alcard_ci_vm=1',
            '-drive', f'file={private / "rootfs.ext4"},format=raw,if=none,id=rootdisk',
            '-device', 'virtio-blk-pci,drive=rootdisk',
            # QEMU 8.2 EDID rounds vertical sync to zero below 200 lines.
            # Let the DRM virtio driver generate valid CVT timings for 320x170.
            '-device', 'virtio-gpu-pci,xres=320,yres=170,max_outputs=1,edid=off',
            '-device', 'qemu-xhci,id=usb', '-device', 'usb-kbd,bus=usb.0',
            '-display', 'none', '-monitor', 'none', '-nic', 'none', '-no-reboot',
            '-qmp', f'unix:{qmp_socket},server=on,wait=off',
            '-serial', 'stdio',
            '-chardev', f'socket,id=control,path={observer_socket},server=on,wait=off',
            '-device', 'virtio-serial-pci',
            '-device', 'virtserialport,chardev=control,name=org.alcard.ci',
        ]
        self.process = subprocess.Popen(command, stdin=subprocess.DEVNULL,
                                        stdout=self.serial, stderr=self.stderr,
                                        start_new_session=True)
        self.qmp = Lines(qmp_socket, self.process)
        assert 'QMP' in self.qmp.receive()
        self.qmp.request({'execute': 'qmp_capabilities'})
        self.observer = Lines(observer_socket, self.process)
        greeting = self.observer.receive(timeout=240)
        assert greeting == {'ready': True, 'protocol': 1}, greeting
        self.note('Booted real aarch64 Linux VT with private observer')
        metadata = self.call('setup-metadata')
        self.note('Boot service metadata before Hello', metadata=metadata)
        assert not metadata['files']['/run/openrc/started/dmesg'].get('missing'), (
            'The shared OpenRC dmesg boot service did not start')
        assert metadata['console_loglevel'] == 1, (
            'The standard OpenRC dmesg console level was not applied')
        setup = self.call('setup', mode=self.mode, password=self.password, timeout=120)
        self.uid = setup['uid']
        self.note('Real Hello arm/apply completed under umask 077', setup=setup)

    def call(self, operation, timeout=30, **arguments):
        return self.observer.request({'op': operation, **arguments}, timeout=timeout)

    def key(self, *keys, deadline=None):
        timeout = 30 if deadline is None else deadline - time.monotonic()
        assert timeout > 0, 'Deadline before VM input'
        self.qmp.request({'execute': 'send-key', 'arguments': {
            'keys': [{'type': 'qcode', 'data': key} for key in keys], 'hold-time': 40}}, timeout=timeout)
        time.sleep(.13 if deadline is None else min(.13, max(0, deadline - time.monotonic())))
        assert deadline is None or time.monotonic() < deadline, 'Deadline after VM input'

    def wait_shell_frame(self, wanted, deadline, description):
        assert wanted and set(wanted) <= ShellFrames.STATES
        started = time.monotonic()
        samples, last, rpc_timeout, late_reply = 0, 'unknown', False, False
        target = self.work / 'private-shell-frame.png'
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError('QEMU exited before ' + description)
            try:
                self.qmp.request({'execute': 'screendump', 'arguments': {
                    'filename': str(target), 'format': 'png'}},
                    timeout=max(.01, deadline - time.monotonic()))
                with Image.open(target) as source:
                    last = self.frames.classify(source.convert('RGB'))
                samples += 1
            except TimeoutError:
                rpc_timeout = True
                break
            finally:
                # Even unexpected greeter/credential frames never enter the
                # evidence directory and are not kept after classification.
                target.unlink(missing_ok=True)
            if time.monotonic() >= deadline:
                late_reply = True
                break
            if last in wanted:
                self.note('Observed Shell frame', description=description, state=last,
                          samples=samples, elapsed_seconds=round(time.monotonic() - started, 3))
                return last
            time.sleep(min(.2, max(0, deadline - time.monotonic())))
        timing = dict(elapsed_seconds=round(time.monotonic() - started, 3), samples=samples,
                      rpc_timeout=rpc_timeout, late_reply=late_reply)
        self.note('VM frame deadline', description=description, expected=sorted(wanted),
                  last_state=last, timing=timing)
        raise AssertionError(f'Timeout: {description}; state={last}; timing={timing}')

    def observed_home(self, deadline):
        current = self.wait_shell_frame({'home', 'probe-menu'}, deadline, 'known Shell before Home')
        if current == 'probe-menu':
            self.key('meta_l', deadline=deadline)
            self.wait_shell_frame({'home'}, deadline, 'Home after the menu')
        # Sending Super when Home is already visible would leave an async Home
        # request whose later dispatch could undo the following Menu key.

    def probe_menu(self, deadline):
        self.observed_home(deadline)
        self.key('6', deadline=deadline)
        self.wait_shell_frame({'probe-menu'}, deadline, 'selected ordinary policy probe in Menu')

    def text(self, value):
        if not all(character in 'abcdefghijklmnopqrstuvwxyz0123456789' for character in value):
            raise ValueError('Only lowercase ASCII login/test input is supported')
        if value in (self.password, self.bad_password):
            # Set before the first key, so even partial input suppresses the
            # automatic failure screenshot. Later captures have explicit
            # known-empty form / authenticated Home preconditions.
            self.credentials_entered = True
        for character in value:
            self.key(character)

    def wait(self, description, predicate, timeout=40, *, operation='state'):
        assert operation in {'state', 'process-state'}
        started = time.monotonic()
        deadline = started + timeout
        last = None
        samples = 0
        rpc_seconds = 0
        rpc_timeout = None
        late_reply = False
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError('QEMU exited before ' + description)
            rpc_started = time.monotonic()
            try:
                last = self.call(operation, timeout=max(.01, deadline - rpc_started))
            except TimeoutError as error:
                rpc_seconds = time.monotonic() - rpc_started
                rpc_timeout = error
                break
            rpc_seconds = time.monotonic() - rpc_started
            samples += 1
            if time.monotonic() >= deadline:
                late_reply = True
                break
            result = predicate(last)
            if result:
                return result
            time.sleep(min(.25, max(0, deadline - time.monotonic())))
        # No console text or credential arguments are included in state.
        timing = dict(operation=operation, budget_seconds=timeout,
                      elapsed_seconds=round(time.monotonic() - started, 3),
                      successful_samples=samples, last_rpc_seconds=round(rpc_seconds, 3),
                      rpc_timeout=rpc_timeout is not None, late_reply=late_reply)
        self.note('VM observation deadline', description=description, timing=timing, public_state=last)
        raise AssertionError(f'Timeout: {description}; timing={timing}; public state={last}') from rpc_timeout

    @staticmethod
    def roles(state, role):
        return [p for p in state['processes'] if p['role'] == role]

    def client(self, role, timeout=40, different=None):
        return self.wait(role + ' ready', lambda state: next((p for p in self.roles(state, role)
                         if p['pid'] != different), None), timeout, operation='process-state')

    def registered_agent(self, timeout=40, different=None, expected_sid=None):
        # A visible VT log line can scroll away or belong to an older PID.
        # Require a fresh real authority challenge, then cancel without typing
        # any credential. The normal wrong/success PAM tests still run later.
        deadline = time.monotonic() + timeout

        def remaining():
            seconds = deadline - time.monotonic()
            if seconds <= 0:
                raise AssertionError('Deadline proving production agent through a fresh authority challenge')
            return seconds

        def available(state):
            agents = self.roles(state, 'agent')
            if (len(agents) == 1 and agents[0]['uid'] == self.uid
                    and agents[0]['pid'] != different
                    and not state['presence']['org.alcard.Authentication']
                    and not any(self.roles(state, role) for role in ('probe', 'pkexec', 'polkit-helper'))):
                return agents[0]
            return None

        for attempt in range(1, 5):
            candidate = self.wait('one current agent and no old authentication request',
                                  available, remaining())
            previous = (self.call('state', timeout=remaining())['probe'] or {}).get('pid')
            remaining()
            self.probe_menu(deadline)
            self.note('Started fresh agent readiness request', attempt=attempt, agent=candidate,
                      previous_probe=previous)
            self.key('4', deadline=deadline)  # Observed ordinary menu entry: pkexec /bin/true.
            observed_challenge = False

            def outcome(state):
                nonlocal observed_challenge
                observed_challenge |= bool(state['presence']['org.alcard.Authentication']
                                           or self.roles(state, 'polkit-helper'))
                probe = state['probe']
                if not probe or probe['pid'] == previous:
                    return None
                assert probe['uid'] == self.uid
                if probe['result'] is not None:
                    return state
                if (state['presence']['org.alcard.Authentication']
                        and state.get('authentication_owner')
                        and self.roles(state, 'polkit-helper')):
                    return state
                return None

            state = self.wait('fresh pkexec challenge or explicit pre-challenge refusal',
                              outcome, remaining())
            probe = state['probe']
            if probe['result'] is not None:
                self.note('Agent readiness request ended before proof', attempt=attempt,
                          probe=probe, observed_challenge=observed_challenge)
                assert probe['result'] != 0, 'Readiness request authorized without a fresh graphical challenge'
                assert not observed_challenge, 'An observed challenge ended unexpectedly; never retry it'
                assert not state['presence']['org.alcard.Authentication']
                assert not self.roles(state, 'polkit-helper')
                if attempt == 4:
                    raise AssertionError('Four fresh readiness requests ended without a graphical agent challenge')
                # Only this harmless completed probe is retried. Never restart
                # the agent, policy daemon, session, or an observed challenge.
                self.wait('finished readiness probe fully exited', lambda current:
                          not any(self.roles(current, role) for role in ('probe', 'pkexec', 'polkit-helper'))
                          and not current['presence']['org.alcard.Authentication'], remaining())
                time.sleep(min(1.5, remaining()))
                continue

            owner = state['authentication_owner']
            self.note('Fresh agent readiness challenge observed', attempt=attempt,
                      probe=probe, agent=candidate, authentication_owner=owner)
            assert self.lifetime(owner['process']) == self.lifetime(candidate), (
                'Authentication surface is not owned by this agent lifetime')
            proof = self.call('proof', roles=['shell', 'agent', 'probe', 'pkexec'], timeout=remaining())
            self.note('Agent readiness UnixSession proof collected', attempt=attempt, proof=proof)
            sid = self.validate_proof(proof, expected_sid)
            members = {role: [item['process'] for item in proof['members'] if item['process']['role'] == role]
                       for role in ('agent', 'probe', 'pkexec')}
            # The observer also labels the launching Foot ancestor as probe
            # because its argv contains the script path. Select the actual
            # script lifetime from its fresh record; validate_proof above still
            # checks that every ancestor belongs to this same UnixSession.
            members['probe'] = [item for item in members['probe'] if item['pid'] == probe['pid']]
            members['pkexec'] = [item for item in members['pkexec'] if item['pid'] == probe['request_pid']]
            assert all(len(items) == 1 for items in members.values()), 'Ambiguous readiness request processes'
            assert self.lifetime(members['agent'][0]) == self.lifetime(candidate)
            assert members['probe'][0]['pid'] == probe['pid']
            assert members['pkexec'][0]['pid'] == probe['request_pid']
            assert members['pkexec'][0]['ppid'] == probe['pid']
            for role in ('probe', 'pkexec'):
                original = next(item for item in self.roles(state, role) if item['pid'] == members[role][0]['pid'])
                assert self.lifetime(members[role][0]) == self.lifetime(original)
            time.sleep(.5)  # Allow the real credential surface to map before Escape.
            self.key('esc', deadline=deadline)

            def cancelled(current):
                record = current['probe']
                if not record or record['pid'] != probe['pid'] or record['result'] is None:
                    return None
                assert record['request_pid'] == probe['request_pid'] and record['result'] != 0
                if (current['presence']['org.alcard.Authentication']
                        or self.roles(current, 'polkit-helper')):
                    return None
                assert any(self.lifetime(item) == self.lifetime(candidate)
                           for item in self.roles(current, 'agent')), 'Agent lifetime ended during readiness cancellation'
                return record

            denied = self.wait('readiness cancellation denied the exact request and closed its UI',
                               cancelled, remaining())
            self.wait('cancelled readiness probe exited', lambda current:
                      not any(self.roles(current, role) for role in ('probe', 'pkexec')), remaining())
            self.observed_home(deadline)
            remaining()
            agent = {key: candidate[key] for key in ('pid', 'start')}
            agent.update(reason='real_authority_challenge', session=sid)
            self.note('Production polkit registration observed', agent=agent, attempt=attempt,
                      proof=proof, authentication_owner=owner, cancelled_probe=denied)
            return agent
        raise AssertionError('No readiness challenge completed')

    def no_client(self, role, timeout=40):
        return self.wait(role + ' stopped', lambda state: not self.roles(state, role), timeout,
                         operation='process-state')

    def capture(self, name, *, home=False):
        # Call only when the credential field is known empty or no field exists.
        target = self.evidence / name
        self.qmp.request({'execute': 'screendump', 'arguments': {
            'filename': str(target), 'format': 'png'}})
        with Image.open(target) as source:
            image = source.convert('RGB')
        assert image.size == (320, 170), f'Unexpected real DRM output: {image.size}'
        if home:
            pixels = list(image.getdata())
            assert pixels.count((0x10, 0x1d, 0x30)) > 1000, 'Installed Shell is not mapped'
            assert pixels.count((0x75, 0xdc, 0xeb)) > 20, 'Shell text/accent is absent'

    def login(self, wrong=False):
        self.wait('real agreety/getty login prompt', lambda state: state['login_prompt'], 50)
        self.note('Login prompt before username input', public_state=self.call('state'))
        self.text('alcardci')
        self.key('ret')
        self.wait('password prompt on tty1', lambda state: state['password_prompt'])
        self.note('PAM secret prompt before credential input', public_state=self.call('state'))
        self.text(self.bad_password if wrong else self.password)
        self.key('ret')
        if wrong:
            state = self.wait('wrong login password rejected and fresh login prompt', lambda state:
                              state if state['login_failed'] and state['login_prompt']
                              and not self.roles(state, 'shell')
                              and any(p.get('reading_stdin') for p in self.roles(state, 'agreety'))
                              else None, 20)
            assert state['login_prompt_reason'] == 'cursor_suffix'
            assert all(state['tty_markers'][literal]['count'] == 0 for literal in (
                'client loop failed:', 'error: authentication error:', 'pam_authenticate:'))
            assert any(p['uid'] == 0 and p['ppid'] == 1
                       and p['standard_fds']['stderr'] == '/run/alcard-greetd/daemon.log'
                       for p in self.roles(state, 'greetd'))
            assert any(p.get('reading_stdin') and p['standard_fds']['stderr'] == '/dev/tty1'
                       for p in self.roles(state, 'agreety'))
            directory, log = state['greetd_log']['directory'], state['greetd_log']['file']
            assert (directory['uid'], directory['gid'], directory['mode'], directory['type']) == (
                0, 0, '0700', 0o040000)
            assert (log['uid'], log['gid'], log['mode'], log['type'], log['links']) == (
                0, 0, '0600', 0o100000, 1)
            self.note('Wrong greetd/PAM login rejected with clean current prompt',
                      greetd_log=state['greetd_log'])
            return
        role = 'shell' if self.mode == 'graphics' else 'foot'
        client_deadline = time.monotonic() + 60
        client = self.client(role, timeout=60)
        agent = None
        if self.mode == 'graphics':
            # A process exists before its first mapped frame. Share the original
            # 60-second startup budget instead of taking time from auth proof.
            self.wait_shell_frame({'home'}, client_deadline, 'initial mapped Shell Home')
            agent = self.registered_agent()
        time.sleep(1)
        proof = self.call('proof', roles=[role, 'compositor'] + (
            ['agent'] if self.mode == 'graphics' else []))
        if agent is not None:
            assert any(member['process']['role'] == 'agent'
                       and all(member['process'][key] == agent[key] for key in ('pid', 'start'))
                       for member in proof['members']), 'Registered agent lifetime changed'
        sid = self.validate_proof(proof)
        if agent is not None:
            assert agent['session'] == sid, 'Ready proof and login proof belong to different sessions'
        self.note('Real PAM login1 session verified', proof=proof)
        self.health = self.call('service-health')
        assert self.health['private_bus']['session']['Id'] == [sid]
        if self.mode == 'graphics':
            assert self.health['polkit'] is not None
        self.note('Production service ownership and health verified',
                  session=sid, health=self.health)
        return sid, client

    @staticmethod
    def validate_proof(proof, expected_sid=None):
        assert proof['active_vt'] == 'tty1'
        assert proof['drm_devices'] and proof['input_devices']
        assert proof['runtime_mode'] == '0700'
        identifiers = set()
        for member in proof['members']:
            session = member['session']
            for key, expected in {'Name': ['alcardci'], 'Type': ['wayland'],
                                  'Class': ['user'], 'Service': ['alcard-greetd'],
                                  'Active': ['true'], 'Remote': ['false'], 'VTNr': ['1']}.items():
                assert session[key] == expected, (key, session[key])
            assert session['Seat'][0] == 'seat0'
            assert int(session['User'][0]) == proof['runtime_uid']
            if member['process']['role'] == 'pkexec':
                assert member['process']['uid'] in {0, proof['runtime_uid']}
            else:
                assert member['process']['uid'] == proof['runtime_uid']
            identifiers.add(session['Id'][0])
        assert len(identifiers) == 1
        sid = next(iter(identifiers))
        if expected_sid is not None:
            assert sid == expected_sid, 'Authentication request escaped the logged-in session'
        assert proof['environment'].get('XDG_SESSION_ID') == sid
        return sid

    def policy_probe(self, sid, *, correct, label):
        previous = (self.call('state')['probe'] or {}).get('pid')
        deadline = time.monotonic() + 40
        self.probe_menu(deadline)
        self.key('4', deadline=deadline)
        self.wait('new ordinary policy probe', lambda state: state['probe'] and
                  state['probe']['pid'] != previous, max(.01, deadline - time.monotonic()))
        self.wait('production agent authentication form', lambda state:
                  state['presence']['org.alcard.Authentication'])
        self.client('polkit-helper')
        time.sleep(.5)
        proof = self.call('proof', roles=['shell', 'agent', 'probe', 'pkexec'])
        self.validate_proof(proof, sid)
        self.note('Production UnixSession policy challenge', label=label, proof=proof)
        self.capture(label + '-auth-empty.png')
        self.text(self.password if correct else self.bad_password)
        self.key('ret')
        if correct:
            self.wait('pkexec true authorized by real current-user password', lambda state:
                      state['probe'] and state['probe']['result'] == 0, 25)
            self.note('Real distro polkit/PAM authorized benign request', label=label)
        else:
            self.no_client('polkit-helper', timeout=15)
            state = self.call('state')
            assert state['probe']['result'] is None
            assert state['presence']['org.alcard.Authentication']
            self.key('esc')
            self.wait('wrong polkit password then cancellation denied request', lambda state:
                      state['probe'] and state['probe']['result'] not in (None, 0), 15)
            self.note('Wrong polkit password rejected; cancellation denied request', label=label)
        close_deadline = time.monotonic() + 40
        self.wait('authentication surface closed', lambda state:
                  not state['presence']['org.alcard.Authentication'])
        time.sleep(min(2.2, max(0, close_deadline - time.monotonic())))
        self.observed_home(close_deadline)

    def lock(self, sid, *, label):
        self.quick_action(7, 'quick-lock')
        locker = self.client('locker')
        self.wait('independent production locker presence', lambda state:
                  state['presence']['org.alcard.SessionLock'])
        time.sleep(1)
        self.validate_proof(self.call('proof', roles=['shell', 'locker']), sid)
        self.capture(label + '-locked-empty.png')
        return locker

    def unlock(self, *, correct=True):
        self.key('ret')  # Start a fresh PAM conversation from the idle lock form.
        self.client('pam-worker')
        time.sleep(.7)
        self.text(self.password if correct else self.bad_password)
        self.key('ret')
        if correct:
            deadline = time.monotonic() + 40
            self.no_client('locker', timeout=20)
            self.wait('lock presence gone after successful PAM', lambda state:
                      not state['presence']['org.alcard.SessionLock'],
                      max(.01, deadline - time.monotonic()))
            # Lock completion returns Shell to Home. Observe that mapped
            # frame without leaving a delayed Meta request ahead of Quick.
            self.observed_home(deadline)
        else:
            self.no_client('pam-worker', timeout=15)
            assert self.roles(self.call('state'), 'locker')
            self.key('esc')
            self.note('Wrong current-user lock PAM response left compositor locked')

    def validate_resources(self, value, shell):
        # Reuse the established Hello acceptance criteria, with complete CPU
        # accounting and exact pre-sample lifetimes for this login fixture.
        assert value['schema'] == 1 and value['architecture'] == 'aarch64'
        assert value['session_uid'] == self.uid and value['sampler_uid'] == 0
        assert value['requested_duration_seconds'] == 30
        assert value['requested_settle_seconds'] == 10
        assert value['elapsed_seconds'] >= 30, 'Idle window was shorter than requested'
        assert value['cpu']['online_cpus_start'] == value['cpu']['online_cpus_end'] == 2
        assert value['measurement_phase'] == RESOURCE_PHASE
        sampler = Path(__file__).resolve().parents[1] / 'scripts/ci/hello/measure-idle-resources.py'
        assert value['sampler_sha256'] == hashlib.sha256(sampler.read_bytes()).hexdigest()
        assert isinstance(value['boot_id'], str) and len(value['boot_id']) == 36
        expected = value['expected_role_lifetimes']
        assert set(expected) == {'shell', 'compositor', 'agent'}
        assert expected['shell'] == {key: shell[key] for key in ('pid', 'start')}
        quality = value['quality']
        assert quality['session_processes_stable'], 'GUI processes changed during idle sampling'
        assert quality['cpu_count_stable'], 'Guest CPU count changed during idle sampling'
        assert quality['omitted_during_endpoint_scans'] == [0, 0]
        assert quality['entered_lifetime_count'] == 0 and quality['departed'] == []
        assert quality['per_process_cpu_complete_for_observed_lifetimes'] is True
        assert quality['stable_lifetime_count'] == len(value['processes'])
        for process in value['processes']:
            assert process['lifetime'] == 'stable', process
            assert isinstance(process['cpu_ticks_delta'], int) and process['cpu_ticks_delta'] >= 0
            for field in ('cpu_percent_one_core', 'cpu_percent_vm_capacity'):
                assert isinstance(process[field], (int, float)) and math.isfinite(process[field])
                assert process[field] >= 0
            assert process['memory_start'] is not None and process['memory_end'] is not None
        for reading in value['cpu']['roles_disjoint'].values():
            assert reading['unaccounted_process_count'] == 0 and reading['cpu_ticks_delta'] >= 0
        for role in ('shell', 'compositor', 'agent'):
            lifetime = quality['required_roles'][role]
            assert lifetime['same_single_lifetime'], (role, lifetime)
            assert lifetime['initial_count'] == lifetime['final_count'] == 1
            members = [process for process in value['processes']
                       if process['role'] == role and process['uid'] == self.uid]
            assert len(members) == 1, (role, members)
            assert (members[0]['pid'], members[0]['start_ticks']) == (
                expected[role]['pid'], expected[role]['start']), (role, members)
        for endpoint in ('start', 'end'):
            memory = value['memory'][endpoint]
            for category in ('roles_disjoint', 'views_overlapping'):
                for name, reading in memory[category].items():
                    assert reading['unreadable_memory_count'] == 0, (endpoint, name, reading)
            for role in ('shell', 'compositor', 'agent'):
                reading = memory['roles_disjoint'][role]
                assert reading['process_count'] == 1 and reading['pss_kib'] > 0
            for group in ('gui_core', 'user_session'):
                reading = memory['views_overlapping'][group]
                assert reading['process_count'] >= 3 and reading['pss_kib'] > 0

    def sample_resources(self, shell):
        details = dict(report='idle-resources.json', measurement_phase=RESOURCE_PHASE, **RESOURCE_CONTEXT)
        self.note('Sampling guest idle resource sample', **details)
        # No input, state polling or other RPC while the one-shot observer
        # runs the existing sampler's ten-second settle and thirty-second window.
        resources = self.call('idle-resources', timeout=60)
        resources.update(RESOURCE_CONTEXT)
        (self.evidence / 'idle-resources.json').write_text(json.dumps(resources, indent=2) + '\n')
        self.validate_resources(resources, shell)
        self.note('Completed guest idle resource sample', **details)

    def graphics(self):
        self.login(wrong=True)
        sid, shell = self.login()
        self.capture('01-real-login-home.png', home=True)
        self.sample_resources(shell)
        self.policy_probe(sid, correct=False, label='02-rejected')
        self.policy_probe(sid, correct=True, label='03-authorized')
        locker = self.lock(sid, label='04')
        self.unlock(correct=False)
        self.capture('05-lock-rejected-empty.png')
        self.unlock()
        self.capture('06-unlocked-home.png', home=True)
        self.note('Production current-user PAM unlocked the real compositor')

        locker = self.lock(sid, label='07')
        fault = self.call('kill-client', **{key: shell[key] for key in ('pid', 'start')})
        self.note('Sent SIGKILL to owned Shell', fault=fault, locker=locker)
        replacement = self.client('shell', different=shell['pid'])
        state = self.call('state')
        assert any(p['pid'] == locker['pid'] for p in self.roles(state, 'locker'))
        self.note('Shell restart preserved the active independent locker',
                  old_shell=shell, new_shell=replacement, locker=locker)
        self.unlock()
        self.capture('08-shell-restarted-unlocked.png', home=True)

        locker = self.lock(sid, label='09')
        fault = self.call('kill-client', **{key: locker[key] for key in ('pid', 'start')})
        self.note('Sent SIGKILL to owned locker', fault=fault)
        replacement = self.client('locker', different=locker['pid'], timeout=35)
        time.sleep(1)
        self.capture('10-recovered-lock-empty.png')
        self.note('Abandoned compositor lock acquired by fixed production replacement',
                  old_locker=locker, new_locker=replacement)
        self.unlock()
        self.capture('11-recovered-unlocked-home.png', home=True)

        old_agent = self.client('agent')
        result = self.call('service', name='polkit', action='restart', timeout=45)
        self.note('Sent SIGTERM to current production polkit owner', service=result,
                  agent=old_agent)
        new_agent = self.registered_agent(different=old_agent['pid'], timeout=45, expected_sid=sid)
        self.health = self.call('service-health')
        assert self.health['polkit']['owner'] != result['owner']
        assert self.lifetime(self.health['polkit']['process']) != self.lifetime(result['terminated'])
        assert self.health['private_bus']['session']['Id'] == [sid]
        self.note('Production polkit owner replaced through D-Bus activation', service=result,
                  old_agent=old_agent, new_agent=new_agent, health=self.health)
        self.policy_probe(sid, correct=True, label='12-policy-recovery')

        self.logout(sid)
        next_sid, _ = self.login()
        assert next_sid != sid, 'Relogin must create a fresh real session'
        self.capture('13-relogin-home.png', home=True)
        self.note('Real logout and fresh authenticated login completed',
                  old_session=sid, new_session=next_sid)
        sid = next_sid
        for service, label in (
                ('elogind', '14-login-service-recovery'),
                ('private-bus', '15-private-bus-recovery'),
                ('dbus', '16-system-bus-recovery')):
            if service == 'private-bus':
                fault = self.call('terminate-private-bus')
            else:
                fault = self.call('service', name=service, action='restart', timeout=75)
            self.note('Injected login context fault', cause=service, fault=fault)
            # Context loss invalidates the old login even when a daemon restores
            # its records. Production exit 75 must return us to real greetd/PAM.
            self.wait('all old user clients stopped after ' + service + ' loss', lambda state:
                      not any(p['uid'] == self.uid for p in state['processes']), 45)
            recovered_sid, _ = self.login()
            assert recovered_sid != sid, service + ' loss requires a new authenticated session'
            if service == 'elogind':
                assert self.health['elogind']['owner'] != fault['owner']
                assert self.lifetime(self.health['elogind']['process']) != self.lifetime(fault['terminated'])
            elif service == 'private-bus':
                assert self.lifetime(self.health['private_bus']['process']) != self.lifetime(fault['terminated'])
            else:
                for daemon in ('dbus', 'elogind'):
                    assert self.lifetime(self.health[daemon]['process']) != self.lifetime(
                        fault['previous'][daemon]['process']), daemon + ' owner did not restart'
            self.note('Invalid login context returned to greetd and fresh PAM',
                      cause=service, old_session=sid, new_session=recovered_sid,
                      fault=fault, health=self.health)
            self.policy_probe(recovered_sid, correct=True, label=label)
            sid = recovered_sid
        self.logout(sid)

    @staticmethod
    def lifetime(process):
        return process['pid'], process['start']

    def quick_action(self, index, selected_frame):
        assert (index, selected_frame) in {(7, 'quick-lock'), (4, 'quick-logout')}
        deadline = time.monotonic() + 40
        self.observed_home(deadline)
        self.key('5', deadline=deadline)
        self.wait_shell_frame({'quick-menu', 'quick-lock', 'quick-logout'},
                              deadline, 'mapped inline Quick list')
        # Clamp to Brightness, then navigate the exact new row. Every key
        # and the final frame acknowledgement share this one fixed deadline.
        for _ in range(8):
            self.key('up', deadline=deadline)
        for _ in range(index):
            self.key('down', deadline=deadline)
        self.wait_shell_frame({selected_frame}, deadline, 'selected ' + selected_frame)
        self.key('4', deadline=deadline)

    def logout(self, sid):
        before = [p for p in self.call('state')['processes'] if p['uid'] == self.uid]
        self.quick_action(4, 'quick-logout')
        time.sleep(.5)
        self.key('4')  # Explicitly confirm Logout in Shell.
        self.no_client('shell', timeout=30)
        self.no_client('compositor', timeout=30)
        self.no_client('agent', timeout=30)
        self.wait('all old login-user processes stopped', lambda state:
                  not any(p['uid'] == self.uid for p in state['processes']), 30)
        self.wait('greetd prompt after login1.TerminateSession', lambda state: state['login_prompt'])
        current = {(p['pid'], p['start']) for p in self.call('state')['processes']}
        assert not any((p['pid'], p['start']) in current for p in before)
        self.note('Shell logout terminated only the guest login session',
                  session=sid, stopped=before)

    def minimal(self):
        sid, _ = self.login()
        assert not self.roles(self.call('state'), 'shell')
        self.capture('01-minimal-foot.png')
        self.text('exit')
        self.key('ret')
        self.no_client('foot')
        self.no_client('compositor')
        self.wait('greetd after Foot exit', lambda state: state['login_prompt'])
        self.note('Hello Graphics Minimal used real Foot fallback and returned to greetd', session=sid)

    def console(self):
        self.wait('real getty login prompt', lambda state: state['login_prompt'], 50)
        self.text('alcardci')
        self.key('ret')
        self.wait('distribution console password prompt', lambda state: state['password_prompt'])
        self.text(self.password)
        self.key('ret')
        self.client('console-shell')
        self.text('id')
        self.key('ret')
        self.wait('id executed by actual tty1 login user', lambda state: state['console_identity'])
        proof = self.call('console-proof')
        self.capture('01-console-login.png')
        self.text('alcard')
        self.key('minus')
        self.text('settings')
        self.key('ret')
        app = self.client('settings')
        assert app['uid'] == self.uid
        self.wait('Ratatui settings home on tty1', lambda state: state['settings_home'])
        self.key('4')
        self.wait('System settings on tty1', lambda state: state['settings_system'])
        self.capture('02-console-settings.png')
        self.key('8')
        self.wait('back to settings home', lambda state: state['settings_home'])
        self.key('8')
        self.no_client('settings')
        self.text('exit')
        self.key('ret')
        self.no_client('console-shell')
        self.wait('getty prompt after console logout', lambda state: state['login_prompt'])
        self.note('Hello Console used distribution getty/login and returned after exit', proof=proof)

    def finish(self, succeeded):
        if (not succeeded and not self.credentials_entered and self.qmp is not None
                and self.process is not None and self.process.poll() is None):
            try:
                self.capture('00-pre-credentials-failure.png')
                self.note('Captured failed VT before any credential keystroke')
            except Exception as error:
                self.note('Pre-credentials failure capture unavailable', error=type(error).__name__)
        if self.observer is not None:
            try:
                diagnostics = self.call('diagnostics', timeout=35)
                (self.evidence / 'diagnostics.json').write_text(json.dumps(diagnostics, indent=2))
            except Exception as error:
                self.note('Final observer diagnostics unavailable', error=type(error).__name__)
        if self.qmp is not None and self.process.poll() is None:
            try:
                self.qmp.request({'execute': 'quit'}, timeout=5)
            except (RuntimeError, OSError):
                pass
        if self.process is not None:
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait(timeout=5)
        for channel in (self.qmp, self.observer):
            if channel is not None:
                channel.close()
        for stream in (self.serial, self.stderr):
            if stream is not None:
                stream.close()
        for name in ('serial.log', 'qemu.log'):
            source = self.work / name
            if source.is_file():
                (self.evidence / name).write_bytes(source.read_bytes())
        (self.evidence / 'events.json').write_text(json.dumps(
            {'success': succeeded, 'events': self.events}, indent=2))
        secrets_bytes = (self.password.encode(), self.bad_password.encode())
        for path in self.evidence.rglob('*'):
            if path.is_file() and any(value in path.read_bytes() for value in secrets_bytes):
                for exported in self.evidence.iterdir():
                    if exported.is_file():
                        exported.unlink()
                (self.evidence / 'rejected.txt').write_text('Credential leak detected; all evidence suppressed.\n')
                self.private.cleanup()
                raise RuntimeError('Fixture credential leaked; output suppressed')
        self.private.cleanup()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--modes', nargs='+', choices=('graphics', 'graphics_minimal', 'console'),
                        default=['graphics', 'graphics_minimal', 'console'])
    arguments = parser.parse_args()
    if os.getuid() == 0 or os.environ.get('ALCARD_LOGIN_VM_TEST') != '1':
        raise SystemExit('Only the isolated nonroot CI host may run this gate')
    for mode in arguments.modes:
        machine = Machine(arguments.directory.resolve(), mode)
        success = False
        try:
            machine.start()
            if mode == 'graphics':
                machine.graphics()
            elif mode == 'graphics_minimal':
                machine.minimal()
            else:
                machine.console()
            success = True
        finally:
            machine.finish(success)
        print(f'PASS: real installed {mode} login, authentication and cleanup', flush=True)


if __name__ == '__main__':
    main()
