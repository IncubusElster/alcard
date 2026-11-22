#!/usr/bin/python3
"""Private interactive-Hello observer; no provisioning or auth-success RPC.

Reuse the installed-login fixture's read-only session checks. The host releases
the boot barrier and sends actual keyboard input to the unchanged firstboot UI.
Only this disposable CI guest receives the wrapper, never a production APK.
Installed Settings additionally uses a private ordinary-user PTY and explicit
temporary guest auth/service conditions. Its own production commands apply all
changes being tested; no fixture manufactures an authentication result.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import pwd
import re
import stat
import subprocess
import time


spec = importlib.util.spec_from_file_location(
    'login_observer', Path(__file__).with_name('observer-login.py'))
base = importlib.util.module_from_spec(spec)
spec.loader.exec_module(base)
base.PROGRAMS['/usr/bin/alcard-settings'] = 'hello'
settings_spec = importlib.util.spec_from_file_location(
    'settings_observer', Path(__file__).with_name('settings-observer.py'))
settings_module = importlib.util.module_from_spec(settings_spec)
settings_spec.loader.exec_module(settings_module)
settings = settings_module.Acceptance(base)
SENTINEL = b'Alcard interactive Hello persistence acceptance v1\n'
SENTINEL_PATH = Path('/home/alcardci/hello-persistence.txt')
STATE = Path('/var/lib/alcard')
PUBLIC_FILES = (
    '/etc/profile.d/alcard-locale.sh', '/etc/alcard-keyboards',
    '/etc/alcard/keyboard-xkb.env', '/etc/keymap/alcard.bmap.gz',
    '/etc/conf.d/loadkmap', '/etc/localtime', '/etc/timezone',
    '/etc/alcard/default-session', '/etc/doas.d/alcard.conf',
    '/etc/polkit-1/rules.d/49-alcard-admin.rules',
    '/var/lib/alcard/hello.complete',
)


def boot_id():
    return Path('/proc/sys/kernel/random/boot_id').read_text().strip()


def hello_state():
    running = []
    for item in base.processes():
        if item['role'] != 'hello' or item['uid'] != 0:
            continue
        try:
            arguments = (Path('/proc') / str(item['pid']) / 'cmdline').read_bytes().split(b'\0')
            if b'--hello' in arguments or b'--firstboot' in arguments:
                running.append(item)
        except FileNotFoundError:
            pass
    text, _, error, geometry = base.tty_snapshot()
    result = {'running': bool(running), 'step': None, 'pane': None,
              'focus': None, 'editing': False, 'show_password': False,
              'columns': geometry['columns'] if geometry else None,
              'rows': geometry['rows'] if geometry else None}
    if not running or error or not geometry or geometry['columns'] < 40:
        return result
    columns = geometry['columns']
    rows = [text[offset:offset + columns] for offset in range(0, len(text), columns)]
    progress = re.fullmatch(r'([1-6])/6\s*', rows[0][36:40])
    if not progress:
        return result
    result['step'] = int(progress[1])
    title = rows[2].strip()
    result['pane'] = ('languages' if title in {
        'Choose language', 'Выбор языка', 'Elegir idioma'} else
        'keyboards' if title in {
            'Choose keyboard layouts', 'Выбор раскладок', 'Elegir teclados'} else 'main')
    # All supported main/picker pages place their first selectable row at y=6.
    selected = [index for index, row in enumerate(rows[6:15]) if row.startswith('> ')]
    if len(selected) == 1:
        result['focus'] = selected[0]
        result['editing'] = rows[6 + selected[0]].rstrip().endswith('_')
    if result['step'] == 2:
        result['show_password'] = any('[x]' in rows[index] for index in (9, 13))
    # Never return terminal contents, field text, or typed credentials.
    return result


def state():
    result = base.state()
    result['hello'] = hello_state()
    result['boot_id'] = boot_id()
    return result


def start_interactive():
    marker = base.ROOT / 'setup-complete'
    assert not marker.exists(), 'The boot barrier has already been released'
    completed = (STATE / 'hello.complete').exists()
    # Do not arm/apply firstboot here. Only the packaged tty1 dispatcher does it.
    marker.touch(mode=0o600)
    return {'released': True, 'completed_before_release': completed, 'boot_id': boot_id()}


def file_metadata(path):
    info = path.lstat()
    assert stat.S_ISREG(info.st_mode) and info.st_nlink == 1
    return {'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
            'uid': info.st_uid, 'gid': info.st_gid,
            'mode': f'{stat.S_IMODE(info.st_mode):04o}'}


def user_command(arguments, *, timeout=10):
    account = pwd.getpwnam(base.USER)
    return subprocess.run(['/bin/setpriv', '--reuid', str(account.pw_uid),
                           '--regid', str(account.pw_gid), '--init-groups', *arguments],
                          stdin=subprocess.DEVNULL, capture_output=True, timeout=timeout)


def persistence_check():
    account = pwd.getpwnam(base.USER)
    assert 1000 <= account.pw_uid < 65534 and account.pw_dir == '/home/alcardci'
    assert not hello_state()['running'], 'Hello is still running'
    assert (STATE / 'hello.complete').read_text() == base.USER + '\n'
    assert not (STATE / 'hello.pending').exists()
    files = {name: file_metadata(Path(name)) for name in PUBLIC_FILES}
    assert all(info['uid'] == info['gid'] == 0 for info in files.values())
    assert files['/var/lib/alcard/hello.complete']['mode'] == '0600'
    assert files['/etc/doas.d/alcard.conf']['mode'] == '0600'
    assert files['/etc/polkit-1/rules.d/49-alcard-admin.rules']['mode'] == '0644'
    root_password = next(line.split(':')[1] for line in Path('/etc/shadow').read_text().splitlines()
                         if line.startswith('root:'))
    root_locked = root_password.startswith(('!', '*'))
    del root_password
    assert root_locked
    import grp
    groups = sorted(group.gr_name for group in grp.getgrall() if base.USER in group.gr_mem)
    assert not {'wheel', 'video', 'input'} & set(groups)
    syntax = subprocess.run(['/usr/bin/doas', '-C', '/etc/doas.d/alcard.conf'],
                            stdin=subprocess.DEVNULL, capture_output=True, timeout=10)
    assert syntax.returncode == 0, 'Doas policy syntax is invalid'
    denied = user_command(['/usr/bin/doas', '-n', '/bin/true'])
    password_required = (denied.returncode != 0 and
                         b'Authentication required' in denied.stderr)
    assert password_required, 'The intended doas password route is unavailable'
    locale_text = Path('/etc/profile.d/alcard-locale.sh').read_text()
    locale = re.fullmatch(r"export LANG='([A-Za-z_0-9.\-]+)'\n", locale_text)
    assert locale, 'Unexpected locale configuration'
    sentinel = file_metadata(SENTINEL_PATH)
    assert SENTINEL_PATH.read_bytes() == SENTINEL
    assert sentinel['uid'] == account.pw_uid and sentinel['gid'] == account.pw_gid
    assert sentinel['mode'] == '0600'
    assert user_command(['/bin/sh', '-c',
                         'test -r /home/alcardci/hello-persistence.txt']).returncode == 0
    ntp = any(os.path.lexists('/etc/runlevels/default/' + name)
              for name in ('chronyd', 'ntpd'))
    return {'boot_id': boot_id(), 'account': {'uid': account.pw_uid, 'gid': account.pw_gid,
                                            'home': account.pw_dir},
            'complete': True, 'pending': False, 'root_locked': root_locked, 'groups': groups,
            'locale': locale[1], 'keyboards': Path('/etc/alcard-keyboards').read_text().strip(),
            'timezone': Path('/etc/timezone').read_text().strip(),
            'default_session': Path('/etc/alcard/default-session').read_text().strip(),
            'ntp_enabled': ntp, 'sentinel': sentinel, 'files': files,
            'doas': {'syntax_valid': True, 'password_required': password_required}}


def prepare_login():
    # The inherited login gate proves agent readiness through the ordinary
    # launcher before it returns. Prepare that harmless entry before first
    # login, using the provisioned user's UID; do not create a login/session.
    code = '''from pathlib import Path
import os
os.umask(0o077)
home = Path('/home/alcardci')
directory = home / '.local/share/applications'
directory.mkdir(parents=True, exist_ok=True)
desktop = directory / 'alcard-ci-policy.desktop'
with desktop.open('x') as stream:
    stream.write('[Desktop Entry]\\nType=Application\\nName=000 CI Policy Probe\\n'
                 'Exec=/usr/libexec/alcard-ci/policy-probe\\nTerminal=true\\n')
    stream.flush()
    os.fsync(stream.fileno())
'''
    result = user_command(['/usr/bin/python3', '-c', code])
    assert result.returncode == 0, 'The provisioned user could not create the ordinary launcher'
    return {'prepared': True}


def persistence_seed():
    assert not SENTINEL_PATH.exists()
    assert base.session_environment().get('DBUS_SESSION_BUS_ADDRESS')
    code = '''from pathlib import Path
import os
os.umask(0o077)
with Path('/home/alcardci/hello-persistence.txt').open('xb') as stream:
    stream.write(b'Alcard interactive Hello persistence acceptance v1\\n')
    stream.flush()
    os.fsync(stream.fileno())
'''
    result = user_command(['/usr/bin/python3', '-c', code])
    assert result.returncode == 0, 'The logged-in user could not create the home sentinel'
    os.sync()
    return persistence_check()


def idle_resources():
    result = subprocess.run([
        '/usr/bin/python3', '/usr/libexec/alcard-ci/measure-idle-resources.py',
        '--uid', str(base.uid()), '--settle', '10', '--duration', '30'],
        stdin=subprocess.DEVNULL, capture_output=True, timeout=55)
    assert result.returncode == 0, 'Idle resource sampler failed'
    return json.loads(result.stdout)


def reboot():
    assert (STATE / 'hello.complete').is_file() and not hello_state()['running']
    os.sync()
    # Reply before the ordinary OpenRC shutdown closes the private channel.
    subprocess.Popen(['/bin/sh', '-c', 'sleep 1; exec /sbin/reboot'],
                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                     stderr=subprocess.DEVNULL, start_new_session=True)
    return {'requested': True, 'boot_id': boot_id()}


def dispatch(request):
    operation = request.pop('op')
    if operation.startswith('settings-'):
        return settings.dispatch(operation.removeprefix('settings-'), request)
    handlers = {
        'start-interactive': start_interactive, 'hello-state': hello_state,
        'state': state, 'setup-metadata': base.setup_metadata,
        'process-state': base.process_state, 'prepare-login': prepare_login,
        'service-health': base.service_health,
        'persistence-seed': persistence_seed, 'persistence-check': persistence_check,
        'idle-resources': idle_resources, 'reboot': reboot,
    }
    if operation == 'proof':
        return base.proof(request)
    if operation == 'diagnostics':
        return {'state': state(), 'setup_metadata': base.setup_metadata()}
    if operation not in handlers:
        raise ValueError('Operation is outside interactive Hello acceptance')
    return handlers[operation]()


def main():
    assert os.getuid() == 0 and Path('/etc/alcard-ci-vm').is_file()
    assert 'alcard_ci_vm=1' in Path('/proc/cmdline').read_text().split()
    os.umask(0o077)
    deadline = time.monotonic() + 60
    while not base.PORT.exists():
        if time.monotonic() > deadline:
            raise RuntimeError('Private observer port did not appear')
        time.sleep(.1)
    with base.PORT.open('r+b', buffering=0) as channel:
        def send(value):
            pending = memoryview(json.dumps(value).encode() + b'\n')
            while pending:
                written = channel.write(pending)
                if not written:
                    raise RuntimeError('Private observer channel closed')
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
                    reply = {'id': identifier, 'result': dispatch(request)}
                except Exception as error:
                    # All new assertions/messages are fixed and exclude terminal text.
                    reply = {'id': identifier, 'error': f'{type(error).__name__}: {error}'}
                send(reply)


if __name__ == '__main__':
    main()
