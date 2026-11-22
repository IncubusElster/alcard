#!/usr/bin/python3
"""Private root observer for one isolated VM. Never shipped in a runtime APK.

The host can provision through real Hello, observe public session metadata, and
inject a short allowlist of guest-local faults. It cannot return a PAM result,
register a session, alter policy, or authorize a polkit request.
"""
import hashlib
import json
import os
from pathlib import Path
import pwd
import grp
import re
import shlex
import signal
import stat
import subprocess
import time


ROOT = Path('/run/alcard-ci')
USER = 'alcardci'
PORT = Path('/dev/virtio-ports/org.alcard.ci')
RESOURCE_SAMPLE_STARTED = False
PROGRAMS = {
    '/usr/bin/alcard-settings': 'settings',
    '/usr/bin/alcard-shell': 'shell',
    '/usr/bin/alcard-polkit-agent': 'agent',
    '/usr/bin/alcard-lock': 'locker',
    '/usr/bin/labwc': 'compositor',
    '/usr/bin/foot': 'foot',
    '/usr/bin/pkexec': 'pkexec',
    '/usr/lib/polkit-1/polkit-agent-helper-1': 'polkit-helper',
    '/usr/lib/polkit-1/polkitd': 'polkitd',
    '/usr/libexec/elogind/elogind': 'elogind',
    '/usr/bin/dbus-daemon': 'bus-daemon',
    '/usr/sbin/greetd': 'greetd',
    '/usr/bin/greetd': 'greetd',
    '/usr/bin/agreety': 'agreety',
    '/usr/bin/setfont': 'setfont',
    '/bin/setfont': 'setfont',
}
SCRIPT_ROLES = {
    b'/usr/libexec/alcard-ci/boot': 'ci-boot',
    b'/usr/libexec/alcard-ci/observer.py': 'ci-observer',
    b'/usr/libexec/alcard/tty1-dispatch': 'tty-dispatch',
    b'/usr/libexec/alcard-ci/policy-probe': 'probe',
}
AGENT_REGISTERED = 'Alcard graphical polkit agent registered'
AGENT_EXITED = 'Alcard session: alcard-polkit-agent exited'
AGENT_ERROR = 'Alcard authentication agent:'
GREETD_DIAGNOSTICS = (
    'client loop failed: i/o error: Broken pipe (os error 32)',
    'error: authentication error: pam_authenticate: AUTH_ERR',
)
TTY_MARKERS = (
    'login:', 'password:', 'login incorrect', 'client loop failed:',
    'error: authentication error:', 'pam_authenticate:',
    *GREETD_DIAGNOSTICS,
    AGENT_REGISTERED, AGENT_EXITED, AGENT_ERROR,
    'Timeout was reached', 'Connection timed out',
    'org.freedesktop.DBus.Error.AccessDenied',
    'org.freedesktop.PolicyKit1.Error.Failed',
    'Authentication services changed during registration',
)


def run(argv, *, data=None, timeout=15):
    result = subprocess.run(argv, input=data, capture_output=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(f'{Path(argv[0]).name} returned {result.returncode}')
    return result.stdout.decode(errors='replace').strip()


def agreety_reading_stdin(path, start):
    # This fixture boots only aarch64: Linux asm-generic assigns read syscall
    # 63. Inspect only its identity and fd, never export syscall arguments or
    # buffers. Recheck the PID lifetime after the observation.
    if os.uname().machine != 'aarch64':
        return False
    try:
        with (path / 'syscall').open() as stream:
            syscall = stream.read(512).split()
        fields = (path / 'stat').read_text().rsplit(')', 1)[1].split()
        return (len(syscall) >= 2 and int(syscall[0], 0) == 63
                and int(syscall[1], 0) == 0 and fields[0] == 'S'
                and int(fields[19]) == start)
    except (OSError, ValueError, IndexError):
        return False


def process(pid):
    path = Path('/proc') / str(pid)
    try:
        executable = os.readlink(path / 'exe')
        arguments = (path / 'cmdline').read_bytes().split(b'\0')
        status = (path / 'status').read_text().splitlines()
        real_uid = int(next(line for line in status if line.startswith('Uid:')).split()[1])
        try:
            account_uid = uid()
        except KeyError:
            account_uid = -1
        role = PROGRAMS.get(executable)
        for script, script_role in SCRIPT_ROLES.items():
            if script in arguments:
                role = script_role
                break
        if executable.endswith('/busybox') and arguments:
            name = Path(arguments[0].decode(errors='replace')).name.lstrip('-')
            if name in {'ash', 'sh'} and real_uid == account_uid:
                role = 'console-shell'
        if role == 'locker' and b'--pam-worker' in arguments:
            role = 'pam-worker'
        if (Path(executable).name == 'supervise-daemon' and real_uid == 0
                and b'elogind' in arguments[1:]):
            role = 'elogind-supervisor'
        if role is None and real_uid == account_uid:
            role = 'session-other'
        if role is None:
            return None
        # comm may contain spaces or parentheses: split after its final ')'.
        fields = (path / 'stat').read_text().rsplit(')', 1)[1].split()
        result = {'pid': int(pid), 'ppid': int(fields[1]), 'start': int(fields[19]),
                  'uid': real_uid, 'role': role, 'exe': executable, 'state': fields[0]}
        if role in {'greetd', 'agreety'}:
            result['tty_state'] = {'process_group': int(fields[2]), 'unix_sid': int(fields[3]),
                                   'device': int(fields[4]), 'foreground_group': int(fields[5])}
            try:
                wait = (path / 'wchan').read_text().strip()[:80]
                result['wchan'] = wait if re.fullmatch(r'[A-Za-z0-9_]+', wait) else 'unavailable'
            except OSError:
                result['wchan'] = 'unavailable'
            result['standard_fds'] = {}
            for number, name in enumerate(('stdin', 'stdout', 'stderr')):
                try:
                    target = os.readlink(path / 'fd' / str(number))
                    # Report only fixed terminal names or a descriptor class,
                    # never an arbitrary filename or process argument.
                    if target not in {'/dev/tty1', '/dev/console', '/dev/null',
                                      '/run/alcard-greetd/daemon.log'}:
                        target = 'pipe' if target.startswith('pipe:') else (
                            'socket' if target.startswith('socket:') else 'other')
                    result['standard_fds'][name] = target
                except OSError:
                    result['standard_fds'][name] = 'unavailable'
            if role == 'agreety':
                result['reading_stdin'] = agreety_reading_stdin(path, result['start'])
        return result
    except (FileNotFoundError, PermissionError, ProcessLookupError, StopIteration):
        return None


def processes():
    return [item for path in Path('/proc').iterdir() if path.name.isdigit()
            and (item := process(int(path.name))) and item['state'] != 'Z']


def process_state():
    # A process-lifecycle predicate must not wait for session environment or
    # D-Bus presence. In particular, discovering Shell must not add bus probes
    # to the same observation that establishes its appearance.
    started = time.monotonic()
    items = processes()
    observed = time.monotonic()
    return {'processes': items, 'guest_monotonic': observed,
            'observation_seconds': round(observed - started, 6)}


def idle_resources():
    # One fixed read-only sample in the first Graphics session. No caller
    # command, path, UID or duration is accepted, and no bus polling occurs
    # while the established sampler sleeps between its two /proc endpoints.
    global RESOURCE_SAMPLE_STARTED
    assert not RESOURCE_SAMPLE_STARTED, 'Only one resource sample per guest is allowed'
    assert Path('/etc/alcard/default-session').read_text().strip() == 'alcard-shell'
    account_uid = uid()
    current = processes()
    subjects = {}
    for role in ('shell', 'compositor', 'agent'):
        selected = [item for item in current if item['uid'] == account_uid and item['role'] == role]
        assert len(selected) == 1, 'Resource sample requires one live ' + role
        subjects[role] = {key: selected[0][key] for key in ('pid', 'start')}
    assert not any(item['role'] in ('probe', 'foot', 'pkexec', 'polkit-helper', 'pam-worker', 'locker')
                   for item in current), 'Resource sample requires a quiet unlocked session'
    RESOURCE_SAMPLE_STARTED = True
    sampler = Path('/usr/libexec/alcard-ci/measure-idle-resources.py')
    sampler_sha256 = hashlib.sha256(sampler.read_bytes()).hexdigest()
    boot_id = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
    result = subprocess.run([
        '/usr/bin/python3', str(sampler), '--uid', str(account_uid),
        '--settle', '10', '--duration', '30'],
        stdin=subprocess.DEVNULL, capture_output=True, timeout=55)
    assert result.returncode == 0, 'Idle resource sampler failed'
    value = json.loads(result.stdout)
    value.update(boot_id=boot_id, sampler_sha256=sampler_sha256,
                 expected_role_lifetimes=subjects,
                 measurement_phase='first_graphics_login_after_readiness_cancel_before_policy_lock_faults')
    return value


def uid():
    return pwd.getpwnam(USER).pw_uid


def session_environment():
    candidates = [p for p in processes() if p['uid'] == uid()
                  and p['role'] in {'shell', 'foot', 'compositor'}]
    for item in candidates:
        try:
            entries = (Path('/proc') / str(item['pid']) / 'environ').read_bytes().split(b'\0')
            environment = dict(entry.decode().split('=', 1) for entry in entries if b'=' in entry)
            if environment.get('DBUS_SESSION_BUS_ADDRESS'):
                # Never copy arbitrary process environment or construct a SID.
                return {key: environment[key] for key in (
                    'XDG_RUNTIME_DIR', 'DBUS_SESSION_BUS_ADDRESS', 'WAYLAND_DISPLAY',
                    'XDG_SESSION_ID', 'XDG_SESSION_TYPE') if key in environment}
        except (FileNotFoundError, PermissionError):
            pass
    return {}


def bus_arguments(address=None):
    args = ['busctl', '--timeout=3']
    if address:
        # The distribution session policy admits its owning UID, not arbitrary
        # root clients. Only this read-only observer subprocess drops identity;
        # it neither creates a PAM session nor changes the bus policy.
        account = pwd.getpwnam(USER)
        return ['/bin/setpriv', '--reuid', str(account.pw_uid), '--regid',
                str(account.pw_gid), '--init-groups', *args, '--address=' + address]
    return [*args, '--system']


def bus_name(name, address=None):
    output = run(bus_arguments(address) + ['call', 'org.freedesktop.DBus', '/org/freedesktop/DBus',
                        'org.freedesktop.DBus', 'NameHasOwner', 's', name], timeout=5)
    return output == 'b true'


def property_value(path, key):
    value = run(['busctl', '--system', '--timeout=3', 'get-property',
                 'org.freedesktop.login1', path, 'org.freedesktop.login1.Session', key])
    return shlex.split(value)[1:]


def session_for_pid(pid):
    value = run(['busctl', '--system', '--timeout=3', 'call',
                 'org.freedesktop.login1', '/org/freedesktop/login1',
                 'org.freedesktop.login1.Manager', 'GetSessionByPID', 'u', str(pid)])
    path = shlex.split(value)[1]
    values = {key: property_value(path, key) for key in (
        'Id', 'Name', 'User', 'Type', 'Class', 'Service', 'Active', 'Remote',
        'Seat', 'VTNr', 'TTY', 'Leader', 'State')}
    return {'object': path, **values}


def tty_snapshot():
    # Linux vcsu supplies native u32 Unicode cells without an attribute header.
    # This Alpine aarch64 guest is little endian. vcs/vcsa cell bytes are font
    # glyph indexes after production setfont, so they cannot be decoded as ASCII.
    geometry = None
    try:
        with Path('/dev/vcsa1').open('rb', buffering=0) as stream:
            header = stream.read(4)
        if len(header) != 4:
            return '', '', 'short_header', geometry
        rows, columns, x, y = header
        geometry = {'rows': rows, 'columns': columns, 'x': x, 'y': y}
        cells = rows * columns
        if not (0 < cells <= 16384 and y < rows and x <= columns):
            return '', '', 'invalid_geometry', geometry
        with Path('/dev/vcsu1').open('rb', buffering=0) as stream:
            data = stream.read(cells * 4)
        if len(data) != cells * 4:
            return '', '', 'short_unicode_read', geometry
        text = data.decode('utf-32-le', errors='replace')
        cursor = y * columns + x
        # A 40-column VT may wrap the label itself onto the next row. Inspect
        # only the bounded suffix at the current cursor, never an old prompt
        # elsewhere on the screen. A live Password prompt supersedes Login.
        prompt = text[max(0, cursor - 2 * columns):cursor].strip().lower()
        if x == columns - 1:
            # Linux defers wrapping after the final column: the cursor can
            # still point AT the final printed colon/space. Keep the exclusive
            # candidate too, because that cell can instead contain stale text.
            inclusive = text[max(0, cursor + 1 - 2 * columns):cursor + 1].strip().lower()
            if inclusive.endswith(('login:', 'password:')):
                prompt = inclusive
        return text, prompt, None, geometry
    except OSError as error:
        return '', '', f'errno_{error.errno}', geometry


def registration_tty_hint(markers):
    # Current VT text is ephemeral and cannot establish a process lifetime.
    # Keep it only as a diagnostic; the host proves a fresh real challenge.
    ready = markers[AGENT_REGISTERED]['starts']
    failures = [start for literal in (AGENT_EXITED, AGENT_ERROR)
                for start in markers[literal]['starts']]
    return {'ready_marker_visible': bool(ready),
            'ready_after_visible_error': bool(ready and (not failures or ready[-1] > max(failures)))}


def authentication_owner(address):
    # Public session-bus ownership only; no authentication method/cookie/body is
    # observed or invoked. Tie the UI presence to an exact live agent lifetime.
    prefix = bus_arguments(address) + ['call', 'org.freedesktop.DBus',
                                      '/org/freedesktop/DBus', 'org.freedesktop.DBus']
    signature, owner = shlex.split(run(prefix + ['GetNameOwner', 's', 'org.alcard.Authentication']))
    assert signature == 's' and owner.startswith(':')
    signature, number = shlex.split(run(prefix + ['GetConnectionUnixProcessID', 's', owner]))
    assert signature == 'u' and re.fullmatch(r'[1-9][0-9]*', number)
    target = process(int(number))
    assert target and target['uid'] == uid() and target['role'] == 'agent' and target['state'] != 'Z'
    signature, current_owner = shlex.split(run(prefix + ['GetNameOwner', 's', 'org.alcard.Authentication']))
    current = process(target['pid'])
    assert signature == 's' and current_owner == owner
    assert current and all(current[key] == target[key] for key in ('pid', 'start', 'uid', 'exe'))
    return {'owner': owner, 'process': current}


def greetd_log_metadata():
    result = {}
    for name, path in (('directory', Path('/run/alcard-greetd')),
                       ('file', Path('/run/alcard-greetd/daemon.log'))):
        try:
            info = path.lstat()
            result[name] = {'uid': info.st_uid, 'gid': info.st_gid,
                            'mode': f'{stat.S_IMODE(info.st_mode):04o}',
                            'type': stat.S_IFMT(info.st_mode), 'links': info.st_nlink}
        except FileNotFoundError:
            result[name] = {'missing': True}
    # The daemon's private log contents are never part of the observer protocol.
    return result


def state():
    items = processes()
    environment = session_environment() if any(p['role'] == 'shell' for p in items) else {}
    address = environment.get('DBUS_SESSION_BUS_ADDRESS')
    presence = {}
    for name in ('org.alcard.Authentication', 'org.alcard.SessionLock'):
        try:
            presence[name] = bool(address and bus_name(name, address))
        except (RuntimeError, subprocess.TimeoutExpired):
            presence[name] = False
    auth_owner = None
    auth_owner_error = None
    if presence['org.alcard.Authentication']:
        try:
            auth_owner = authentication_owner(address)
        except (AssertionError, RuntimeError, subprocess.TimeoutExpired, ValueError) as error:
            auth_owner_error = type(error).__name__
    probe_path = Path('/home/alcardci/.local/state/alcard-ci/probe.json')
    try:
        probe = json.loads(probe_path.read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        probe = None
    text, prompt, tty_error, tty_geometry = tty_snapshot()
    markers = {}
    cursor = (tty_geometry['y'] * tty_geometry['columns'] + tty_geometry['x']
              if tty_geometry is not None else None)
    for literal in TTY_MARKERS:
        matches = list(re.finditer(re.escape(literal), text, re.IGNORECASE))
        markers[literal] = {'count': len(matches), 'starts': [match.start() for match in matches[-16:]],
                            'last_end_minus_cursor': (
                                matches[-1].end() - cursor if matches and cursor is not None else None)}
    tty = Path('/sys/class/tty/tty0/active').read_text().strip()
    # Production routes inherited daemon stderr away from the greeter. Require
    # the normal current-cursor label; never classify an old label through logs.
    prompt_reason = 'cursor_suffix' if prompt.endswith('login:') else None
    text = text.lower()
    return {'processes': items, 'presence': presence, 'probe': probe,
            'authentication_owner': auth_owner, 'authentication_owner_error': auth_owner_error,
            'tty': tty,
            'tty_unicode_error': tty_error,
            'tty_geometry': tty_geometry,
            'tty_markers': markers,
            'agent_ready_tty_hint': registration_tty_hint(markers),
            'greetd_log': greetd_log_metadata(),
            'login_prompt': prompt_reason is not None, 'login_prompt_reason': prompt_reason,
            'password_prompt': prompt.endswith('password:'),
            # Only prompt booleans leave the guest; never export raw console text.
            'login_failed': any(message in text for message in (
                'authentication failed', 'login failed', 'login incorrect')),
            'settings_home': any(title in text for title in ('alcard / settings', 'alcard / настройки', 'alcard / ajustes', 'alcard / configuración')),
            'settings_system': 'alpine linux' in text,
            'console_identity': bool(re.search(r'uid=\d+\(alcardci\)', text))}


def setup(request):
    if (ROOT / 'setup-complete').exists():
        raise RuntimeError('This VM was already provisioned')
    mode = request['mode']
    if mode not in {'graphics', 'graphics_minimal', 'console'}:
        raise ValueError('Unknown session mode')
    password = request.pop('password')
    if not re.fullmatch('[0-9a-f]{24}', password):
        raise ValueError('Fixture credential format rejected')
    # All secrets travel through stdin. Restrictive umask exercises explicit
    # modes for public polkit policy and private doas configuration.
    os.umask(0o077)
    Path('/var/lib/alcard').mkdir(parents=True, mode=0o700, exist_ok=True)
    os.chmod('/var/lib/alcard', 0o700)
    run(['/usr/sbin/alcard-setupd', '--arm-firstboot'])
    body = {
        'plan': {'schema_version': 1, 'language': 'en',
                 'account': {'username': USER, 'display_name': 'Alcard CI', 'doas_enabled': True},
                 'keyboard': {'layouts': ['us']}, 'session': mode,
                 'timezone': 'Etc/UTC', 'ntp_enabled': False},
        'credentials': {'user_password': password, 'user_confirm': password,
                        'root_password': '', 'root_confirm': ''},
    }
    completed = subprocess.run(['/usr/sbin/alcard-setupd', '--apply'],
                               input=json.dumps(body).encode(), capture_output=True, timeout=90)
    if completed.returncode:
        # Setupd diagnostics are useful, but never export a literal credential.
        detail = (completed.stdout + completed.stderr).decode(errors='replace')
        raise RuntimeError('Hello apply failed: ' + detail.replace(password, '[redacted]')[-4000:])
    del password, body, completed
    # Give settings a real terminal so a refusal proves the completed-install guard.
    import pty
    master, slave = pty.openpty()
    try:
        refused = subprocess.run(['/usr/bin/alcard-settings', '--hello'], stdin=slave,
                                 stdout=slave, stderr=subprocess.PIPE, timeout=5,
                                 env={**os.environ, 'LC_ALL': 'C'})
        assert refused.returncode and b'Initial setup is unavailable.' in refused.stderr
        assert not Path('/var/lib/alcard/hello.pending').exists()
    finally:
        os.close(slave)
        os.close(master)
    account = pwd.getpwnam(USER)
    assert 1000 <= account.pw_uid < 65534, 'Hello must create an ordinary fresh user'
    policy = Path('/etc/polkit-1/rules.d/49-alcard-admin.rules')
    doas = Path('/etc/doas.d/alcard.conf')
    if mode == 'console':
        assert not policy.exists(), 'Console must not retain a graphical admin rule'
    else:
        assert stat.S_IMODE(policy.stat().st_mode) == 0o644 and policy.stat().st_uid == 0
    assert stat.S_IMODE(doas.stat().st_mode) == 0o600 and doas.stat().st_uid == 0
    root_locked = next(line for line in Path('/etc/shadow').read_text().splitlines()
                       if line.startswith('root:')).split(':')[1].startswith(('!', '*'))
    assert root_locked
    groups = [group.gr_name for group in grp.getgrall() if USER in group.gr_mem]
    assert not {'wheel', 'video', 'input'} & set(groups), 'No privileged group bypass is permitted'
    if mode == 'graphics':
        directory = Path(account.pw_dir) / '.local/share/applications'
        directory.mkdir(parents=True, mode=0o755, exist_ok=True)
        desktop = directory / 'alcard-ci-policy.desktop'
        desktop.write_text('[Desktop Entry]\nType=Application\nName=000 CI Policy Probe\n'
                           'Exec=/usr/libexec/alcard-ci/policy-probe\nTerminal=true\n')
        for path in (Path(account.pw_dir) / '.local', directory.parent, directory, desktop):
            os.chown(path, account.pw_uid, account.pw_gid)
            os.chmod(path, 0o644 if path == desktop else 0o755)
    (ROOT / 'setup-complete').touch(mode=0o600)
    return {'uid': account.pw_uid, 'mode': mode, 'root_locked': root_locked,
            'groups': groups, 'polkit_policy': None if mode == 'console' else policy.read_text(),
            'polkit_policy_mode': None if mode == 'console' else '0644', 'doas_policy_mode': '0600',
            'default_session': Path('/etc/alcard/default-session').read_text().strip()}


def proof(request):
    wanted = request.get('roles', ['shell', 'agent', 'compositor'])
    if not set(wanted) <= {'shell', 'agent', 'compositor', 'locker', 'probe', 'pkexec', 'foot'}:
        raise ValueError('Unknown proof role')
    items = processes()
    probe_pids = {p['pid'] for p in items if p['role'] == 'probe' and p['uid'] == uid()}
    selected = [p for p in items if p['role'] in wanted and (
        p['uid'] == uid() or (p['role'] == 'pkexec' and p['ppid'] in probe_pids))]
    if not set(wanted) <= {p['role'] for p in selected}:
        missing = sorted(set(wanted) - {p['role'] for p in selected})
        raise RuntimeError('Required real session process absent: ' + ','.join(missing))
    result = [{'process': item, 'session': session_for_pid(item['pid'])} for item in selected]
    environment = session_environment()
    runtime = Path(environment.get('XDG_RUNTIME_DIR', '/nonexistent'))
    info = runtime.stat()
    assert info.st_uid == uid() and stat.S_IMODE(info.st_mode) == 0o700
    assert Path('/sys/fs/cgroup/cgroup.controllers').is_file(), 'Expected real cgroup v2 hierarchy'
    return {'members': result, 'runtime': str(runtime), 'runtime_mode': '0700',
            'runtime_uid': info.st_uid, 'environment': environment,
            'active_vt': Path('/sys/class/tty/tty0/active').read_text().strip(),
            'drm_devices': sorted(str(p) for p in Path('/dev/dri').glob('card*')),
            'input_devices': sorted(str(p) for p in Path('/dev/input').glob('event*'))}


def kill_client(request):
    target = process(int(request['pid']))
    if not target or target['uid'] != uid() or target['role'] not in {
            'shell', 'agent', 'locker', 'pam-worker'}:
        raise RuntimeError('Fault target is not an owned production client')
    if target['start'] != request['start']:
        raise RuntimeError('Fault target lifetime changed')
    os.kill(target['pid'], signal.SIGKILL)
    return {'killed': target}


def console_proof():
    selected = [p for p in processes() if p['role'] == 'console-shell' and p['uid'] == uid()]
    assert selected, 'Real console login shell is absent'
    for item in selected:
        item['stdin'] = os.readlink(Path('/proc') / str(item['pid']) / 'fd/0')
        assert item['stdin'] == '/dev/tty1'
    assert not any(p['role'] in {'greetd', 'agreety', 'shell', 'compositor'} for p in processes())
    assert Path('/etc/alcard/default-session').read_text().strip() == 'console'
    assert state()['console_identity'], 'Real user id command did not run on tty1'
    for service_name in ('dbus', 'networkmanager'):
        assert Path('/run/openrc/started', service_name).exists(), f'Missing {service_name}'
    output = run(['/usr/bin/nmcli', '-t', '-f', 'DEVICE,TYPE', 'device', 'status'])
    assert 'lo:loopback' in output
    return {'members': selected, 'tty': 'tty1', 'user': USER,
            'greetd_present': False, 'desktop_present': False}


def service(request):
    name, action = request['name'], request['action']
    if name not in {'polkit', 'elogind', 'dbus'} or action != 'restart':
        raise ValueError('Only guest-local polkit/elogind/dbus restart is allowed')
    if name in {'polkit', 'elogind'}:
        return terminate_system_owner(name)
    # Only the system bus is OpenRC-managed. Stock D-Bus activation owns
    # elogind, which exits when its system-bus connection is lost.
    previous = {service: system_owner(service) for service in ('dbus', 'elogind')}
    output = run(['/sbin/rc-service', 'dbus', 'restart'], timeout=60)
    return {'service': name, 'action': action, 'previous': previous,
            'output': output[-2000:]}


def system_owner(name):
    # These queries go only to the bus driver and cannot activate a daemon.
    # Neither service names nor executable targets come from an RPC argument.
    owners = {
        'polkit': ('org.freedesktop.PolicyKit1', 'polkitd',
                   '/usr/lib/polkit-1/polkitd', 'polkitd'),
        'elogind': ('org.freedesktop.login1', 'root',
                    '/usr/libexec/elogind/elogind', 'elogind'),
        'dbus': ('org.freedesktop.DBus', 'messagebus',
                 '/usr/bin/dbus-daemon', 'bus-daemon'),
    }
    well_known, account, executable, role = owners[name]
    prefix = bus_arguments() + ['call', 'org.freedesktop.DBus',
                               '/org/freedesktop/DBus', 'org.freedesktop.DBus']
    if name == 'dbus':
        owner = well_known
    else:
        signature, owner = shlex.split(run(prefix + ['GetNameOwner', 's', well_known]))
        assert signature == 's' and owner.startswith(':')
    signature, number = shlex.split(run(prefix + [
        'GetConnectionUnixProcessID', 's', owner]))
    assert signature == 'u' and re.fullmatch(r'[1-9][0-9]*', number)
    target = process(int(number))
    assert target and target['uid'] == pwd.getpwnam(account).pw_uid
    assert target['exe'] == executable and target['role'] == role and target['state'] != 'Z'
    if name != 'dbus':
        current_signature, current_owner = shlex.split(run(prefix + [
            'GetNameOwner', 's', well_known]))
        assert current_signature == 's' and current_owner == owner
    current = process(target['pid'])
    assert current and all(current[key] == target[key] for key in ('start', 'uid', 'exe'))
    assert current['state'] != 'Z'
    return {'name': well_known, 'owner': owner, 'process': current}


def terminate_system_owner(name):
    # Production activates polkit and elogind through the stock bus service
    # files. Never start the replacement or add an OpenRC supervisor here.
    assert name in {'polkit', 'elogind'}
    resolved = system_owner(name)
    target = resolved['process']
    descriptor = os.pidfd_open(target['pid'])
    try:
        current = system_owner(name)
        assert current['owner'] == resolved['owner']
        assert all(current['process'][key] == target[key] for key in ('pid', 'start', 'uid', 'exe'))
        signal.pidfd_send_signal(descriptor, signal.SIGTERM)
    finally:
        os.close(descriptor)
    return {'service': name, 'action': 'restart', 'fault_kind': 'owner_termination',
            'owner': resolved['owner'], 'terminated': target, 'signal': 'SIGTERM'}


def private_bus_owner():
    environment = session_environment()
    address = environment.get('DBUS_SESSION_BUS_ADDRESS')
    assert address, 'The logged-in session has no private bus address'
    value = run(bus_arguments(address) + [
        'call', 'org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
        'GetConnectionUnixProcessID', 's', 'org.freedesktop.DBus'])
    signature, number = shlex.split(value)
    assert signature == 'u' and re.fullmatch(r'[1-9][0-9]*', number)
    target = process(int(number))
    assert target and target['uid'] == uid() and target['exe'] == '/usr/bin/dbus-daemon'
    assert target['state'] != 'Z'
    owner_session = session_for_pid(target['pid'])
    client = next(p for p in processes() if p['role'] in {'shell', 'foot'} and p['uid'] == uid())
    client_session = session_for_pid(client['pid'])
    assert owner_session['Id'] == client_session['Id']
    assert owner_session['Name'] == [USER] and owner_session['Service'] == ['alcard-greetd']
    current = process(target['pid'])
    assert current and all(current[key] == target[key] for key in ('start', 'uid', 'exe'))
    assert current['state'] != 'Z'
    return {'process': current, 'session': owner_session}


def terminate_private_bus():
    resolved = private_bus_owner()
    target = resolved['process']
    # The kernel handle and a second starttime check close PID-reuse ambiguity.
    # This fixed operation accepts no user-supplied target or signal.
    descriptor = os.pidfd_open(target['pid'])
    try:
        current = process(target['pid'])
        assert current and current['start'] == target['start'] and current['uid'] == uid()
        signal.pidfd_send_signal(descriptor, signal.SIGTERM)
    finally:
        os.close(descriptor)
    return {'terminated': target, 'session': resolved['session'], 'signal': 'SIGTERM'}


def service_health():
    # Inspect the exact production activation path, not merely a running PID.
    # An installed elogind-openrc subpackage is allowed, but it must not be an
    # enabled or running second launcher competing with D-Bus activation.
    service_name = 'org.freedesktop.login1.service'
    vendor = Path('/usr/share/dbus-1/system-services') / service_name
    for directory in ('/etc/dbus-1/system-services', '/run/dbus-1/system-services',
                      '/usr/local/share/dbus-1/system-services'):
        assert not os.path.lexists(Path(directory) / service_name), 'Elogind activation override'
    info = vendor.lstat()
    assert stat.S_ISREG(info.st_mode) and info.st_uid == info.st_gid == 0
    assert stat.S_IMODE(info.st_mode) == 0o644 and info.st_nlink == 1
    assert info.st_size <= 4096, 'Oversized vendor elogind activation file'
    lines = [line.strip() for line in vendor.read_text().splitlines()
             if line.strip() and not line.lstrip().startswith('#')]
    assert len(lines) == 4 and lines[0] == '[D-BUS Service]' and set(lines[1:]) == {
        'Name=org.freedesktop.login1',
        'Exec=/usr/libexec/elogind/elogind --daemon', 'User=root',
    }, 'Unexpected vendor elogind activation'
    daemon = Path('/usr/libexec/elogind/elogind').lstat()
    assert stat.S_ISREG(daemon.st_mode) and daemon.st_uid == daemon.st_gid == 0
    assert stat.S_IMODE(daemon.st_mode) == 0o755 and daemon.st_nlink == 1
    assert not list(Path('/etc/runlevels').glob('*/elogind')), 'Elogind OpenRC autostart remains'
    assert not any(os.path.lexists('/run/openrc/' + state + '/elogind')
                   for state in ('started', 'failed')), 'Elogind remains OpenRC-managed'
    items = processes()
    assert not any(p['role'] == 'elogind-supervisor' for p in items), 'Competing elogind supervisor'
    login_owner = system_owner('elogind')
    live = [p for p in processes() if p['role'] == 'elogind']
    assert len(live) == 1 and all(live[0][key] == login_owner['process'][key]
                                for key in ('pid', 'start', 'uid', 'exe'))
    assert live[0]['uid'] == 0 and live[0]['ppid'] == 1, 'Expected stock daemonized elogind'

    status = subprocess.run(['/sbin/rc-service', 'dbus', 'status'],
                            capture_output=True, timeout=10)
    assert status.returncode == 0, 'OpenRC dbus service is not healthy'
    assert os.path.lexists('/run/openrc/started/dbus'), 'OpenRC dbus is not started'
    assert not os.path.lexists('/run/openrc/failed/dbus'), 'OpenRC dbus is failed'
    system_bus = system_owner('dbus')
    pidfile = Path('/run/dbus.pid')
    info = pidfile.lstat()
    assert stat.S_ISREG(info.st_mode) and info.st_uid == info.st_gid == 0
    assert not stat.S_IMODE(info.st_mode) & 0o022 and info.st_nlink == 1 and info.st_size <= 32
    number = pidfile.read_text().strip()
    assert re.fullmatch(r'[1-9][0-9]*', number), 'Invalid OpenRC dbus PID file'
    assert int(number) == system_bus['process']['pid'], 'OpenRC and actual system bus disagree'
    polkit = system_owner('polkit') if any(
        p['role'] == 'agent' and p['uid'] == uid() for p in processes()) else None
    return {'elogind': {'activation': str(vendor), 'activation_mode': '0644',
                         'launcher': 'vendor_dbus', 'openrc_enabled': False,
                         'supervisor_present': False, **login_owner},
            'dbus': {'openrc_status': 'started', 'pidfile_matches_owner': True, **system_bus},
            'private_bus': private_bus_owner(), 'polkit': polkit}


def setup_metadata():
    # Fixed, public metadata only: no account database, credential, or marker
    # contents are returned, even when firstboot fails midway through apply.
    result = {'files': {}, 'service_accounts': {}}
    try:
        account = pwd.getpwnam(USER)
        result['account'] = {'exists': True, 'uid': account.pw_uid, 'gid': account.pw_gid}
    except KeyError:
        result['account'] = {'exists': False}
    for name in ('root', 'polkitd', 'messagebus', 'greeter', 'greetd'):
        try:
            account = pwd.getpwnam(name)
            result['service_accounts'][name] = {'exists': True, 'uid': account.pw_uid,
                                                'gid': account.pw_gid}
        except KeyError:
            result['service_accounts'][name] = {'exists': False}
    for name in (
            '/', '/etc', '/usr', '/usr/bin', '/etc/doas.d', '/usr/bin/doas',
            '/etc/polkit-1', '/etc/polkit-1/rules.d',
            '/etc/polkit-1/rules.d/49-alcard-admin.rules', '/etc/doas.d/alcard.conf',
            '/etc/init.d/loadkmap', '/sbin/rc-update', '/bin/rc-status', '/sbin/rc-service',
            '/var/lib/alcard', '/var/lib/alcard/hello.pending',
            '/var/lib/alcard/hello.progress', '/var/lib/alcard/hello.complete',
            '/run/alcard-ci/setup-complete',
            '/run/nologin', '/etc/nologin',
            '/etc/keymap/alcard.bmap.gz', '/etc/localtime', '/etc/alcard/default-session',
            '/run/openrc/started/dmesg',
            '/run/openrc/started/dbus', '/run/openrc/started/elogind',
            '/run/openrc/started/polkit', '/run/openrc/started/cgroups'):
        path = Path(name)
        try:
            info = path.lstat()
            result['files'][name] = {'uid': info.st_uid, 'gid': info.st_gid,
                                    'mode': f'{stat.S_IMODE(info.st_mode):04o}',
                                    'type': stat.S_IFMT(info.st_mode)}
            if stat.S_ISLNK(info.st_mode):
                result['files'][name]['target'] = os.readlink(path)
        except FileNotFoundError:
            result['files'][name] = {'missing': True}
    try:
        result['openrc_runlevel'] = Path('/run/openrc/softlevel').read_text().strip()[:100]
    except FileNotFoundError:
        result['openrc_runlevel'] = None
    try:
        result['console_loglevel'] = int(Path('/proc/sys/kernel/printk').read_text().split()[0])
    except (OSError, ValueError, IndexError):
        result['console_loglevel'] = None
    return result


def agent_wait_diagnostics(items):
    result = []
    for agent in items:
        if agent['role'] != 'agent':
            continue
        tasks = []
        try:
            paths = sorted((Path('/proc') / str(agent['pid']) / 'task').iterdir(),
                           key=lambda path: int(path.name))
            for path in paths[:32]:
                try:
                    with (path / 'stat').open() as stream:
                        raw = stream.read(4096)
                    prefix, suffix = raw.rsplit(')', 1)
                    fields = suffix.split()
                    comm = prefix.split('(', 1)[1]
                    with (path / 'wchan').open() as stream:
                        wchan = stream.read(128).strip()
                    tasks.append({'tid': int(path.name), 'start': int(fields[19]), 'state': fields[0],
                                  'comm': comm if re.fullmatch(r'[A-Za-z0-9_.:+ -]{1,32}', comm) else 'unavailable',
                                  'wchan': wchan if re.fullmatch(r'[A-Za-z0-9_]{1,80}', wchan) else 'unavailable'})
                except (OSError, ValueError, IndexError):
                    continue
            current = process(agent['pid'])
            if current and all(current[key] == agent[key] for key in ('pid', 'start', 'uid', 'exe')):
                result.append({'process': current, 'tasks': tasks, 'tasks_truncated': len(paths) > 32})
        except (OSError, ValueError):
            continue
    return result


def diagnostics():
    current = state()
    result = {'state': current, 'setup_metadata': setup_metadata(),
              'agent_waits': agent_wait_diagnostics(current['processes'])}
    for name, command in {
        'openrc': ['/bin/rc-status', '-a'],
        'dmesg': ['dmesg'],
        'sessions': ['loginctl', 'list-sessions', '--no-legend'],
    }.items():
        try:
            result[name] = run(command)[-18000:]
        except (RuntimeError, subprocess.TimeoutExpired, FileNotFoundError) as error:
            result[name] = type(error).__name__
    for name, path in {'messages': '/var/log/messages',
                       'observer': '/run/alcard-ci/observer.log'}.items():
        try:
            result[name] = Path(path).read_text(errors='replace')[-20000:]
        except FileNotFoundError:
            pass
    return result


def main():
    assert os.getuid() == 0 and Path('/etc/alcard-ci-vm').is_file()
    assert 'alcard_ci_vm=1' in Path('/proc/cmdline').read_text().split()
    os.umask(0o077)
    deadline = time.monotonic() + 60
    while not PORT.exists():
        if time.monotonic() > deadline:
            raise RuntimeError('Private virtserial observer port did not appear')
        time.sleep(.1)
    with PORT.open('r+b', buffering=0) as channel:
        def send(value):
            pending = memoryview(json.dumps(value).encode() + b'\n')
            while pending:
                written = channel.write(pending)
                if not written:
                    raise RuntimeError('Private observer channel stopped accepting data')
                pending = pending[written:]

        send({'ready': True, 'protocol': 1})
        buffer = b''
        while True:
            chunk = channel.read(65536)
            if not chunk:
                time.sleep(.1)
                continue
            buffer += chunk
            if len(buffer) > 65536:
                raise RuntimeError('Oversized observer request')
            while b'\n' in buffer:
                line, buffer = buffer.split(b'\n', 1)
                request = json.loads(line)
                identifier = request.get('id')
                try:
                    operation = request.pop('op')
                    if operation == 'setup':
                        result = setup(request)
                    elif operation == 'setup-metadata':
                        result = setup_metadata()
                    elif operation == 'state':
                        result = state()
                    elif operation == 'process-state':
                        result = process_state()
                    elif operation == 'idle-resources':
                        result = idle_resources()
                    elif operation == 'proof':
                        result = proof(request)
                    elif operation == 'kill-client':
                        result = kill_client(request)
                    elif operation == 'console-proof':
                        result = console_proof()
                    elif operation == 'service':
                        result = service(request)
                    elif operation == 'service-health':
                        result = service_health()
                    elif operation == 'terminate-private-bus':
                        result = terminate_private_bus()
                    elif operation == 'diagnostics':
                        result = diagnostics()
                    else:
                        raise ValueError('Unknown observer operation')
                    reply = {'id': identifier, 'result': result}
                except Exception as error:
                    reply = {'id': identifier, 'error': f'{type(error).__name__}: {error}'}
                send(reply)


if __name__ == '__main__':
    main()
